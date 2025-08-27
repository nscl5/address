use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use native_tls::TlsConnector as NativeTlsConnector;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector as TokioTlsConnector;

const PROXY_FILE: &str = "Data/test.txt";
const OUTPUT_FILE: &str = "active_proxies.md";
const MAX_CONCURRENT: usize = 30; // کاهش بیشتر برای جلوگیری از rate limit
const TIMEOUT_SECONDS: u64 = 15;
const TEST_TARGET: &str = "httpbin.org"; // سایت تست
const TEST_PATH: &str = "/ip"; // مسیر برای تست

// نقشه کشورها برای تبدیل کد دو حرفی به نام کامل
fn get_country_name_with_flag(country_code: &str, country_name: &str) -> String {
    let flag = match country_code.to_uppercase().as_str() {
        "US" => "🇺🇸",
        "GB" => "🇬🇧",
        "DE" => "🇩🇪",
        "FR" => "🇫🇷",
        "CA" => "🇨🇦",
        "AU" => "🇦🇺",
        "JP" => "🇯🇵",
        "CN" => "🇨🇳",
        "IN" => "🇮🇳",
        "BR" => "🇧🇷",
        "RU" => "🇷🇺",
        "IT" => "🇮🇹",
        "ES" => "🇪🇸",
        "NL" => "🇳🇱",
        "KR" => "🇰🇷",
        "TR" => "🇹🇷",
        "MX" => "🇲🇽",
        "AR" => "🇦🇷",
        "PL" => "🇵🇱",
        "SE" => "🇸🇪",
        "NO" => "🇳🇴",
        "DK" => "🇩🇰",
        "FI" => "🇫🇮",
        "CH" => "🇨🇭",
        "AT" => "🇦🇹",
        "BE" => "🇧🇪",
        "PT" => "🇵🇹",
        "GR" => "🇬🇷",
        "CZ" => "🇨🇿",
        "HU" => "🇭🇺",
        "RO" => "🇷🇴",
        "BG" => "🇧🇬",
        "HR" => "🇭🇷",
        "SK" => "🇸🇰",
        "SI" => "🇸🇮",
        "LT" => "🇱🇹",
        "LV" => "🇱🇻",
        "EE" => "🇪🇪",
        "IE" => "🇮🇪",
        "IS" => "🇮🇸",
        "LU" => "🇱🇺",
        "MT" => "🇲🇹",
        "CY" => "🇨🇾",
        "UA" => "🇺🇦",
        "BY" => "🇧🇾",
        "MD" => "🇲🇩",
        "IL" => "🇮🇱",
        "SA" => "🇸🇦",
        "AE" => "🇦🇪",
        "EG" => "🇪🇬",
        "ZA" => "🇿🇦",
        "NG" => "🇳🇬",
        "KE" => "🇰🇪",
        "MA" => "🇲🇦",
        "TH" => "🇹🇭",
        "SG" => "🇸🇬",
        "MY" => "🇲🇾",
        "ID" => "🇮🇩",
        "PH" => "🇵🇭",
        "VN" => "🇻🇳",
        "BD" => "🇧🇩",
        "PK" => "🇵🇰",
        "LK" => "🇱🇰",
        "NZ" => "🇳🇿",
        "CL" => "🇨🇱",
        "PE" => "🇵🇪",
        "CO" => "🇨🇴",
        "VE" => "🇻🇪",
        "IR" => "🇮🇷",
        _ => "🌍",
    };
    format!("{} {}", flag, country_name)
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize, Debug, Clone)]
struct IpApiResponse {
    status: String,
    country: Option<String>,
    #[serde(rename = "countryCode")]
    country_code: Option<String>,
    region: Option<String>,
    #[serde(rename = "regionName")]
    region_name: Option<String>,
    city: Option<String>,
    zip: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    timezone: Option<String>,
    isp: Option<String>,
    org: Option<String>,
    #[serde(rename = "as")]
    asn: Option<String>,
    query: Option<String>,
}

#[derive(Debug, Clone)]
struct ProxyInfo {
    ip: String,
    delay: String,
    isp: String,
    asn: String,
    city: String,
    region: String,
    country: String,
    country_code: String,
    response_time: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🔎 Starting advanced proxy scanner...");

    if let Some(parent) = Path::new(OUTPUT_FILE).parent() {
        fs::create_dir_all(parent)?;
    }
    File::create(OUTPUT_FILE)?;

    let proxies = read_proxy_file(PROXY_FILE)?;
    println!("📁 Loaded {} proxies from file", proxies.len());

    let proxies: Vec<String> = proxies
        .into_iter()
        .filter(|line| {
            line.split(',')
                .nth(1)
                .and_then(|p| p.trim().parse::<u16>().ok())
                .map_or(false, |p| p == 443)
        })
        .collect();
    println!("🔍 Filtered to {} proxies on port 443.", proxies.len());

    let active_proxies = Arc::new(Mutex::new(HashMap::<String, Vec<ProxyInfo>>::new()));

    let tasks = futures::stream::iter(
        proxies.into_iter().map(|proxy_line| {
            let active_proxies = Arc::clone(&active_proxies);
            async move {
                process_proxy(proxy_line, &active_proxies).await;
                // کمی تاخیر برای جلوگیری از rate limiting
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    )
    .buffer_unordered(MAX_CONCURRENT)
    .collect::<Vec<()>>();

    tasks.await;

    write_markdown_file(&active_proxies.lock().unwrap())?;

    println!("🟢 Proxy checking completed.");
    Ok(())
}

fn write_markdown_file(proxies_by_country: &HashMap<String, Vec<ProxyInfo>>) -> io::Result<()> {
    let mut file = File::create(OUTPUT_FILE)?;
    
    // نوشتن هدر فایل
    writeln!(file, "# 🌐 Active Proxies Report")?;
    writeln!(file, "")?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    writeln!(file, "**Generated at:** {}", timestamp)?;
    writeln!(file, "")?;
    
    if proxies_by_country.is_empty() {
        writeln!(file, "## 🔴 No Active Proxies Found")?;
        writeln!(file, "")?;
        writeln!(file, "No working proxies were detected during the scan.")?;
        println!("🔴 No active proxies found");
        return Ok(());
    }

    // شمارش کل پروکسی‌ها
    let total_proxies: usize = proxies_by_country.values().map(|v| v.len()).sum();
    writeln!(file, "## 📊 Summary")?;
    writeln!(file, "")?;
    writeln!(file, "- **Total Active Proxies:** {}", total_proxies)?;
    writeln!(file, "- **Countries:** {}", proxies_by_country.len())?;
    writeln!(file, "")?;
    writeln!(file, "---")?;
    writeln!(file, "")?;

    // مرتب‌سازی کشورها براساس نام
    let mut countries: Vec<_> = proxies_by_country.iter().collect();
    countries.sort_by(|a, b| a.0.cmp(b.0));

    for (country_key, proxies) in countries {
        writeln!(file, "## {} ({} proxies)", country_key, proxies.len())?;
        writeln!(file, "")?;
        
        // مرتب‌سازی پروکسی‌ها براساس response time
        let mut sorted_proxies = proxies.clone();
        sorted_proxies.sort_by(|a, b| {
            a.response_time.partial_cmp(&b.response_time).unwrap_or(std::cmp::Ordering::Equal)
        });

        for (index, info) in sorted_proxies.iter().enumerate() {
            writeln!(file, "### 🪄 Proxy #{}", index + 1)?;
            writeln!(file, "")?;
            writeln!(file, "| Field | Value |")?;
            writeln!(file, "|-------|-------|")?;
            writeln!(file, "| **IP Address** | `{}` |", info.ip)?;
            writeln!(file, "| **Location** | {} |", format!("{}, {}", info.city, info.region))?;
            writeln!(file, "| **ISP** | {} |", info.isp)?;
            writeln!(file, "| **ASN** | {} |", info.asn)?;
            
            // تشخیص سرعت براساس response time
            let speed_emoji = if info.response_time < 200.0 { "⚡" }
                else if info.response_time < 500.0 { "🌟" }
                else if info.response_time < 1000.0 { "🐌" }
                else { "🦆" };
            
            writeln!(file, "| **Response Time** | {} {:.0}ms |", speed_emoji, info.response_time)?;
            writeln!(file, "")?;
            
            // جداکننده بین پروکسی‌ها
            if index < sorted_proxies.len() - 1 {
                writeln!(file, "---")?;
                writeln!(file, "")?;
            }
        }
        
        writeln!(file, "")?;
        writeln!(file, "---")?;
        writeln!(file, "")?;
    }

    // اضافه کردن فوتر
    writeln!(file, "## 🔧 Technical Notes")?;
    writeln!(file, "")?;
    writeln!(file, "- All proxies tested on port 443 (HTTPS)")?;
    writeln!(file, "- Timeout: {} seconds", TIMEOUT_SECONDS)?;
    writeln!(file, "- Speed indicators: ⚡ Fast (<200ms) | 🌟 Good (<500ms) | 🐌 Slow (<1000ms) | 🦆 Very Slow (>1000ms)")?;
    writeln!(file, "- Geolocation data provided by ip-api.com")?;
    writeln!(file, "")?;
    writeln!(file, "---")?;
    writeln!(file, "*Report generated by Rust Proxy Scanner*")?;

    println!("📝 All active proxies saved to {}", OUTPUT_FILE);
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

// تست اتصال از طریق پروکسی
async fn test_proxy_connection(proxy_ip: &str, proxy_port: u16) -> Result<(bool, f64)> {
    let start_time = Instant::now();
    let timeout_duration = Duration::from_secs(TIMEOUT_SECONDS);

    match tokio::time::timeout(timeout_duration, async {
        // ایجاد HTTP request برای تست
        let payload = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36\r\n\
             Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\n\
             Accept-Language: en-US,en;q=0.5\r\n\
             Accept-Encoding: gzip, deflate\r\n\
             Connection: close\r\n\r\n",
            TEST_PATH, TEST_TARGET
        );

        let connect_addr = if proxy_ip.contains(':') {
            format!("[{}]:{}", proxy_ip, proxy_port)
        } else {
            format!("{}:{}", proxy_ip, proxy_port)
        };

        let stream = TcpStream::connect(connect_addr).await?;

        // ایجاد اتصال TLS
        let native_connector = NativeTlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()?;
        let tokio_connector = TokioTlsConnector::from(native_connector);

        let mut tls_stream = tokio_connector.connect(TEST_TARGET, stream).await?;

        // ارسال درخواست
        tls_stream.write_all(payload.as_bytes()).await?;

        // خواندن پاسخ
        let mut response = Vec::new();
        let mut buffer = [0; 4096];

        loop {
            match tls_stream.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buffer[..n]),
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response);
        
        // بررسی موفقیت‌آمیز بودن پاسخ
        if response_str.contains("200 OK") || response_str.contains("origin") {
            Ok(true)
        } else {
            Err("Invalid response".into())
        }
    }).await {
        Ok(Ok(_)) => {
            let elapsed = start_time.elapsed().as_millis() as f64;
            Ok((true, elapsed))
        },
        _ => Ok((false, -1.0))
    }
}

// دریافت اطلاعات IP از ip-api.com
async fn get_ip_info(ip: &str) -> Result<IpApiResponse> {
    let timeout_duration = Duration::from_secs(10);
    
    match tokio::time::timeout(timeout_duration, async {
        
        // ایجاد HTTP request برای ip-api
        let host = "ip-api.com";
        let path = format!("/json/{}?lang=en", ip);
        
        let payload = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36\r\n\
             Accept: application/json\r\n\
             Connection: close\r\n\r\n",
            path, host
        );

        // اتصال مستقیم به ip-api.com
        let stream = TcpStream::connect("ip-api.com:80").await?;
        let mut stream = stream;

        stream.write_all(payload.as_bytes()).await?;

        let mut response = Vec::new();
        let mut buffer = [0; 4096];

        loop {
            match stream.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buffer[..n]),
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response);

        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body = &response_str[body_start + 4..];
            match serde_json::from_str::<IpApiResponse>(body.trim()) {
                Ok(data) => Ok(data),
                Err(e) => {
                    eprintln!("JSON parse error for {}: {}", ip, e);
                    eprintln!("Response body: {}", body);
                    Err(format!("Failed to parse IP info: {}", e).into())
                }
            }
        } else {
            Err("Invalid HTTP response".into())
        }
    }).await {
        Ok(result) => result,
        Err(_) => Err("Timeout getting IP info".into())
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
    let port: u16 = match parts[1].parse() {
        Ok(p) => p,
        Err(_) => return,
    };

    println!("🔍 Testing proxy: {}:{}", ip, port);

    match test_proxy_connection(ip, port).await {
        Ok((true, response_time)) => {
            println!("✔️ Connection successful for {}, getting IP info...", ip);
            
            match get_ip_info(ip).await {
                Ok(ip_info) => {
                    if ip_info.status == "success" {
                        let proxy_info = ProxyInfo {
                            ip: ip.to_string(),
                            isp: ip_info.isp.unwrap_or_else(|| "Unknown ISP".to_string()),
                            asn: ip_info.asn.unwrap_or_else(|| "Unknown ASN".to_string()),
                            city: ip_info.city.unwrap_or_else(|| "Unknown City".to_string()),
                            region: ip_info.region_name.unwrap_or_else(|| "Unknown Region".to_string()),
                            country: ip_info.country.clone().unwrap_or_else(|| "Unknown Country".to_string()),
                            country_code: ip_info.country_code.clone().unwrap_or_else(|| "XX".to_string()),
                            response_time,
                        };

                        let country_key = get_country_name_with_flag(
                            &proxy_info.country_code,
                            &proxy_info.country
                        );

                        println!("🟢 PROXY LIVE: {} ({:.0}ms) - {}", 
                            proxy_info.ip, 
                            response_time,
                            country_key
                        );

                        let mut active_proxies_locked = active_proxies.lock().unwrap();
                        active_proxies_locked
                            .entry(country_key)
                            .or_default()
                            .push(proxy_info);
                    } else {
                        println!("🔴 IP info failed for {}: Invalid status", ip);
                    }
                },
                Err(e) => {
                    println!("🔴 Failed to get IP info for {}: {}", ip, e);
                }
            }
        },
        Ok((false, _)) => {
            println!("❌ PROXY DEAD: {}:{} (Connection failed)", ip, port);
        },
        Err(e) => {
            println!("❌ PROXY ERROR: {}:{} - {}", ip, port, e);
        }
    }
}
