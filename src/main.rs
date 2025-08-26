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

const PROXY_FILE: &str = "Data/test.txt";
const OUTPUT_FILE: &str = "active_proxies.md";
const MAX_CONCURRENT: usize = 50; // کاهش تعداد درخواست‌های همزمان
const TIMEOUT_SECONDS: u64 = 12; // افزایش timeout

// نقشه کشورها برای تبدیل کد دو حرفی به نام کامل
fn get_country_name(country_code: &str) -> String {
    let countries = [
        ("US", "🇺🇸 United States"),
        ("GB", "🇬🇧 United Kingdom"),
        ("DE", "🇩🇪 Germany"),
        ("FR", "🇫🇷 France"),
        ("CA", "🇨🇦 Canada"),
        ("AU", "🇦🇺 Australia"),
        ("JP", "🇯🇵 Japan"),
        ("CN", "🇨🇳 China"),
        ("IN", "🇮🇳 India"),
        ("BR", "🇧🇷 Brazil"),
        ("RU", "🇷🇺 Russia"),
        ("IT", "🇮🇹 Italy"),
        ("ES", "🇪🇸 Spain"),
        ("NL", "🇳🇱 Netherlands"),
        ("KR", "🇰🇷 South Korea"),
        ("TR", "🇹🇷 Turkey"),
        ("MX", "🇲🇽 Mexico"),
        ("AR", "🇦🇷 Argentina"),
        ("PL", "🇵🇱 Poland"),
        ("SE", "🇸🇪 Sweden"),
        ("NO", "🇳🇴 Norway"),
        ("DK", "🇩🇰 Denmark"),
        ("FI", "🇫🇮 Finland"),
        ("CH", "🇨🇭 Switzerland"),
        ("AT", "🇦🇹 Austria"),
        ("BE", "🇧🇪 Belgium"),
        ("PT", "🇵🇹 Portugal"),
        ("GR", "🇬🇷 Greece"),
        ("CZ", "🇨🇿 Czech Republic"),
        ("HU", "🇭🇺 Hungary"),
        ("RO", "🇷🇴 Romania"),
        ("BG", "🇧🇬 Bulgaria"),
        ("HR", "🇭🇷 Croatia"),
        ("SK", "🇸🇰 Slovakia"),
        ("SI", "🇸🇮 Slovenia"),
        ("LT", "🇱🇹 Lithuania"),
        ("LV", "🇱🇻 Latvia"),
        ("EE", "🇪🇪 Estonia"),
        ("IE", "🇮🇪 Ireland"),
        ("IS", "🇮🇸 Iceland"),
        ("LU", "🇱🇺 Luxembourg"),
        ("MT", "🇲🇹 Malta"),
        ("CY", "🇨🇾 Cyprus"),
        ("UA", "🇺🇦 Ukraine"),
        ("BY", "🇧🇾 Belarus"),
        ("MD", "🇲🇩 Moldova"),
        ("IL", "🇮🇱 Israel"),
        ("SA", "🇸🇦 Saudi Arabia"),
        ("AE", "🇦🇪 United Arab Emirates"),
        ("EG", "🇪🇬 Egypt"),
        ("ZA", "🇿🇦 South Africa"),
        ("NG", "🇳🇬 Nigeria"),
        ("KE", "🇰🇪 Kenya"),
        ("MA", "🇲🇦 Morocco"),
        ("TH", "🇹🇭 Thailand"),
        ("SG", "🇸🇬 Singapore"),
        ("MY", "🇲🇾 Malaysia"),
        ("ID", "🇮🇩 Indonesia"),
        ("PH", "🇵🇭 Philippines"),
        ("VN", "🇻🇳 Vietnam"),
        ("BD", "🇧🇩 Bangladesh"),
        ("PK", "🇵🇰 Pakistan"),
        ("LK", "🇱🇰 Sri Lanka"),
        ("NZ", "🇳🇿 New Zealand"),
        ("CL", "🇨🇱 Chile"),
        ("PE", "🇵🇪 Peru"),
        ("CO", "🇨🇴 Colombia"),
        ("VE", "🇻🇪 Venezuela"),
        ("EC", "🇪🇨 Ecuador"),
        ("UY", "🇺🇾 Uruguay"),
        ("PY", "🇵🇾 Paraguay"),
        ("BO", "🇧🇴 Bolivia"),
        ("CR", "🇨🇷 Costa Rica"),
        ("PA", "🇵🇦 Panama"),
        ("GT", "🇬🇹 Guatemala"),
        ("SV", "🇸🇻 El Salvador"),
        ("HN", "🇭🇳 Honduras"),
        ("NI", "🇳🇮 Nicaragua"),
        ("DO", "🇩🇴 Dominican Republic"),
        ("CU", "🇨🇺 Cuba"),
        ("JM", "🇯🇲 Jamaica"),
        ("TT", "🇹🇹 Trinidad and Tobago"),
        ("BB", "🇧🇧 Barbados"),
        ("IR", "🇮🇷 Iran"),
        ("IQ", "🇮🇶 Iraq"),
        ("SY", "🇸🇾 Syria"),
        ("LB", "🇱🇧 Lebanon"),
        ("JO", "🇯🇴 Jordan"),
        ("KW", "🇰🇼 Kuwait"),
        ("QA", "🇶🇦 Qatar"),
        ("BH", "🇧🇭 Bahrain"),
        ("OM", "🇴🇲 Oman"),
        ("YE", "🇾🇪 Yemen"),
    ];
    
    countries.iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(country_code))
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| format!("🌍 {}", country_code))
}

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
    println!("🔎 Starting proxy scanner...");

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
    writeln!(file, "Generated at: {}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs())?;
    writeln!(file, "")?;
    
    if proxies_by_country.is_empty() {
        writeln!(file, "## ❌ No Active Proxies Found")?;
        writeln!(file, "")?;
        writeln!(file, "No working proxies were detected during the scan.")?;
        println!("❌ No active proxies found");
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

    for (country_code, proxies) in countries {
        let country_name = get_country_name(country_code);
        
        writeln!(file, "## {} ({} proxies)", country_name, proxies.len())?;
        writeln!(file, "")?;
        
        // مرتب‌سازی پروکسی‌ها براساس تاخیر
        let mut sorted_proxies = proxies.clone();
        sorted_proxies.sort_by(|a, b| {
            let delay_a = a.delay.trim_end_matches("ms").parse::<f32>().unwrap_or(9999.0);
            let delay_b = b.delay.trim_end_matches("ms").parse::<f32>().unwrap_or(9999.0);
            delay_a.partial_cmp(&delay_b).unwrap_or(std::cmp::Ordering::Equal)
        });

        for (index, info) in sorted_proxies.iter().enumerate() {
            writeln!(file, "### 🧬 Proxy #{}", index + 1)?;
            writeln!(file, "")?;
            writeln!(file, "| Field | Value |")?;
            writeln!(file, "|-------|-------|")?;
            writeln!(file, "| **IP Address** | `{}` |", info.ip)?;
            
            // نمایش لوکیشن کامل
            let location = format!("{} {}, {}", info.countryflag, info.city, info.region);
            writeln!(file, "| **Location** | {} |", location)?;
            writeln!(file, "| **ISP** | {} |", info.isp)?;
            writeln!(file, "| **ASN** | {} |", info.asn)?;
            
            // تشخیص سرعت براساس پینگ
            let delay_num = info.delay.trim_end_matches("ms").parse::<f32>().unwrap_or(9999.0);
            let speed_emoji = if delay_num < 50.0 { "🚀" }
                else if delay_num < 150.0 { "⚡" }
                else if delay_num < 300.0 { "🐌" }
                else { "🦆" };
            
            writeln!(file, "| **Ping** | {} {} |", speed_emoji, info.delay)?;
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
    writeln!(file, "- Speed indicators: 🚀 Fast (<50ms) | ⚡ Good (<150ms) | 🐌 Slow (<300ms) | 🦆 Very Slow (>300ms)")?;
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

async fn check_connection(
    host: &str,
    path: &str,
    proxy: Option<(&str, u16)>,
) -> Result<Value> {
    let timeout_duration = Duration::from_secs(TIMEOUT_SECONDS);

    // بسته‌بندی کل عملیات در timeout
    match tokio::time::timeout(timeout_duration, async {
        // ساخت payload درخواست HTTP
        let payload = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36\r\n\
             Accept: application/json, text/plain, */*\r\n\
             Accept-Language: en-US,en;q=0.9\r\n\
             Accept-Encoding: gzip, deflate, br\r\n\
             Connection: close\r\n\r\n",
            path, host
        );

        // ایجاد اتصال TCP
        let stream = if let Some((proxy_ip, proxy_port)) = proxy {
            let connect_addr = if proxy_ip.contains(':') {
                format!("[{}]:{}", proxy_ip, proxy_port)
            } else {
                format!("{}:{}", proxy_ip, proxy_port)
            };
            TcpStream::connect(connect_addr).await?
        } else {
            TcpStream::connect(format!("{}:443", host)).await?
        };

        // ایجاد اتصال TLS
        let native_connector = NativeTlsConnector::builder()
            .danger_accept_invalid_certs(true) // قبول گواهی‌های نامعتبر
            .build()?;
        let tokio_connector = TokioTlsConnector::from(native_connector);

        let mut tls_stream = tokio_connector.connect(host, stream).await?;

        // ارسال درخواست HTTP
        tls_stream.write_all(payload.as_bytes()).await?;

        // خواندن پاسخ
        let mut response = Vec::new();
        let mut buffer = [0; 8192]; // افزایش سایز بافر

        loop {
            match tls_stream.read(&mut buffer).await {
                Ok(0) => break, // پایان stream
                Ok(n) => response.extend_from_slice(&buffer[..n]),
                Err(e) => {
                    return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                }
            }
        }

        // پردازش پاسخ
        let response_str = String::from_utf8_lossy(&response);

        // جداسازی header و body
        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body = &response_str[body_start + 4..];

            // تلاش برای parse کردن JSON
            match serde_json::from_str::<Value>(body.trim()) {
                Ok(json_data) => Ok(json_data),
                Err(_) => {
                    // اگر JSON نیست، تلاش برای استخراج IP از متن
                    if let Some(ip_match) = extract_ip_from_text(body) {
                        let json_obj = serde_json::json!({
                            "ip": ip_match,
                            "delay": "N/A",
                            "isp": "Unknown ISP",
                            "asn": "Unknown ASN",
                            "city": "Unknown City",
                            "region": "Unknown Region",
                            "country": "Unknown",
                            "countryflag": "🌍"
                        });
                        Ok(json_obj)
                    } else {
                        Err("Unable to parse response".into())
                    }
                }
            }
        } else {
            Err("Invalid HTTP response format".into())
        }
    }).await {
        Ok(inner_result) => inner_result,
        Err(_) => Err(Box::new(io::Error::new(io::ErrorKind::TimedOut, "Connection timed out")) as Box<dyn std::error::Error + Send + Sync>),
    }
}

fn extract_ip_from_text(text: &str) -> Option<String> {
    // RegEx ساده برای استخراج IP
    let ip_pattern = regex::Regex::new(r"\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\b").ok()?;
    ip_pattern.find(text).map(|m| m.as_str().to_string())
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

    // لیست سرویس‌های مختلف برای تست
    let services = [
        ("httpbin.org", "/ip"),
        ("api.ipify.org", "/?format=json"),
        ("ipinfo.io", "/json"),
        ("icanhazip.com", "/"),
    ];

    for (host, path) in &services {
        match check_connection(host, path, None).await {
            Ok(proxy_data) => {
                // تلاش برای deserialize کردن به ProxyInfo
                let info = match serde_json::from_value::<ProxyInfo>(proxy_data.clone()) {
                    Ok(info) => info,
                    Err(_) => {
                        // اگر format استاندارد نبود، یک ProxyInfo ساده بسازیم
                        ProxyInfo {
                            ip: ip.to_string(),
                            delay: "N/A".to_string(),
                            isp: proxy_data.get("org").and_then(|v| v.as_str()).unwrap_or("Unknown ISP").to_string(),
                            asn: proxy_data.get("asn").and_then(|v| v.as_str()).unwrap_or("Unknown ASN").to_string(),
                            city: proxy_data.get("city").and_then(|v| v.as_str()).unwrap_or("Unknown City").to_string(),
                            region: proxy_data.get("region").and_then(|v| v.as_str()).unwrap_or("Unknown Region").to_string(),
                            country: proxy_data.get("country").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string(),
                            countryflag: proxy_data.get("country").and_then(|v| v.as_str()).map(|code| {
                                get_country_name(code).chars().take(2).collect()
                            }).unwrap_or_else(|| "🌍".to_string()),
                        }
                    }
                };

                println!("🟢 PROXY LIVE: {} (Service: {})", info.ip, host);
                let mut active_proxies_locked = active_proxies.lock().unwrap();

                active_proxies_locked
                    .entry(info.country.clone())
                    .or_default()
                    .push(info);
                return; // موفق بود، نیازی به تست سرویس‌های دیگر نیست
            },
            Err(_) => {
                continue; // این سرویس کار نکرد، سرویس بعدی را امتحان کن
            }
        }
    }

    // اگر هیچ سرویسی کار نکرد
    println!("🔴 PROXY DEAD: {}", full_proxy_address);
}
