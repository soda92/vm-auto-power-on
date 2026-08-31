use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use vim_rs::core::client::TransportMode;
use vim_rs::core::{Client, ClientBuilder};
use vim_rs::mo::{ContainerView, ViewManager, VirtualMachine};
use vim_rs::types::enums::VirtualMachinePowerStateEnum;

// --- Configuration ---
const SECRETS_JSON: &str = include_str!("../secrets.json");

const TARGET_PREFIX: &str = "win-";
const TARGET_RANGE_START: u32 = 201;
const TARGET_RANGE_END: u32 = 226;
const SCAN_INTERVAL_SECS: u64 = 60;

#[derive(Deserialize, Debug, Clone)]
struct Secrets {
    esxi_host: String,
    esxi_user: String,
    esxi_password: String,
}

fn load_secrets() -> Option<Secrets> {
    match serde_json::from_str(SECRETS_JSON) {
        Ok(secrets) => Some(secrets),
        Err(e) => {
            eprintln!("[-] Error parsing embedded secrets: {}", e);
            None
        }
    }
}

pub fn is_target_vm(name: &str) -> bool {
    let name_lower = name.to_lowercase();

    if name_lower.contains("maintenance") {
        return false;
    }

    let re_str = format!(r"{}(\d+)", TARGET_PREFIX);
    if let Ok(re) = Regex::new(&re_str) {
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

async fn create_esxi_client(secrets: &Secrets) -> Result<Arc<Client>> {
    ClientBuilder::new(&secrets.esxi_host)
        .basic_authn(&secrets.esxi_user, &secrets.esxi_password)
        .app_details(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .insecure(true)
        .transport(TransportMode::Soap)
        .build()
        .await
        .context("Failed to authenticate with ESXi via vim_rs")
}

async fn check_and_start(client: &Arc<Client>) -> Result<()> {
    let service_content = client.service_content();
    let view_mgr_moref = service_content
        .view_manager
        .as_ref()
        .context("ViewManager not present in ServiceContent")?;

    let view_manager = ViewManager::new(client.clone(), &view_mgr_moref.value);

    let container_view_moref = view_manager
        .create_container_view(
            &service_content.root_folder,
            Some(&["VirtualMachine".to_string()]),
            true,
        )
        .await
        .context("Failed to create ContainerView")?;

    let container_view = ContainerView::new(client.clone(), &container_view_moref.value);
    let vm_morefs = container_view.view().await?.unwrap_or_default();

    for vm_moref in &vm_morefs {
        let vm = VirtualMachine::new(client.clone(), &vm_moref.value);
        let name = match vm.name().await {
            Ok(n) => n,
            Err(e) => {
                eprintln!("[-] Failed to fetch name for VM {}: {}", vm_moref.value, e);
                continue;
            }
        };

        if is_target_vm(&name) {
            match vm.runtime().await {
                Ok(runtime) => {
                    if runtime.power_state == VirtualMachinePowerStateEnum::PoweredOff {
                        println!("[!] {} (MoRef: {}) is DOWN. Powering ON...", name, vm_moref.value);
                        if let Err(e) = vm.power_on_vm_task(None).await {
                            eprintln!("[-] Error powering on {}: {}", name, e);
                        } else {
                            println!("[+] Power ON task dispatched for {}.", name);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[-] Failed to query runtime state for {}: {}", name, e);
                }
            }
        }
    }

    let _ = container_view.destroy_view().await;
    Ok(())
}

#[tokio::main]
async fn main() {
    println!("[*] VM Keep Alive Daemon (vim_rs structured edition) started.");

    let secrets = match load_secrets() {
        Some(s) => s,
        None => {
            eprintln!("[-] Fatal: Failed to load secrets. Exiting.");
            return;
        }
    };

    let mut cached_client: Option<Arc<Client>> = None;

    loop {
        println!(
            "--- Run: {} ---",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );

        let client = match cached_client.as_ref() {
            Some(c) => c.clone(),
            None => {
                println!("[*] Establishing authenticated ESXi session ({})...", secrets.esxi_host);
                match create_esxi_client(&secrets).await {
                    Ok(c) => {
                        println!("[+] Connected to ESXi: {}", c.service_content().about.full_name);
                        cached_client = Some(c.clone());
                        c
                    }
                    Err(e) => {
                        eprintln!("[-] Connection error: {:#}", e);
                        println!("[*] Retrying in {} seconds...", SCAN_INTERVAL_SECS);
                        sleep(Duration::from_secs(SCAN_INTERVAL_SECS)).await;
                        continue;
                    }
                }
            }
        };

        if let Err(e) = check_and_start(&client).await {
            eprintln!("[-] Iteration failed: {:#}", e);
            eprintln!("[!] Resetting client session cache for re-authentication on next cycle.");
            cached_client = None;
        }

        sleep(Duration::from_secs(SCAN_INTERVAL_SECS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_target_vm() {
        // Normal target VMs in range (201..=226)
        assert!(is_target_vm("win-201"));
        assert!(is_target_vm("win-214"));
        assert!(is_target_vm("win-226"));

        // Maintenance exclusions
        assert!(!is_target_vm("win-214 [maintenance]"));
        assert!(!is_target_vm("win-205 maintenance"));
        assert!(!is_target_vm("win-201 Maintenance"));

        // Out of target range or non-matching prefix
        assert!(!is_target_vm("win-100"));
        assert!(!is_target_vm("win-250"));
        assert!(!is_target_vm("ubuntu-201"));
        assert!(!is_target_vm("vCenter -192.168.1.190"));
    }
}


