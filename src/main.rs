use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use chrono::{Duration as ChronoDuration, Utc};
use colored::*;
use futures::StreamExt;

const DEFAULT_IP_RESOLVER: &str = "ip-api.com";
const DEFAULT_PROXY_FILE: &str = "Data/alive.txt";
const DEFAULT_OUTPUT_FILE: &str = "active_proxies.md";
const DEFAULT_MAX_CONCURRENT: usize = 30;
const DEFAULT_TIMEOUT_SECONDS: u64 = 8;
const REQUEST_DELAY_MS: u64 = 500;

const GOOD_ISPS: &[&str] = &[
    "Google",
    "Amazon",
    "Cloudflare",
    "Rackspace",
    "M247",
    "DataCamp",
    "Total Uptime",
    "gmbh",
    "Akamai",
    "Tencent",
    "The Empire",
    "OVH",
    "ByteDance",
    "Starlink",
    "3NT SOLUTION",
    "WorkTitans B.V.",
    "PQ Hosting",
    "Multacom Corporation",
    "The Constant Company",
    "G-Core",
    "IONOS",
    "Stark Industries",
];

#[derive(Parser, Clone)]
#[command(name = "Proxy Checker")]
#[command(about = "Checks proxies and outputs active ones")]
struct Args {
    /// Path to the proxy file
    #[arg(short, long, default_value = DEFAULT_PROXY_FILE)]
    proxy_file: String,

    /// Output file path
    #[arg(short, long, default_value = DEFAULT_OUTPUT_FILE)]
    output_file: String,

    /// IP resolver URL
    #[arg(long, default_value = DEFAULT_IP_RESOLVER)]
    ip_resolver: String,

    /// Max concurrent tasks
    #[arg(long, default_value_t = DEFAULT_MAX_CONCURRENT)]
    max_concurrent: usize,

    /// Timeout in seconds
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
    timeout: u64,
}

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
    #[serde(default)]
    proxy_type: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(parent) = Path::new(&args.output_file).parent() {
        fs::create_dir_all(parent).context("Failed to create output directory")?;
    }
    File::create(&args.output_file).context("Failed to create output file")?;

    let proxies = read_proxy_file(&args.proxy_file)
        .context("Failed to read proxy file")?;
    println!("Loaded {} proxies from file", proxies.len());

    let proxies: Vec<String> = proxies
        .into_iter()
        .filter(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() < 4 {
                return false;
            }
            let port_ok = parts[1].trim() == "443";
            let isp_name = parts[3].to_string();
            let isp_ok = GOOD_ISPS.iter().any(|kw| isp_name.contains(kw));
            port_ok && isp_ok
        })
        .collect();
    println!("Filtered to {} good proxies (port 443 + ISP whitelist)", proxies.len());

    let active_proxies = Arc::new(Mutex::new(BTreeMap::<String, Vec<(ProxyInfo, u128)>>::new()));

    let tasks = futures::stream::iter(
        proxies.into_iter().map(|proxy_line| {
            let active_proxies = Arc::clone(&active_proxies);
            let args = args.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(REQUEST_DELAY_MS)).await;
                process_proxy(proxy_line, &active_proxies, &args).await;
            }
        })
    )
    .buffer_unordered(args.max_concurrent)
    .collect::<Vec<()>>();

    tasks.await;

    write_markdown_file(&active_proxies.lock().unwrap(), &args.output_file)
        .context("Failed to write Markdown file")?;

    println!("Proxy checking completed.");
    Ok(())
}

fn write_markdown_file(proxies_by_country: &BTreeMap<String, Vec<(ProxyInfo, u128)>>, output_file: &str) -> io::Result<()> {
    let mut file = File::create(output_file)?;

    let total_active = proxies_by_country.values().map(|v| v.len()).sum::<usize>();
    let total_countries = proxies_by_country.len();
    let avg_ping = if total_active > 0 {
        let sum_ping: u128 = proxies_by_country.values().flatten().map(|(_, p)| *p).sum();
        sum_ping / total_active as u128
    } else {
        0
    };

    let now = Utc::now();
    // بر اساس مثال شما (از جمعه تا یکشنبه)، فاصله آپدیت ۲ روز در نظر گرفته شده است
    let next_update = now + ChronoDuration::days(2); 
    let last_updated_str = now.format("%a, %d %b %Y %H:%M:%S").to_string();
    let next_update_str = next_update.format("%a, %d %b %Y %H:%M:%S").to_string();

    // نوشتن هِدِر سفارشی شما در فایل
    // برای جلوگیری از خطا در کاراکترهای خاص لاتک، از دابل براکت {{}} استفاده شده است
    writeln!(
        file,
        r##"<p align="left">
 <img src="https://latex.codecogs.com/svg.image?\huge&space;{{\color{{Golden}}\mathrm{{PR{{\color{{black}}\O}}XY\;IP}}" width=200px" </p><br/>

> [!WARNING]
>
> **Daily Fresh Proxies**
>
> Only **high-quality**, tested proxies from **top ISPs** and data centers worldwide such as Google, Cloudflare, Amazon, Tencent, OVH, DataCamp.
>
> <Br/>
>
> **Automatically updated every day**
>
> **Last updated:** {} <br/>
> **Next update:** {}
>
> <br/>
> 
> **Summary**
> 
> **Total Active Proxies:** {} <br/>
> **Countries Covered:** {} <br/> 
> **Average Ping:** {} ms
>
> <br/>

</br>

        "##,
        last_updated_str,
        next_update_str,
        total_active,
        total_countries,
        avg_ping
    )?;

    if proxies_by_country.is_empty() {
        writeln!(file, "\n😞 No active proxies found.")?;
        println!("No active proxies found");
        return Ok(());
    }
    
    // نوشتن لیست پروکسی‌ها به تفکیک کشور (ادامه منطق قبلی برنامه)
    for (country_name, proxies) in proxies_by_country {
        if proxies.is_empty() {
            continue;
        }

        let flag = country_flag(&proxies[0].0.countryCode);
        writeln!(file, "## {} {} ({} proxies)", flag, country_name, proxies.len())?;
        writeln!(file, "<details open>")?;
        writeln!(file, "<summary>Click to collapse</summary>\n")?;
        writeln!(file, "| IP             | Location                   | ISP        | Ping       |")?;
        writeln!(file, "| -------------- | -------------------------- | ---------- | ---------- |")?;

        for (info, ping) in proxies {
            let location = format!("{}, {}", info.regionName, info.city);
            let emoji = if *ping < 100 { "⚡" } else if *ping < 500 { "🐌" } else { "🦥" };
            writeln!(
                file,
                "| `{}` | {} | {} | {} ms {} |",
                info.query,
                location,
                info.isp,
                ping,
                emoji
            )?;
        }
        writeln!(file, "\n</details>\n\n---\n")?;
    }

    println!("All active proxies saved to {}", output_file);
    Ok(())
}

fn country_flag(code: &str) -> String {
    code.chars()
        .filter_map(|c| {
            if c.is_ascii_alphabetic() {
                Some(char::from_u32(0x1F1E6 + (c.to_ascii_uppercase() as u32 - 'A' as u32)).unwrap())
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

async fn check_connection(proxy_ip: &str) -> Result<u128> {
    let start = Instant::now();
    match tokio::time::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS), tokio::net::TcpStream::connect(format!("{}:443", proxy_ip))).await {
        Ok(Ok(_stream)) => {
            let elapsed = start.elapsed().as_millis();
            Ok(elapsed)
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("Connection failed: {}", e)),
        Err(_) => Err(anyhow::anyhow!("Connection timed out")),
    }
}

async fn fetch_proxy_info(ip: &str, resolver: &str) -> Result<ProxyInfo> {
    let path = format!("/json/{}", ip);
    let url = format!("http://{}{}", resolver, path);

    let resp = reqwest::get(&url).await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("API request failed with status: {}", resp.status()));
    }
    let data = resp.json::<ProxyInfo>().await?;
    Ok(data)
}

async fn process_proxy(
    proxy_line: String,
    active_proxies: &Arc<Mutex<BTreeMap<String, Vec<(ProxyInfo, u128)>>>>,
    args: &Args,
) {
    let parts: Vec<&str> = proxy_line.split(',').collect();
    if parts.len() < 2 {
        return;
    }
    let ip = parts[0];

    match tokio::try_join!(check_connection(ip), fetch_proxy_info(ip, &args.ip_resolver)) {
        Ok((ping, info)) => {
            println!("{}", format!("PROXY LIVE 🟢: {} ({} ms)", info.query, ping).green());
            let mut active_proxies_locked = active_proxies.lock().unwrap();
            active_proxies_locked
                .entry(info.country.clone())
                .or_default()
                .push((info, ping));
        }
        Err(e) => {
            println!("PROXY DEAD 🪧: {} ({})", ip, e);
        }
    }
}
