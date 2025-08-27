use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use native_tls::TlsConnector as NativeTlsConnector;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector as TokioTlsConnector;

// 🔁 تغییر: افزایش timeout به 15 ثانیه
const IP_RESOLVER: &str = "proxy.ndeso.xyz";
const PROXY_FILE: &str = "Data/test.txt";
const OUTPUT_FILE: &str = "active_proxies.md";
const MAX_CONCURRENT: usize = 175;
const TIMEOUT_SECONDS: u64 = 15; // ⏱️ افزایش timeout

// 🔁 تغییر: اضافه کردن نام کامل کشورها و پرچم
// می‌تونی این مپ رو گسترش بدی
const COUNTRY_NAMES: &[(&str, &str)] = &[
    ("IR", "Iran"),
    ("US", "United States"),
    ("DE", "Germany"),
    ("FR", "France"),
    ("GB", "United Kingdom"),
    ("CA", "Canada"),
    ("AU", "Australia"),
    ("JP", "Japan"),
    ("KR", "South Korea"),
    ("CN", "China"),
    ("TR", "Turkey"),
    ("RU", "Russia"),
    ("IN", "India"),
    ("BR", "Brazil"),
    ("IT", "Italy"),
    ("ES", "Spain"),
    ("NL", "Netherlands"),
    ("SE", "Sweden"),
    ("CH", "Switzerland"),
    ("NO", "Norway"),
    // می‌تونی بقیه رو اضافه کنی
];

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize, Debug, Clone)]
struct ProxyInfo {
    ip: String,
    delay: String,
    isp: String,
    asn: String,
    city: String,
    region: String,
    country: String, // دو حرفی مثل "IR"
    countryflag: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Starting proxy scanner...");

    if let Some(parent) = Path::new(OUTPUT_FILE).parent() {
        fs::create_dir_all(parent)?;
    }
    File::create(OUTPUT_FILE)?;

    let proxies = read_proxy_file(PROXY_FILE)?;
    println!("Loaded {} proxies from file", proxies.len());

    let proxies: Vec<String> = proxies
        .into_iter()
        .filter(|line| {
            line.split(',')
                .nth(1)
                .and_then(|p| p.trim().parse::<u16>().ok())
                .map_or(false, |p| p == 443)
        })
        .collect();
    println!("Filtered to {} proxies on port 443.", proxies.len());

    let active_proxies = Arc::new(Mutex::new(HashMap::<String, Vec<ProxyInfo>>::new()));

    let tasks = futures::stream::iter(
        proxies.into_iter().map(|proxy_line| {
            let active_proxies = Arc::clone(&active_proxies);
            async move {
                process_proxy(proxy_line, &active_proxies).await;
            }
        })
    )
    .buffer_unordered(MAX_CONCURRENT)
    .collect::<Vec<()>>();

    tasks.await;

    write_markdown_file(&active_proxies.lock().unwrap())?;

    println!("Proxy checking completed.");
    Ok(())
}

fn get_full_country_name(code: &str) -> &str {
    COUNTRY_NAMES
        .iter()
        .find(|&&(c, _)| c == code)
        .map(|&(_, name)| name)
        .unwrap_or("Unknown Country")
}

fn write_markdown_file(proxies_by_country: &HashMap<String, Vec<ProxyInfo>>) -> io::Result<()> {
    let mut file = File::create(OUTPUT_FILE)?;
    writeln!(file, "# Active Proxies")?;
    writeln!(file, "")?;
    writeln!(file, ">  Total: **{}** active HTTPS proxies found", proxies_by_country.values().map(|v| v.len()).sum::<usize>())?;
    writeln!(file, "")?;

    if proxies_by_country.is_empty() {
        writeln!(file, "🔴 No active proxies found.")?;
        return Ok(());
    }

    // مرتب‌سازی کشورها بر اساس نام کامل
    let mut countries: Vec<_> = proxies_by_country.keys().collect();
    countries.sort_by_key(|code| get_full_country_name(code));

    for country_code in countries {
        let full_name = get_full_country_name(country_code);
        let flag = proxies_by_country[country_code][0].countryflag.clone();

        // 🔁 نمایش پروتکل (HTTPS) کنار کشور
        writeln!(file, "## {} {} (HTTPS)", flag, full_name)?;
        writeln!(file, "")?;

        if let Some(proxies) = proxies_by_country.get(country_code) {
            for info in proxies {
                writeln!(file, "- **IP:** `{}`", info.ip)?;
                writeln!(file, "  - **Location:** {} {}, {}", info.countryflag, info.city, info.region)?;
                writeln!(file, "  - **ISP:** {} –– **ASN:** {}", info.isp, info.asn)?;
                writeln!(file, "  - **Ping:** {}", info.delay)?;
                writeln!(file, "")?;
            }
        }
    }

    println!("All active proxies saved to {}", OUTPUT_FILE);
    Ok(())
}

fn read_proxy_file(file_path: &str) -> io::Result<Vec<String>> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let mut proxies = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if !line.trim().is_empty() {
            proxies.push(line);
        }
    }

    Ok(proxies)
}

async fn check_connection(
    host: &str,
    path: &str,
    proxy: Option<(&str, u16)>,
) -> Result<Value> {
    let timeout_duration = Duration::from_secs(TIMEOUT_SECONDS);

    match tokio::time::timeout(timeout_duration, async {
        let payload = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/42.0.2311.135 Safari/537.36 Edge/12.10240\r\n\
             Connection: close\r\n\r\n",
            path, host
        );

        let connect_addr = if let Some((proxy_ip, proxy_port)) = proxy {
            if proxy_ip.contains(':') {
                format!("[{}]:{}", proxy_ip, proxy_port)
            } else {
                format!("{}:{}", proxy_ip, proxy_port)
            }
        } else {
            format!("{}:443", host)
        };

        let stream = TcpStream::connect(&connect_addr).await?;

        let native_connector = NativeTlsConnector::builder().build()?;
        let tokio_connector = TokioTlsConnector::from(native_connector);
        let mut tls_stream = tokio_connector.connect(host, stream).await?;

        tls_stream.write_all(payload.as_bytes()).await?;

        let mut response = Vec::new();
        let mut buffer = [0; 4096];

        loop {
            match tls_stream.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buffer[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            }
        }

        let response_str = String::from_utf8_lossy(&response);

        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body = &response_str[body_start + 4..].trim();
            match serde_json::from_str::<Value>(body) {
                Ok(json) => Ok(json),
                Err(e) => {
                    eprintln!("JSON Parse Error: {}", e);
                    eprintln!("Raw Body: {}", body);
                    Err("Invalid JSON".into())
                }
            }
        } else {
            Err("No HTTP body separator".into())
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Request timed out",
        )) as Box<dyn std::error::Error + Send + Sync>),
    }
}

async fn process_proxy(
    proxy_line: String,
    active_proxies: &Arc<Mutex<HashMap<String, Vec<ProxyInfo>>>>,
) {
    let parts: Vec<&str> = proxy_line.split(',').collect();
    if parts.len() < 2 {
        return;
    }
    let ip = parts[0];
    let port = parts[1];
    let full_proxy_address = format!("{}:{}", ip, port);

    let path = format!("/check?ip={}", full_proxy_address);

    match check_connection(IP_RESOLVER, &path, None).await {
        Ok(proxy_data) => {
            match serde_json::from_value::<ProxyInfo>(proxy_data) {
                Ok(info) => {
                    println!("🟢 PROXY LIVE: {} | {} | {} | {}", info.ip, info.country, info.city, info.delay);
                    let mut active_proxies_locked = active_proxies.lock().unwrap();
                    active_proxies_locked
                        .entry(info.country.clone())
                        .or_default()
                        .push(info);
                }
                Err(e) => {
                    // فقط در حالت development نمایش بده
                    #[cfg(debug_assertions)]
                    println!("🔴 JSON Parse Failed for {}: {}", full_proxy_address, e);
                }
            }
        }
        Err(e) => {
            // فقط در صورت نیاز لاگ کن (مثلاً توی توسعه)
            // توی GitHub Actions اینو خاموش کن تا لاگ شلوغ نشه
            #[cfg(debug_assertions)]
            println!("🔴 PROXY DEAD (Timeout/Error): {} - {}", full_proxy_address, e);
        }
    }
}
