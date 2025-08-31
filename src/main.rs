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
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector;
use native_tls::TlsConnector as NativeTlsConnector;

const DEFAULT_PROXY_FILE: &str = "Data/alive.txt";
const DEFAULT_OUTPUT_FILE: &str = "ProxyIP-Daily.md";
const DEFAULT_MAX_CONCURRENT: usize = 20;
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
    "OVH",
    "ByteDance",
    "Starlink",
    "PQ Hosting",
    "Multacom",
    "The Constant Company",
    "G-Core",
    "IONOS",
];

#[derive(Parser, Clone)]
#[command(name = "Proxy Checker")]
#[command(about = "Checks proxies via Cloudflare /meta and outputs active ones")]
struct Args {
    #[arg(short, long, default_value = DEFAULT_PROXY_FILE)]
    proxy_file: String,

    #[arg(short, long, default_value = DEFAULT_OUTPUT_FILE)]
    output_file: String,

    #[arg(long, default_value_t = DEFAULT_MAX_CONCURRENT)]
    max_concurrent: usize,

    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
    timeout: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct CfMeta {
    clientIp: String,
    asOrganization: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
}

#[derive(Debug, Clone)]
struct ProxyInfo {
    ip: String,
    isp: String,
    country: String,
    region: String,
    city: String,
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

    // خودت (بدون پروکسی) برای مقایسه
    let self_meta = fetch_cf_meta(None).await?;
    println!("Your real IP: {}", self_meta.clientIp);

    let active_proxies = Arc::new(Mutex::new(BTreeMap::<String, Vec<(ProxyInfo, u128)>>::new()));

    let tasks = futures::stream::iter(
        proxies.into_iter().map(|proxy_line| {
            let active_proxies = Arc::clone(&active_proxies);
            let args = args.clone();
            let self_ip = self_meta.clientIp.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(REQUEST_DELAY_MS)).await;
                process_proxy(proxy_line, &active_proxies, &args, &self_ip).await;
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

async fn fetch_cf_meta(proxy: Option<(String, u16)>) -> Result<CfMeta> {
    let host = "speed.cloudflare.com";
    let path = "/meta";
    let payload = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: RustProxyChecker\r\nConnection: close\r\n\r\n",
        path, host
    );

    let (ip, port) = proxy.unwrap_or((host.to_string(), 443));

    let stream = tokio::time::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS), TcpStream::connect((ip.as_str(), port))).await??;

    let tls = TlsConnector::from(NativeTlsConnector::new()?);
    let mut stream = tls.connect(host, stream).await?;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(payload.as_bytes()).await?;

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await?;

    let text = String::from_utf8_lossy(&resp);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let meta: CfMeta = serde_json::from_str(body)?;
    Ok(meta)
}

async fn process_proxy(
    proxy_line: String,
    active_proxies: &Arc<Mutex<BTreeMap<String, Vec<(ProxyInfo, u128)>>>>,
    args: &Args,
    self_ip: &str,
) {
    let parts: Vec<&str> = proxy_line.split(',').collect();
    if parts.len() < 2 {
        return;
    }
    let ip = parts[0];
    let port = parts[1].parse::<u16>().unwrap_or(443);

    let start = Instant::now();
    match fetch_cf_meta(Some((ip.to_string(), port))).await {
        Ok(meta) => {
            if meta.clientIp != self_ip {
                let ping = start.elapsed().as_millis();
                let info = ProxyInfo {
                    ip: meta.clientIp.clone(),
                    isp: meta.asOrganization.unwrap_or_else(|| "Unknown".to_string()),
                    country: meta.country.unwrap_or_else(|| "Unknown".to_string()),
                    region: meta.region.unwrap_or_else(|| "Unknown".to_string()),
                    city: meta.city.unwrap_or_else(|| "Unknown".to_string()),
                };
                println!("{}", format!("PROXY LIVE 🟢: {} ({} ms)", ip, ping).green());
                let mut active_proxies_locked = active_proxies.lock().unwrap();
                active_proxies_locked
                    .entry(info.country.clone())
                    .or_default()
                    .push((info, ping));
            } else {
                println!("PROXY DEAD 🪧: {} (did not change IP)", ip);
            }
        }
        Err(e) => {
            println!("PROXY DEAD 🪧: {} ({})", ip, e);
        }
    }
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
    let next_update = now + ChronoDuration::days(2); 
    let last_updated_str = now.format("%a, %d %b %Y %H:%M:%S").to_string();
    let next_update_str = next_update.format("%a, %d %b %Y %H:%M:%S").to_string();

    writeln!(
        file,
        r##"
# Daily Proxy Report

**Last updated:** {}  
**Next update:** {}  

- **Total Active Proxies:** {}  
- **Countries Covered:** {}  
- **Average Ping:** {} ms  

---
        "##,
        last_updated_str,
        next_update_str,
        total_active,
        total_countries,
        avg_ping
    )?;

    for (country, proxies) in proxies_by_country {
        writeln!(file, "## {} ({} proxies)", country, proxies.len())?;
        writeln!(file, "| IP | ISP | Location | Ping |")?;
        writeln!(file, "|----|-----|----------|------|")?;

        for (info, ping) in proxies {
            let location = format!("{}, {}", info.region, info.city);
            writeln!(
                file,
                "| `{}` | {} | {} | {} ms |",
                info.ip, info.isp, location, ping
            )?;
        }
        writeln!(file, "\n---\n")?;
    }

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
