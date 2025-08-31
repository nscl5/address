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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::{TlsConnector, rustls::{ClientConfig, RootCertStore}};
use rustls_native_certs;

const CLOUDFLARE_HOST: &str = "speed.cloudflare.com";
const CLOUDFLARE_PATH: &str = "/meta";
const DEFAULT_PROXY_FILE: &str = "Data/ProxyIsp.txt";
const DEFAULT_OUTPUT_FILE: &str = "ProxyIP-Daily.md";
const DEFAULT_MAX_CONCURRENT: usize = 20;
const DEFAULT_TIMEOUT_SECONDS: u64 = 8;

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
#[command(name = "Cloudflare Proxy Checker")]
#[command(about = "Checks proxies using Cloudflare speed test endpoint")]
struct Args {
    /// Path to the proxy file
    #[arg(short, long, default_value = DEFAULT_PROXY_FILE)]
    proxy_file: String,

    /// Output file path
    #[arg(short, long, default_value = DEFAULT_OUTPUT_FILE)]
    output_file: String,

    /// Max concurrent tasks
    #[arg(long, default_value_t = DEFAULT_MAX_CONCURRENT)]
    max_concurrent: usize,

    /// Timeout in seconds
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
    timeout: u64,
}

#[derive(Deserialize, Debug, Clone)]
struct CloudflareResponse {
    #[serde(rename = "clientIp")]
    client_ip: String,
    #[serde(rename = "asOrganization")]
    as_organization: Option<String>,
    #[serde(rename = "asn")]
    asn: Option<u32>,
    country: Option<String>,
    city: Option<String>,
    region: Option<String>,
    #[serde(rename = "colo")]
    colo: Option<String>,
}

#[derive(Debug, Clone)]
struct ProxyInfo {
    ip: String,
    port: String,
    country: String,
    org: String,
    ping: u128,
    cf_info: CloudflareResponse,
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

    // فیلتر کردن پروکسی‌های خوب
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

    let active_proxies = Arc::new(Mutex::new(Vec::<ProxyInfo>::new()));

    // دریافت IP اصلی (بدون پروکسی)
    let original_response = get_cloudflare_info(None, args.timeout).await
        .context("Failed to get original IP info")?;
    println!("Original IP: {}", original_response.client_ip);

    let tasks = futures::stream::iter(
        proxies.into_iter().map(|proxy_line| {
            let active_proxies = Arc::clone(&active_proxies);
            let args = args.clone();
            let original_response = original_response.clone();
            async move {
                process_proxy_cloudflare(proxy_line, &active_proxies, &args, &original_response).await;
            }
        })
    )
    .buffer_unordered(args.max_concurrent)
    .collect::<Vec<()>>();

    tasks.await;

    write_markdown_file_cf(&active_proxies.lock().unwrap(), &args.output_file)
        .context("Failed to write Markdown file")?;

    println!("Cloudflare proxy checking completed.");
    Ok(())
}

async fn get_cloudflare_info(proxy: Option<(&str, &str)>, timeout_secs: u64) -> Result<CloudflareResponse> {
    let start = Instant::now();
    
    // تنظیم TLS
    let mut root_cert_store = RootCertStore::empty();
    root_cert_store.add_server_trust_anchors(
        rustls_native_certs::load_native_certs()?
            .into_iter()
            .map(|cert| rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                cert.0,
                cert.1,
                cert.2,
            ))
    );

    let mut config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));

    // اتصال به سرور (مستقیم یا از طریق پروکسی)
    let stream = if let Some((proxy_ip, proxy_port)) = proxy {
        tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            TcpStream::connect(format!("{}:{}", proxy_ip, proxy_port))
        ).await??
    } else {
        tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            TcpStream::connect(format!("{}:443", CLOUDFLARE_HOST))
        ).await??
    };

    // TLS handshake
    let domain = rustls::ServerName::try_from(CLOUDFLARE_HOST)?;
    let mut tls_stream = connector.connect(domain, stream).await?;

    // ارسال HTTP request
    let request = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36\r\n\
         Connection: close\r\n\r\n",
        CLOUDFLARE_PATH, CLOUDFLARE_HOST
    );

    tls_stream.write_all(request.as_bytes()).await?;

    // خواندن پاسخ
    let mut response = Vec::new();
    tls_stream.read_to_end(&mut response).await?;

    let response_str = String::from_utf8_lossy(&response);
    
    // جدا کردن header از body
    if let Some(body_start) = response_str.find("\r\n\r\n") {
        let body = &response_str[body_start + 4..];
        let cf_response: CloudflareResponse = serde_json::from_str(body.trim())?;
        return Ok(cf_response);
    }

    Err(anyhow::anyhow!("Invalid HTTP response"))
}

async fn process_proxy_cloudflare(
    proxy_line: String,
    active_proxies: &Arc<Mutex<Vec<ProxyInfo>>>,
    args: &Args,
    original_response: &CloudflareResponse,
) {
    let parts: Vec<&str> = proxy_line.split(',').collect();
    if parts.len() < 4 {
        return;
    }
    
    let ip = parts[0];
    let port = parts[1];
    let country = parts[2];
    let org = parts[3];

    let start = Instant::now();

    match get_cloudflare_info(Some((ip, port)), args.timeout).await {
        Ok(proxy_response) => {
            let ping = start.elapsed().as_millis();
            
            // بررسی اینکه IP واقعاً تغییر کرده (پروکسی کار میکنه)
            if original_response.client_ip != proxy_response.client_ip {
                let cleaned_org = clean_org_name(&proxy_response.as_organization.unwrap_or_else(|| org.to_string()));
                
                let proxy_info = ProxyInfo {
                    ip: ip.to_string(),
                    port: port.to_string(),
                    country: proxy_response.country.unwrap_or_else(|| country.to_string()),
                    org: cleaned_org,
                    ping,
                    cf_info: proxy_response.clone(),
                };

                println!(
                    "{}",
                    format!(
                        "CF PROXY LIVE 🟢: {}:{} -> {} ({} ms) [{}]",
                        ip, port, proxy_response.client_ip, ping,
                        proxy_response.country.unwrap_or_default()
                    ).green()
                );

                active_proxies.lock().unwrap().push(proxy_info);
            } else {
                println!("PROXY DEAD 💀: {}:{} (IP not changed)", ip, port);
        }
    }
}
