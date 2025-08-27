use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use colored::*;
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
    "Google",
    "Amazon",
    "Cloudflare",
    "Hetzner",
    "DigitalOcean",
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
    "The constant company",
    "G-Core",
    "IONOS",
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
            let isp_name = parts[3].to_string();
            let isp_ok = GOOD_ISPS.iter().any(|kw| isp_name.contains(kw));
            port_ok && isp_ok
        })
        .collect();
    println!("Filtered to {} good proxies (port 443 + ISP whitelist)", proxies.len());

    let active_proxies = Arc::new(Mutex::new(HashMap::<String, Vec<(ProxyInfo, u128)>>::new()));

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

fn write_markdown_file(proxies_by_country: &HashMap<String, Vec<(ProxyInfo, u128)>>) -> io::Result<()> {
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

            let flag = country_flag(&proxies[0].0.countryCode);

            writeln!(file, "## {} {}\n", flag, country_name)?;
            writeln!(file, "| IP | Location | ISP | Ping |")?;
            writeln!(file, "|----|----------|-------|----|")?;

            for (info, ping) in proxies {
                let location = format!("{}, {}", info.regionName, info.city);
                writeln!(
                    file,
                    "| `{}` | {} | {} | {} ms |",
                    info.query,
                    location,
                    info.isp,
                    ping
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

async fn check_connection(host: &str, ip: &str) -> Result<u128> {
    let timeout_duration = Duration::from_secs(TIMEOUT_SECONDS);
    let start = Instant::now();

    match tokio::time::timeout(timeout_duration, TcpStream::connect(format!("{}:443", ip))).await {
        Ok(Ok(_stream)) => {
            let elapsed = start.elapsed().as_millis();
            Ok(elapsed)
        }
        Ok(Err(e)) => Err(Box::new(e)),
        Err(_) => Err("Connection attempt timed out".into()),
    }
}

async fn fetch_proxy_info(ip: &str) -> Result<ProxyInfo> {
    let path = format!("/json/{}", ip);
    let url = format!("http://{}{}", IP_RESOLVER, path);

    let resp = reqwest::get(&url).await?;
    let data = resp.json::<ProxyInfo>().await?;
    Ok(data)
}

async fn process_proxy(
    proxy_line: String,
    active_proxies: &Arc<Mutex<HashMap<String, Vec<(ProxyInfo, u128)>>>>,
) {
    let parts: Vec<&str> = proxy_line.split(',').collect();
    if parts.len() < 2 {
        return;
    }
    let ip = parts[0];

    match tokio::try_join!(check_connection(IP_RESOLVER, ip), fetch_proxy_info(ip)) {
        Ok((ping, info)) => {
            println!("{}", format!("PROXY LIVE 🟢: {} ({} ms)", info.query, ping).green());
            let mut active_proxies_locked = active_proxies.lock().unwrap();
            active_proxies_locked
                .entry(info.country.clone())
                .or_default()
                .push((info, ping));
        }
        Err(e) => {
            println!("PROXY DEAD 🦖: {} ({})", ip, e);
        }
    }
}
