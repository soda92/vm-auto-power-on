use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;
use std::thread;
use std::time::Duration;

// --- Configuration ---
const SECRETS_JSON: &str = include_str!("../secrets.json");

const TARGET_PREFIX: &str = "win-";
const TARGET_RANGE_START: u32 = 201;
const TARGET_RANGE_END: u32 = 226;

#[derive(Deserialize, Debug)]
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

fn ssh_exec(command: &str, secrets: &Secrets) -> Option<String> {
    let target = format!("{}@{}", secrets.esxi_user, secrets.esxi_host);

    let output = Command::new("sshpass")
        .arg("-p")
        .arg(&secrets.esxi_password)
        .arg("ssh")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg("-o")
        .arg("LogLevel=QUIET")
        .arg(&target)
        .arg(command)
        .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("[-] SSH Error running '{}': {}", command, stderr);
                None
            }
        }
        Err(e) => {
            eprintln!("[-] Failed to execute sshpass: {}", e);
            None
        }
    }
}

fn parse_vms_output(raw_output: &str) -> HashMap<String, String> {
    // Matches: <vmid> <vm_name> [<datastore>] <file>.vmx ...
    // Note that VM names may contain brackets (e.g. `[maintenance]`). The File column is identified
    // by the datastore bracket followed by the path ending in .vmx.
    let re = Regex::new(r"^\s*(\d+)\s+(.+?)\s+\[([^\]]+)\]\s+\S+\.vmx").unwrap();
    let mut vms = HashMap::new();

    for line in raw_output.lines() {
        if let Some(caps) = re.captures(line) {
            if let (Some(vmid), Some(name)) = (caps.get(1), caps.get(2)) {
                vms.insert(vmid.as_str().to_string(), name.as_str().trim().to_string());
            }
        }
    }
    vms
}

fn get_vms(secrets: &Secrets) -> HashMap<String, String> {
    let raw_output = match ssh_exec("vim-cmd vmsvc/getallvms", secrets) {
        Some(output) => output,
        None => return HashMap::new(),
    };

    parse_vms_output(&raw_output)
}

fn is_target_vm(name: &str) -> bool {
    let name_lower = name.to_lowercase();

    if name_lower.contains("maintenance") {
        return false;
    }

    // Regex to match TARGET_PREFIX followed by digits
    let re_str = format!(r"{}(\d+)", TARGET_PREFIX);
    let re = Regex::new(&re_str).unwrap();

    if let Some(caps) = re.captures(&name_lower) {
        if let Some(num_match) = caps.get(1) {
            if let Ok(num) = num_match.as_str().parse::<u32>() {
                if num >= TARGET_RANGE_START && num <= TARGET_RANGE_END {
                    return true;
                }
            }
        }
    }

    false
}

fn check_and_start() {
    let secrets = match load_secrets() {
        Some(s) => s,
        None => return,
    };

    println!(
        "--- Run: {} ---",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );

    let vms = get_vms(&secrets);

    for (vmid, name) in vms {
        if is_target_vm(&name) {
            let cmd = format!("vim-cmd vmsvc/power.getstate {}", vmid);
            if let Some(state_output) = ssh_exec(&cmd, &secrets) {
                if state_output.contains("Powered off") {
                    println!("[!] {} (ID: {}) is DOWN. Powering ON...", name, vmid);
                    let power_on_cmd = format!("vim-cmd vmsvc/power.on {}", vmid);
                    ssh_exec(&power_on_cmd, &secrets);
                }
            }
        }
    }
}

fn main() {
    println!("[*] VM Keep Alive Daemon started.");

    loop {
        check_and_start();

        // Wait 60 seconds before the next check
        thread::sleep(Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vms_output_with_spaces_and_brackets() {
        let sample_output = r#"
Vmid   Name                                        File                                                                   Guest OS          Version   Annotation
18     win-214 [maintenance]                       [datastore1] win-214/win-214.vmx                                       windows9_64Guest  vmx-19    
22     win-205                                     [datastore1] win-205/win-205.vmx                                       windows9_64Guest  vmx-19    
30     test-server-01                              [ssd_store] test-server/test.vmx                                       ubuntu64Guest     vmx-17    
"#;
        let vms = parse_vms_output(sample_output);

        assert_eq!(vms.get("18"), Some(&"win-214 [maintenance]".to_string()));
        assert_eq!(vms.get("22"), Some(&"win-205".to_string()));
        assert_eq!(vms.get("30"), Some(&"test-server-01".to_string()));
    }

    #[test]
    fn test_is_target_vm() {
        // Normal target VM in range (201..=226)
        assert!(is_target_vm("win-205"));
        assert!(is_target_vm("win-201"));
        assert!(is_target_vm("win-226"));

        // Maintenance tag in name should be excluded
        assert!(!is_target_vm("win-214 [maintenance]"));
        assert!(!is_target_vm("win-205 maintenance"));
        assert!(!is_target_vm("win-201 Maintenance"));

        // Out of target range or non-matching prefix
        assert!(!is_target_vm("win-100"));
        assert!(!is_target_vm("win-250"));
        assert!(!is_target_vm("ubuntu-205"));
    }
}

