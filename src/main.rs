use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const IP_RESOLVER: &str = "ip-api.com";
const PROXY_FILE: &str = "Data/test.txt";
const OUTPUT_FILE: &str = "active_proxies.md";
const MAX_CONCURRENT: usize = 150;
const TIMEOUT_SECONDS: u64 = 9;

const GOOD_ISPS: &[&str] = &[
    "google",
    "amazon",
    "cloudflare",
    "microsoft",
    "hetzner",
    "digitalocean",
    "rackspace",
    "m247",
    "datacamp",
    "total uptime",
    "gmbh",
    "stark industries",
];

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize, Debug, Clone)]
struct ProxyInfo {
    query: String,
    isp: String,
    org: String,
    #[serde(rename = "as")]
    asn: String,
    city: String,
    regionName: String,
    country: String,
    countryCode: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(parent) = Path::new(OUTPUT_FILE).parent() {
        fs::create_dir_all(parent)?;
    }
    File::create(OUTPUT_FILE)?;

    let proxies = read_proxy_file(PROXY_FILE)?;
    println!("Loaded {} proxies from file", proxies.len());

    let proxies: Vec<String> = proxies
        .into_iter()
        .filter(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() < 4 {
                return false;
            }
            let port_ok = parts[1].trim() == "443";

            let isp_name = parts[3].to_lowercase();
            let isp_ok = GOOD_ISPS.iter().any(|kw| isp_name.contains(&kw.to_lowercase()));

            port_ok && isp_ok
        })
        .collect();

    println!(
        "Filtered to {} good proxies (port 443 + ISP whitelist)",
        proxies.len()
    );

    let active_proxies = Arc::new(Mutex::new(HashMap::<String, Vec<ProxyInfo>>::new()));

    let tasks = futures::stream::iter(
        proxies.into_iter().map(|proxy_line| {
            let active_proxies = Arc::clone(&active_proxies);
            async move {
                process_proxy(proxy_line, &active_proxies).await;
            }
        }),
    )
    .buffer_unordered(MAX_CONCURRENT)
    .collect::<Vec<()>>();

    tasks.await;

    write_markdown_file(&active_proxies.lock().unwrap())?;

    println!("Proxy checking completed.");
    Ok(())
}

fn write_markdown_file(proxies_by_country: &HashMap<String, Vec<ProxyInfo>>) -> io::Result<()> {
    let mut file = File::create(OUTPUT_FILE)?;
    writeln!(file, "# Active Proxies\n")?;

    if proxies_by_country.is_empty() {
        writeln!(file, "No active proxies found.")?;
        println!("No active proxies found");
        return Ok(());
    }

    let mut countries: Vec<_> = proxies_by_country.keys().collect();
    countries.sort();

    for country_name in countries {
        if let Some(proxies) = proxies_by_country.get(country_name) {
            if proxies.is_empty() {
                continue;
            }

            let flag = country_flag(&proxies[0].countryCode);

            writeln!(file, "## {} {}\n", flag, country_name)?;
            writeln!(file, "| IP | Location | ISP | ASN | Ping |")?;
            writeln!(file, "|----|----------|-----|-----|------|")?;

            for info in proxies {
                let location = format!("{}, {}", info.regionName, info.city);
                writeln!(
                    file,
                    "| `{}` | {} | {} | {} | ok |",
                    info.query, location, info.isp, info.asn
                )?;
            }
            writeln!(file, "\n---\n")?;
        }
    }

    println!("All active proxies saved to {}", OUTPUT_FILE);
    Ok(())
}

fn country_flag(code: &str) -> String {
    code.chars()
        .filter_map(|c| {
            if c.is_ascii_alphabetic() {
                Some(char::from_u32(
                    0x1F1E6 + (c.to_ascii_uppercase() as u32 - 'A' as u32),
                ).unwrap())
            } else {
                None
            }
        })
        .collect()
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

async fn check_connection(host: &str, path: &str) -> Result<Value> {
    let timeout_duration = Duration::from_secs(TIMEOUT_SECONDS);

    match tokio::time::timeout(timeout_duration, async {
        let payload = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: RustScanner\r\n\
             Connection: close\r\n\r\n",
            path, host
        );

        let mut stream = TcpStream::connect(format!("{}:80", host)).await?;
        stream.write_all(payload.as_bytes()).await?;

        let mut response = Vec::new();
        let mut buffer = [0; 4096];

        loop {
            match stream.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buffer[..n]),
                Err(e) => return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            }
        }

        let response_str = String::from_utf8_lossy(&response);
        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body = &response_str[body_start + 4..];
            match serde_json::from_str::<Value>(body.trim()) {
                Ok(json_data) => Ok(json_data),
                Err(e) => Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            }
        } else {
            Err(Box::<dyn std::error::Error + Send + Sync>::from(
                "Invalid HTTP response",
            ))
        }
    }).await {
        Ok(inner_result) => inner_result,
        Err(_) => Err(Box::<dyn std::error::Error + Send + Sync>::from(
            "Connection attempt timed out",
        )),
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

    let path = format!("/json/{}", ip);

    match check_connection(IP_RESOLVER, &path).await {
        Ok(proxy_data) => match serde_json::from_value::<ProxyInfo>(proxy_data.clone()) {
            Ok(info) => {
                println!("PROXY LIVE ✅: {}", info.query);
                let mut active_proxies_locked = active_proxies.lock().unwrap();
                active_proxies_locked
                    .entry(info.country.clone())
                    .or_default()
                    .push(info);
            }
            Err(_) => {
                println!("PROXY DEAD ❌ (Invalid JSON): {}", ip);
            }
        },
        Err(e) => {
            println!("PROXY DEAD ⏱️ ({}): {}", ip, e);
        }
    }
}
