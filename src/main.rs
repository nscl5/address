use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use native_tls::TlsConnector as NativeTlsConnector; // Renamed to avoid conflict
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt}; // Untuk read_exact, write_all async
use tokio::net::TcpStream; // TcpStream async dari Tokio
use tokio_native_tls::TlsConnector as TokioTlsConnector; // Konektor TLS async

const IP_RESOLVER: &str = "proxy.ndeso.xyz";
const PROXY_FILE: &str = "Data/Proxy-September.txt"; //input
const OUTPUT_FILE: &str = "active_proxies.md";
const MAX_CONCURRENT: usize = 175;
const TIMEOUT_SECONDS: u64 = 9;

// Define a custom error type that implements Send + Sync
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize, Debug, Clone)]
struct ProxyInfo {
    ip: String,
    delay: String,
    isp: String,
    asn: String,
    city: String,
    region: String,
    country: String,
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

fn write_markdown_file(proxies_by_country: &HashMap<String, Vec<ProxyInfo>>) -> io::Result<()> {
    let mut file = File::create(OUTPUT_FILE)?;
    writeln!(file, "# Active Proxies")?;
    writeln!(file, "")?;

    if proxies_by_country.is_empty() {
        writeln!(file, "No active proxies found.")?;
        println!("No active proxies found");
        return Ok(());
    }

    let mut countries: Vec<_> = proxies_by_country.keys().collect();
    countries.sort();

    for country_name in countries {
        writeln!(file, "## {}", country_name)?;
        if let Some(proxies) = proxies_by_country.get(country_name) {
            for info in proxies {
                writeln!(file, "Location: {} {}", info.countryflag, info.region)?;
                writeln!(file, "City: {}", info.city)?;
                writeln!(file, "ISP: {} – ${}", info.isp, info.asn)?;
                writeln!(file, "Ping: {}", info.delay)?;
                writeln!(file, "proxyIP: {}", info.ip)?;
                writeln!(file, "")?; // Add a blank line between entries
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

    // Bungkus seluruh operasi koneksi dalam tokio::time::timeout
    match tokio::time::timeout(timeout_duration, async {
        // Build HTTP request payload
        let payload = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/42.0.2311.135 Safari/537.36 Edge/12.10240\r\n\
             Connection: close\r\n\r\n",
            path, host
        );

        // Create TCP connection
        let stream = if let Some((proxy_ip, proxy_port)) = proxy {
            // *** PERUBAHAN UTAMA DI SINI ***
            // Menangani alamat IPv6 dengan benar dengan membungkusnya dalam kurung siku.
            let connect_addr = if proxy_ip.contains(':') {
                // Ini adalah alamat IPv6, formatnya menjadi "[ipv6]:port"
                format!("[{}]:{}", proxy_ip, proxy_port)
            } else {
                // Ini adalah alamat IPv4, formatnya tetap "ipv4:port"
                format!("{}:{}", proxy_ip, proxy_port)
            };
            TcpStream::connect(connect_addr).await?
        } else {
            // Connect directly to host (Tokio's connect can resolve hostnames)
            TcpStream::connect(format!("{}:443", host)).await?
        };

        // Create TLS connection
        // NativeTlsConnector dikonfigurasi terlebih dahulu
        let native_connector = NativeTlsConnector::builder().build()?;
        // Kemudian dibungkus dengan TokioTlsConnector untuk penggunaan async
        let tokio_connector = TokioTlsConnector::from(native_connector);

        let mut tls_stream = tokio_connector.connect(host, stream).await?;

        // Send HTTP request
        tls_stream.write_all(payload.as_bytes()).await?;

        // Read response
        let mut response = Vec::new();
        // Menggunakan buffer yang sama ukurannya
        let mut buffer = [0; 4096];

        // Loop untuk membaca data dari stream
        // AsyncReadExt::read akan mengembalikan Ok(0) saat EOF.
        loop {
            match tls_stream.read(&mut buffer).await {
                Ok(0) => break, // End of stream
                Ok(n) => response.extend_from_slice(&buffer[..n]),
                Err(e) => {
                    // Jika jenis error adalah WouldBlock, dalam konteks async,
                    // ini biasanya ditangani oleh runtime (tidak akan sampai ke sini jika .await digunakan dengan benar).
                    // Namun, jika ada error I/O lain, kita return.
                    return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                }
            }
        }

        // Parse response
        let response_str = String::from_utf8_lossy(&response);

        // Split headers and body
        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body = &response_str[body_start + 4..];

            // Try to parse the JSON body
            match serde_json::from_str::<Value>(body.trim()) {
                Ok(json_data) => Ok(json_data),
                Err(e) => {
                    eprintln!("Failed to parse JSON: {}", e);
                    eprintln!("Response body for {}:{}: {}", host, proxy.map_or_else(|| "direct".to_string(), |(ip,p)| format!("{}:{}",ip,p)), body);
                    Err("Invalid JSON response".into())
                }
            }
        } else {
            Err("Invalid HTTP response: No separator found".into())
        }
    }).await {
        Ok(inner_result) => inner_result, // Hasil dari blok async (bisa Ok atau Err)
        Err(_) => Err(Box::new(io::Error::new(io::ErrorKind::TimedOut, "Connection attempt timed out")) as Box<dyn std::error::Error + Send + Sync>), // Error karena timeout
    }
}


async fn process_proxy(
    proxy_line: String,
    active_proxies: &Arc<Mutex<HashMap<String, Vec<ProxyInfo>>>>,
) {
    let ip = match proxy_line.split(',').next() {
        Some(ip) => ip,
        None => return,
    };

    let path = format!("/check?ip={}", ip);

    match check_connection(IP_RESOLVER, &path, None).await {
        Ok(proxy_data) => {
            // Try to deserialize the JSON response into our ProxyInfo struct
            match serde_json::from_value::<ProxyInfo>(proxy_data.clone()) {
                Ok(info) => {
                    println!("PROXY LIVE ✅: {}", info.ip);
                    let mut active_proxies_locked = active_proxies.lock().unwrap();

                    active_proxies_locked
                        .entry(info.country.clone())
                        .or_default()
                        .push(info);
                },
                Err(e) => {
                    // The API responded, but the JSON format did not match our ProxyInfo struct.
                    println!("PROXY DEAD ❌ (JSON parsing error): {} - Error: {}, Response: {:?}", ip, e, proxy_data);
                }
            }
        },
        Err(e) => {
            // The connection to the API failed (e.g., timeout).
            println!("PROXY DEAD ⏱️ (Error checking): {} - {}", ip, e);
        }
    }
}
