use anyhow::{Context, Result};
use clap::Parser;
use std::collections::{HashMap, BTreeMap};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use chrono::Utc;

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

    // Create output directory
    if let Some(parent) = Path::new(&args.output_dir).parent() {
        fs::create_dir_all(parent).context("Failed to create parent directory")?;
    }
    fs::create_dir_all(&args.output_dir).context("Failed to create output directory")?;

    // Read and parse proxy file
    let proxies = read_proxy_file(&args.input_file)
        .context("Failed to read proxy file")?;
    
    println!("Loaded {} proxies from file", proxies.len());

    // Group proxies by country
    let mut country_groups: BTreeMap<String, Vec<ProxyEntry>> = BTreeMap::new();
    
    for proxy in &proxies {
        country_groups
            .entry(proxy.country.clone())
            .or_default()
            .push(proxy.clone());
    }

    println!("Found proxies from {} countries", country_groups.len());

    // Generate country-specific files
    for (country, country_proxies) in &country_groups {
        let country_file = format!("{}{}", args.output_dir, country);
        write_country_file(&country_file, country_proxies)
            .context(format!("Failed to write country file for {}", country))?;
        println!("Created {}: {} proxies", country_file, country_proxies.len());
    }

    // Generate last_update.txt
    let update_file = format!("{}last_update.txt", args.output_dir);
    write_update_file(&update_file)
        .context("Failed to write update file")?;

    // Generate proxies.csv
    let csv_file = format!("{}proxies.csv", args.output_dir);
    write_csv_file(&csv_file, &proxies)
        .context("Failed to write CSV file")?;

    // Generate proxies.txt
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
    let timestamp = now.format("%Y-%m-%d %H:%M:%S").to_string();
    
    writeln!(file, "Last updated: {}", timestamp)?;
    
    Ok(())
}

fn write_csv_file(file_path: &str, proxies: &[ProxyEntry]) -> io::Result<()> {
    let mut file = File::create(file_path)?;
    
    // Write CSV header
    writeln!(file, "IP Address, Port, TLS, Data Center, Region, City, ASN, latency")?;
    
    // Write proxy data
    for proxy in proxies {
        // Since we don't have TLS, Data Center, etc. info, we'll use default values
        writeln!(
            file, 
            "{},{},true,n/a,n/a,n/a,n/a,n/a", 
            proxy.ip, 
            proxy.port
        )?;
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
