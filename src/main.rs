use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use reqwest;
use serde_json::Value;

const PROXY_FILE: &str = "Data/test.txt";
const OUTPUT_FILE: &str = "active_proxies.md";
const MAX_CONCURRENT: usize = 50; // کاهش تعداد درخواست‌های همزمان
const TIMEOUT_SECONDS: u64 = 10; // افزایش timeout

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
    delay: Option<String>,
    isp: Option<String>,
    asn: Option<String>,
    city: Option<String>,
    region: Option<String>,
    country: Option<String>,
    countryflag: Option<String>,
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

    println!("✅ Proxy checking completed.");
    Ok(())
}

fn write_markdown_file(proxies_by_country: &HashMap<String, Vec<ProxyInfo>>) -> io::Result<()> {
    let mut file = File::create(OUTPUT_FILE)?;
    
    // نوشتن هدر فایل
    writeln!(file, "# 🌐 Active Proxies Report")?;
    writeln!(file, "")?;
    writeln!(file, "Generated at: {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"))?;
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
        
        writeln!(file, "## {} ({})", country_name, proxies.len())?;
        writeln!(file, "")?;
        
        // مرتب‌سازی پروکسی‌ها براساس IP
        let mut sorted_proxies = proxies.clone();
        sorted_proxies.sort_by(|a, b| {
            // مرتب‌سازی براساس تاخیر اگر موجود باشد، سپس IP
            match (&a.delay, &b.delay) {
                (Some(delay_a), Some(delay_b)) => {
                    let delay_a_num = delay_a.trim_end_matches("ms").parse::<f32>().unwrap_or(9999.0);
                    let delay_b_num = delay_b.trim_end_matches("ms").parse::<f32>().unwrap_or(9999.0);
                    delay_a_num.partial_cmp(&delay_b_num).unwrap_or(std::cmp::Ordering::Equal)
                },
                _ => a.ip.cmp(&b.ip)
            }
        });

        for (index, info) in sorted_proxies.iter().enumerate() {
            writeln!(file, "### 🧬 Proxy #{}", index + 1)?;
            writeln!(file, "")?;
            writeln!(file, "| Field | Value |")?;
            writeln!(file, "|-------|-------|")?;
            writeln!(file, "| **IP Address** | `{}` |", info.ip)?;
            
            // نمایش لوکیشن کامل
            let location = format!("{} {} {}", 
                info.countryflag.as_deref().unwrap_or("🌍"),
                info.city.as_deref().unwrap_or("Unknown City"),
                info.region.as_deref().unwrap_or("Unknown Region")
            );
            writeln!(file, "| **Location** | {} |", location)?;
            
            if let Some(isp) = &info.isp {
                writeln!(file, "| **ISP** | {} |", isp)?;
            }
            
            if let Some(asn) = &info.asn {
                writeln!(file, "| **ASN** | {} |", asn)?;
            }
            
            if let Some(delay) = &info.delay {
                // تشخیص سرعت براساس پینگ
                let speed_emoji = if let Ok(ping_num) = delay.trim_end_matches("ms").parse::<f32>() {
                    if ping_num < 50.0 { "🚀" }
                    else if ping_num < 150.0 { "⚡" }
                    else if ping_num < 300.0 { "🐌" }
                    else { "🦆" }
                } else { "❓" };
                
                writeln!(file, "| **Ping** | {} {} |", speed_emoji, delay)?;
            }
            
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

async fn check_proxy_with_multiple_services(proxy_ip: &str) -> Option<ProxyInfo> {
    // لیست سرویس‌های مختلف برای تست
    let services = [
        "https://httpbin.org/ip",
        "https://api.ipify.org?format=json",
        "https://ipinfo.io/json",
        "https://api.myip.com",
    ];
    
    // ایجاد کلاینت reqwest با پروکسی
    let proxy_url = format!("http://{}:443", proxy_ip);
    
    let client = match reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(&proxy_url).ok()?)
        .timeout(Duration::from_secs(TIMEOUT_SECONDS))
        .build()
    {
        Ok(client) => client,
        Err(_) => return None,
    };

    // تست با سرویس‌های مختلف
    for service_url in &services {
        match client.get(*service_url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    if let Ok(json) = response.json::<Value>().await {
                        // پردازش پاسخ و ایجاد ProxyInfo
                        return Some(create_proxy_info_from_response(proxy_ip, &json));
                    }
                }
            },
            Err(_) => continue, // امتحان سرویس بعدی
        }
    }
    
    None
}

fn create_proxy_info_from_response(proxy_ip: &str, json_data: &Value) -> ProxyInfo {
    ProxyInfo {
        ip: proxy_ip.to_string(),
        delay: Some("N/A".to_string()), // پینگ دقیق نیاز به تست جداگانه دارد
        isp: json_data.get("org").and_then(|v| v.as_str()).map(|s| s.to_string()),
        asn: json_data.get("asn").and_then(|v| v.as_str()).map(|s| s.to_string()),
        city: json_data.get("city").and_then(|v| v.as_str()).map(|s| s.to_string()),
        region: json_data.get("region").and_then(|v| v.as_str()).map(|s| s.to_string()),
        country: json_data.get("country").and_then(|v| v.as_str()).map(|s| s.to_string()),
        countryflag: json_data.get("country").and_then(|v| v.as_str()).map(|code| {
            get_country_name(code).chars().take(2).collect()
        }),
    }
}

async fn test_proxy_ping(proxy_ip: &str) -> Option<String> {
    let start = std::time::Instant::now();
    
    let proxy_url = format!("http://{}:443", proxy_ip);
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(&proxy_url).ok()?)
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    match client.get("https://httpbin.org/ip").send().await {
        Ok(_) => {
            let duration = start.elapsed();
            Some(format!("{}ms", duration.as_millis()))
        },
        Err(_) => None,
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
    let _port = parts[1]; // پورت همیشه 443 است

    println!("🔍 Testing proxy: {}", ip);

    // تست پروکسی با سرویس‌های مختلف
    if let Some(mut proxy_info) = check_proxy_with_multiple_services(ip).await {
        // تست پینگ جداگانه
        if let Some(ping) = test_proxy_ping(ip).await {
            proxy_info.delay = Some(ping);
        }

        println!("🟢 PROXY LIVE: {} (Ping: {})", 
            proxy_info.ip, 
            proxy_info.delay.as_deref().unwrap_or("N/A")
        );

        let mut active_proxies_locked = active_proxies.lock().unwrap();
        let country_code = proxy_info.country.clone().unwrap_or_else(|| "Unknown".to_string());

        active_proxies_locked
            .entry(country_code)
            .or_default()
            .push(proxy_info);
    } else {
        println!("🔴 PROXY DEAD: {}", ip);
    }
}
