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
// rand حذف شد چون استفاده نمی‌کنیم

const DEFAULT_IP_RESOLVER: &str = "api.ipquery.io";
const DEFAULT_PROXY_FILE: &str = "Data/alive.txt";
const DEFAULT_OUTPUT_FILE: &str = "ProxyIP-Daily.md";
const DEFAULT_MAX_CONCURRENT: usize = 50; // افزایش همزمانی (بدون محدودیت!)
const DEFAULT_TIMEOUT_SECONDS: u64 = 8;
const MAX_RETRIES: usize = 2;
const BULK_SIZE: usize = 100; // bulk query size

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
struct IpQueryLocation {
    country: String,
    country_code: String,
    city: String,
    state: String,
}

#[derive(Deserialize, Debug, Clone)]
struct IpQueryIsp {
    asn: String,
    org: String,
    isp: String,
}

#[derive(Deserialize, Debug, Clone)]
struct IpQueryResponse {
    ip: String,
    location: IpQueryLocation,
    isp: IpQueryIsp,
}

// برای سازگاری با کد قبلی
#[derive(Debug, Clone)]
struct ProxyInfo {
    query: String,
    isp: String,
    org: String,
    asn: String,
    city: String,
    regionName: String,
    country: String,
    countryCode: String,
    proxy_type: String,
}

impl From<IpQueryResponse> for ProxyInfo {
    fn from(resp: IpQueryResponse) -> Self {
        ProxyInfo {
            query: resp.ip,
            isp: resp.isp.isp,
            org: resp.isp.org,
            asn: resp.isp.asn,
            city: resp.location.city,
            regionName: resp.location.state,
            country: resp.location.country,
            countryCode: resp.location.country_code,
            proxy_type: String::new(),
        }
    }
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
    
    // Extract IP addresses for processing
    let ip_list: Vec<String> = proxies.iter()
        .map(|line| line.split(',').next().unwrap_or("").to_string())
        .collect();
        
    println!("Testing connectivity for {} IPs...", ip_list.len());
    
    // Step 1: Check connectivity for all IPs (this is the crucial part!)
    let connectivity_tasks = futures::stream::iter(
        ip_list.iter().map(|ip| {
            let ip = ip.clone();
            async move {
                match check_connection(&ip).await {
                    Ok(ping) => {
                        println!("Connection OK: {} ({} ms)", ip, ping);
                        Some((ip, ping))
                    },
                    Err(_) => {
                        println!("PROXY DEAD 🪧: {} (connection failed)", ip);
                        None
                    }
                }
            }
        })
    )
    .buffer_unordered(args.max_concurrent)
    .collect::<Vec<_>>()
    .await;

    let connected_ips: Vec<(String, u128)> = connectivity_tasks.into_iter().filter_map(|x| x).collect();
    
    if connected_ips.is_empty() {
        println!("😞 No live connections found!");
    } else {
        println!("Found {} live connections! Fetching geo info...", connected_ips.len());
        
        // Step 2: Get geo info for live IPs in bulk chunks
        let chunks: Vec<Vec<(String, u128)>> = connected_ips.chunks(BULK_SIZE).map(|chunk| chunk.to_vec()).collect();
        let total_chunks = chunks.len();
        
        for (chunk_index, chunk) in chunks.into_iter().enumerate() {
            let ips: Vec<&str> = chunk.iter().map(|(ip, _)| ip.as_str()).collect();
            
            println!("Fetching geo info for chunk {}/{} ({} IPs)...", chunk_index + 1, total_chunks, ips.len());
            
            match fetch_bulk_proxy_info(&ips, &args.ip_resolver).await {
                Ok(geo_infos) => {
                    let mut active_proxies_locked = active_proxies.lock().unwrap();
                    for (geo_info, (_, ping)) in geo_infos.into_iter().zip(chunk.iter()) {
                        println!("{}", format!("PROXY LIVE 🟢: {} ({} ms)", geo_info.query, ping).green());
                        active_proxies_locked
                            .entry(geo_info.country.clone())
                            .or_default()
                            .push((geo_info, *ping));
                    }
                }
                Err(e) => {
                    println!("Failed to fetch geo info for chunk {}: {}", chunk_index + 1, e);
                    // Fallback: add IPs without detailed geo info
                    for (ip, ping) in chunk.iter() {
                        println!("PROXY LIVE 🟢: {} ({} ms) [no geo info]", ip, ping);
                    }
                }
            }
            
            // Small delay between chunks
            if chunk_index < total_chunks - 1 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }

    write_markdown_file(&active_proxies.lock().unwrap(), &args.output_file)
        .context("Failed to write Markdown file")?;

    println!("Proxy checking completed.");
    Ok(())
}



async fn fetch_bulk_proxy_info(ips: &[&str], resolver: &str) -> Result<Vec<ProxyInfo>> {
    if ips.is_empty() {
        return Ok(vec![]);
    }
    
    // Create comma-separated IP list
    let ip_list = ips.join(",");
    let url = format!("https://{}/{}", resolver, ip_list);
    
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS * 2)) // longer timeout for bulk
        .build()?;

    let resp = client.get(&url).send().await?; // اضافه کردن .send()
    
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Bulk API request failed with status: {}", resp.status()));
    }
    
    let responses: Vec<IpQueryResponse> = resp.json().await?;
    let proxy_infos: Vec<ProxyInfo> = responses.into_iter().map(|r| r.into()).collect();
    
    Ok(proxy_infos)
}

async fn fetch_proxy_info(ip: &str, resolver: &str) -> Result<ProxyInfo> {
    let url = format!("https://{}/{}", resolver, ip);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS))
        .build()?;

    let resp = client.get(&url).send().await?; // اضافه کردن .send()
    
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("API request failed with status: {}", resp.status()));
    }
    
    let data: IpQueryResponse = resp.json().await?;
    Ok(data.into())
}

// باقی توابع مثل قبل...
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
