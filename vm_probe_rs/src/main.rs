use anyhow::{Context, Result};
use reqwest::Client as HttpClient;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::Path;
use vim_rs::core::client::TransportMode;
use vim_rs::core::ClientBuilder;

#[derive(Deserialize, Debug)]
struct Secrets {
    esxi_host: String,
    esxi_user: String,
    esxi_password: String,
}

fn load_secrets() -> Result<Secrets> {
    let candidate_paths = [
        "secrets.json",
        "../secrets.json",
        "../vm_keep_alive_rs/secrets.json",
        "../vm_keep_alive_rs/secrets_demo.json",
    ];

    for path_str in candidate_paths {
        let path = Path::new(path_str);
        if path.exists() {
            println!("[+] Loading credentials from: {}", path.display());
            let content = fs::read_to_string(path)?;
            let secrets: Secrets = serde_json::from_str(&content)?;
            return Ok(secrets);
        }
    }

    anyhow::bail!("No secrets.json or secrets_demo.json found");
}

async fn probe_raw_endpoints(host: &str) -> Result<()> {
    println!("\n=== [1] Probing Raw HTTPS Endpoints on https://{} ===", host);

    let client = HttpClient::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()?;

    let endpoints = [
        ("VIM Service Versions (SOAP)", format!("https://{}/sdk/vimServiceVersions.xml", host)),
        ("vCenter REST API Root", format!("https://{}/api", host)),
        ("vSphere REST Session Endpoint", format!("https://{}/rest/com/vmware/cis/session", host)),
        ("Managed Object Browser (MOB)", format!("https://{}/mob", host)),
    ];

    for (desc, url) in endpoints {
        print!("[*] Checking {} ({})... ", desc, url);
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let preview = body.lines().take(3).collect::<Vec<_>>().join(" ");
                println!("Status: {} | Preview: {:.100}", status, preview);
                if desc.contains("VIM Service Versions") && status.is_success() {
                    println!("    --> Found vimServiceVersions! Response contains VIM versions.");
                }
            }
            Err(e) => {
                println!("Failed: {}", e);
            }
        }
    }

    Ok(())
}

async fn probe_vim_rs(host: &str, user: &str, pass: &str, mode: TransportMode, mode_name: &str) {
    println!("\n=== [2] Probing via vim_rs with Transport: {} ===", mode_name);

    let client_res = ClientBuilder::new(host)
        .basic_authn(user, pass)
        .app_details("vm_probe_rs", env!("CARGO_PKG_VERSION"))
        .insecure(true)
        .transport(mode)
        .build()
        .await;

    match client_res {
        Ok(client) => {
            println!("[+] Connection & Authentication Successful!");
            let about = &client.service_content().about;
            println!("--- Server Version & About Info ---");
            println!("  Full Name       : {}", about.full_name);
            println!("  Product Line ID : {}", about.product_line_id);
            println!("  API Version     : {}", about.api_version);
            println!("  API Type        : {}", about.api_type);
            println!("  Version         : {}", about.version);
            println!("  Build           : {}", about.build);
            println!("  OS Type         : {}", about.os_type);
            println!("  Vendor          : {}", about.vendor);
        }
        Err(e) => {
            println!("[-] vim_rs connection failed with {}: {:#}", mode_name, e);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== VMware ESXi / vCenter Server Capability Probe ===");

    let secrets = load_secrets().context("Failed to load secrets")?;
    let host = env::var("ESXI_HOST").unwrap_or(secrets.esxi_host);
    let user = env::var("ESXI_USER").unwrap_or(secrets.esxi_user);
    let pass = env::var("ESXI_PASSWORD").unwrap_or(secrets.esxi_password);

    println!("[*] Target Host: {}", host);
    println!("[*] Username   : {}", user);

    // 1. Raw HTTPS Endpoint Probe
    if let Err(e) = probe_raw_endpoints(&host).await {
        eprintln!("[-] Error in raw endpoint probe: {}", e);
    }

    // 2. Test vim_rs with Auto and SOAP
    probe_vim_rs(&host, &user, &pass, TransportMode::Auto, "Auto (VI JSON or SOAP)").await;
    probe_vim_rs(&host, &user, &pass, TransportMode::Soap, "Direct SOAP/XML").await;

    Ok(())
}

