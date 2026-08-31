use anyhow::{Context, Result};
use reqwest::Client as HttpClient;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::Path;
use vim_rs::core::client::TransportMode;
use vim_rs::core::ClientBuilder;
use vim_rs::mo::{ContainerView, ViewManager, VirtualMachine};
use vim_rs::types::enums::VirtualMachinePowerStateEnum;

const TARGET_PREFIX: &str = "win-";
const TARGET_RANGE_START: u32 = 201;
const TARGET_RANGE_END: u32 = 226;

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

fn is_target_vm(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    if name_lower.contains("maintenance") {
        return false;
    }

    let re_str = format!(r"{}(\d+)", TARGET_PREFIX);
    if let Ok(re) = regex::Regex::new(&re_str) {
        if let Some(caps) = re.captures(&name_lower) {
            if let Some(num_match) = caps.get(1) {
                if let Ok(num) = num_match.as_str().parse::<u32>() {
                    if num >= TARGET_RANGE_START && num <= TARGET_RANGE_END {
                        return true;
                    }
                }
            }
        }
    }
    false
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

async fn probe_and_list_vms(host: &str, user: &str, pass: &str) -> Result<()> {
    println!("\n=== [2] Connecting via vim_rs (TransportMode::Soap) ===");

    let client = ClientBuilder::new(host)
        .basic_authn(user, pass)
        .app_details("vm_probe_rs", env!("CARGO_PKG_VERSION"))
        .insecure(true)
        .transport(TransportMode::Soap)
        .build()
        .await
        .context("Failed to connect to ESXi via vim_rs")?;

    println!("[+] Connection & Authentication Successful!");
    let service_content = client.service_content();
    let about = &service_content.about;
    println!("--- Server Version & About Info ---");
    println!("  Full Name       : {}", about.full_name);
    println!("  Product Line ID : {}", about.product_line_id);
    println!("  API Version     : {}", about.api_version);
    println!("  API Type        : {}", about.api_type);

    let view_mgr_moref = service_content
        .view_manager
        .as_ref()
        .context("ViewManager not present in ServiceContent")?;

    let view_manager = ViewManager::new(client.clone(), &view_mgr_moref.value);

    println!("\n=== [3] Querying Virtual Machine Inventory ===");
    let container_view_moref = view_manager
        .create_container_view(
            &service_content.root_folder,
            Some(&["VirtualMachine".to_string()]),
            true,
        )
        .await
        .context("Failed to create ContainerView for VirtualMachines")?;

    let container_view = ContainerView::new(client.clone(), &container_view_moref.value);
    let vm_morefs = container_view
        .view()
        .await?
        .unwrap_or_default();

    println!("[+] Found {} Virtual Machine(s) registered in ESXi inventory:\n", vm_morefs.len());
    println!("{:<8} | {:<35} | {:<12} | {:<10}", "MoRef", "VM Name", "Power State", "Target?");
    println!("{:-<8}-+-{:-<35}-+-{:-<12}-+-{:-<10}", "", "", "", "");

    for vm_moref in &vm_morefs {
        let vm = VirtualMachine::new(client.clone(), &vm_moref.value);
        let name = vm.name().await.unwrap_or_else(|_| "<unknown>".to_string());
        let runtime = vm.runtime().await;

        let (power_state_str, is_powered_off) = match runtime {
            Ok(r) => match r.power_state {
                VirtualMachinePowerStateEnum::PoweredOn => ("PoweredOn", false),
                VirtualMachinePowerStateEnum::PoweredOff => ("PoweredOff", true),
                VirtualMachinePowerStateEnum::Suspended => ("Suspended", false),
                VirtualMachinePowerStateEnum::Other_(ref s) => (s.as_str(), false),
            },
            Err(e) => ("<error>", false),
        };

        let target = is_target_vm(&name);
        let target_str = if target {
            if is_powered_off { "YES (DOWN)" } else { "YES (UP)" }
        } else {
            "NO"
        };

        println!("{:<8} | {:<35} | {:<12} | {:<10}", vm_moref.value, name, power_state_str, target_str);
    }

    // Clean up container view
    let _ = container_view.destroy_view().await;
    println!("\n[+] Inventory scan complete!");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== VMware ESXi / vCenter Server Capability & VM Probe ===");

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

    // 2. Structured VM Inventory & Power State Inspection
    if let Err(e) = probe_and_list_vms(&host, &user, &pass).await {
        eprintln!("[-] Error querying VM inventory: {:#}", e);
    }

    Ok(())
}


