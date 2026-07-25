use anyhow::{Context, Result};
use clap::Parser;
use std::collections::{HashMap, BTreeMap};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use chrono::Utc;
use chrono_tz::Asia::Tehran;

const DEFAULT_PROXY_FILE: &str = "Data/alive.txt";
const DEFAULT_OUTPUT_DIR: &str = "country_proxies/";

#[derive(Parser)]
#[command(name = "Proxy Organizer")]
#[command(about = "Organizes proxies by country and generates output files")]
struct Args {
    #[arg(short, long, default_value = DEFAULT_PROXY_FILE)]
    input_file: String,

    #[arg(short, long, default_value = DEFAULT_OUTPUT_DIR)]
    output_dir: String,
}

#[derive(Debug, Clone)]
struct ProxyEntry {
    ip: String,
    port: String,
    country: String,
    isp: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(parent) = Path::new(&args.output_dir).parent() {
        fs::create_dir_all(parent).context("Failed to create parent directory")?;
    }
    fs::create_dir_all(&args.output_dir).context("Failed to create output directory")?;

    let mut proxies = read_proxy_file(&args.input_file)
    .context("Failed to read proxy file")?;
    
    println!("Loaded {} proxies from file", proxies.len());

    let csv_proxies = fetch_csv_proxies("https://raw.githubusercontent.com/xgonce/Cloudflare_IP/refs/heads/main/result.csv")
        .await
        .context("Failed to fetch and parse CSV proxy file")?;
    
    println!("Loaded {} proxies from CSV source", csv_proxies.len());
    
    proxies.extend(csv_proxies);

    let mut country_groups: BTreeMap<String, Vec<ProxyEntry>> = BTreeMap::new();
    
    for proxy in &proxies {
        country_groups
            .entry(proxy.country.clone())
            .or_default()
            .push(proxy.clone());
    }

    println!("Found proxies from {} countries", country_groups.len());

    for (country, country_proxies) in &country_groups {
        let country_file = format!("{}{}.txt", args.output_dir, country);
        write_country_file(&country_file, country_proxies)
            .context(format!("Failed to write country file for {}", country))?;
        println!("Created {}: {} proxies", country_file, country_proxies.len());
    }

    let update_file = format!("{}last_update.txt", args.output_dir);
    write_update_file(&update_file)
        .context("Failed to write update file")?;

    let csv_file = format!("{}proxies.csv", args.output_dir);
    write_csv_file(&csv_file, &proxies)
        .context("Failed to write CSV file")?;

    let txt_file = format!("{}proxies.txt", args.output_dir);
    write_txt_file(&txt_file, &proxies)
        .context("Failed to write TXT file")?;

    println!("All files generated successfully!");
    println!("Total proxies processed: {}", proxies.len());
    println!("Countries covered: {}", country_groups.len());

    Ok(())
}

fn read_proxy_file(file_path: &str) -> io::Result<Vec<ProxyEntry>> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let mut proxies = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 4 {
            let proxy = ProxyEntry {
                ip: parts[0].trim().to_string(),
                port: parts[1].trim().to_string(),
                country: parts[2].trim().to_string(),
                isp: parts[3].trim().to_string(),
            };
            proxies.push(proxy);
        }
    }

    Ok(proxies)
}

async fn fetch_csv_proxies(url: &str) -> Result<Vec<ProxyEntry>> {
    let content = reqwest::get(url)
        .await
        .context("Failed to fetch CSV file")?
        .text()
        .await
        .context("Failed to read CSV response body")?;

    let mut proxies = Vec::new();

    for (index, line) in content.lines().enumerate() {
        if index == 0 || line.trim().is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 5 {
            proxies.push(ProxyEntry {
                ip: parts[0].trim().to_string(),
                port: parts[2].trim().to_string(),
                country: parts[4].trim().to_string(),
                isp: String::new(),
            });
        }
    }

    Ok(proxies)
}

fn write_country_file(file_path: &str, proxies: &[ProxyEntry]) -> io::Result<()> {
    let mut file = File::create(file_path)?;
    
    for proxy in proxies {
        writeln!(file, "{} {}", proxy.ip, proxy.port)?;
    }
    
    Ok(())
}

fn write_update_file(file_path: &str) -> io::Result<()> {
    let mut file = File::create(file_path)?;
    let now = Utc::now();
    let tehran_now = now.with_timezone(&Tehran);
    let timestamp = tehran_now.format("%a, %d %b %Y %H:%M:%S").to_string();
    
    writeln!(file, "Last updated: {} – IRN", timestamp)?;
    
    Ok(())
}

fn write_csv_file(file_path: &str, proxies: &[ProxyEntry]) -> io::Result<()> {
    let mut file = File::create(file_path)?;
    
    writeln!(file, "IP Address, Port, TLS, Data Center, Region, City, ASN, latency")?;
    
    for proxy in proxies {
        if proxy.port == "443" {
            writeln!(
                file, 
                "{},{},true,{},N/A,-,{},-", 
                proxy.ip, 
                proxy.port,
                proxy.country,
                proxy.isp
            )?;
        }
    }
    
    Ok(())
}

fn write_txt_file(file_path: &str, proxies: &[ProxyEntry]) -> io::Result<()> {
    let mut file = File::create(file_path)?;
    
    for proxy in proxies {
        writeln!(file, "{} {}", proxy.ip, proxy.port)?;
    }
    
    Ok(())
}
