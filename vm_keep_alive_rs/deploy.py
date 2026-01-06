#!/usr/bin/env python3
import json
import os
import subprocess
import sys
from pathlib import Path

# Configuration
SECRETS_FILE = "secrets.json"
DEMO_SECRETS_FILE = "secrets_demo.json"
SERVICE_TEMPLATE = "vm_keep_alive.service"
RUST_PROJECT_DIR = "."
BINARY_NAME = "vm_keep_alive_rs"
REMOTE_DIR = "/opt/vm_keep_alive"
REMOTE_BINARY_PATH = f"{REMOTE_DIR}/{BINARY_NAME}"
SERVICE_NAME = "vm_keep_alive.service"


def load_config():
    """Load deployment config from secrets.json (preferred) or demo."""
    if os.path.exists(SECRETS_FILE):
        path = SECRETS_FILE
    elif os.path.exists(DEMO_SECRETS_FILE):
        path = DEMO_SECRETS_FILE
        print(
            f"[!] Warning: Using {DEMO_SECRETS_FILE}. Please create {SECRETS_FILE} for real credentials."
        )
    else:
        print("[-] No secrets file found.")
        sys.exit(1)

    with open(path, "r") as f:
        return json.load(f)


def run_local(cmd):
    print(f"[+] Local: {cmd}")
    subprocess.check_call(cmd, shell=True)


def run_remote(host, user, cmd, sudo=False):
    ssh_cmd = f"ssh -o StrictHostKeyChecking=no {user}@{host}"
    if sudo:
        # Assumes user has sudo NOPASSWD or you'll need to handle prompt
        final_cmd = f"{ssh_cmd} 'sudo {cmd}'"
    else:
        final_cmd = f"{ssh_cmd} '{cmd}'"

    print(f"[+] Remote ({host}): {cmd}")
    subprocess.check_call(final_cmd, shell=True)


def scp_file(host, user, local_path, remote_path, sudo=False):
    print(f"[+] Uploading {local_path} -> {host}:{remote_path}")

    if sudo:
        # SCP to tmp first then sudo mv
        tmp_path = f"/tmp/{os.path.basename(local_path)}"
        subprocess.check_call(
            f"scp -o StrictHostKeyChecking=no {local_path} {user}@{host}:{tmp_path}",
            shell=True,
        )
        run_remote(host, user, f"mv {tmp_path} {remote_path}", sudo=True)
    else:
        subprocess.check_call(
            f"scp -o StrictHostKeyChecking=no {local_path} {user}@{host}:{remote_path}",
            shell=True,
        )


def build_rust():
    print("[*] Building Rust binary...")
    run_local(f"cd {RUST_PROJECT_DIR} && cargo build --release")


def prepare_service_file(user):
    """Reads the template and fills in the user."""
    with open(SERVICE_TEMPLATE, "r") as f:
        content = f.read()

    # Replace {{USER}} with the user who will own the files (root for /opt usually, or specific)
    # Since we are deploying to /opt, running as root is easiest for permissions,
    # but running as 'user' is safer if we chown properly.
    # For this script, let's run as root to keep /opt management simple,
    # OR we can run as the ssh user provided in config.

    # Let's decide: Service runs as root (standard for system services in /opt).
    # If user wants otherwise, they can change this string.
    service_user = "root"
    content = content.replace("{{USER}}", service_user)

    with open("vm_keep_alive.service.tmp", "w") as f:
        f.write(content)
    return "vm_keep_alive.service.tmp", service_user


def main():
    config = load_config()
    host = config.get("ubuntu_host")
    user = config.get("ubuntu_user")

    if not host or not user:
        print("[-] Missing ubuntu_host or ubuntu_user in secrets file.")
        sys.exit(1)

    # 1. Build
    build_rust()
    binary_local_path = f"{RUST_PROJECT_DIR}/target/release/{BINARY_NAME}"

    # 2. Prepare Remote Directory
    print("[*] Preparing remote directory...")
    # Create dir, owned by root (since we use sudo), chmod 700 (secure)
    run_remote(host, user, f"mkdir -p {REMOTE_DIR}", sudo=True)
    run_remote(host, user, f"chmod 700 {REMOTE_DIR}", sudo=True)

    # 3. Upload Binary
    scp_file(host, user, binary_local_path, REMOTE_BINARY_PATH, sudo=True)
    run_remote(host, user, f"chmod 755 {REMOTE_BINARY_PATH}", sudo=True)

    # 5. Service Configuration
    service_file, service_user = prepare_service_file(user)
    remote_service_path = f"/etc/systemd/system/{SERVICE_NAME}"
    scp_file(host, user, service_file, remote_service_path, sudo=True)

    # Clean up tmp file
    os.remove(service_file)

    # 6. Reload and Restart
    print("[*] reloading systemd...")
    run_remote(host, user, "systemctl daemon-reload", sudo=True)
    run_remote(host, user, f"systemctl enable {SERVICE_NAME}", sudo=True)
    run_remote(host, user, f"systemctl restart {SERVICE_NAME}", sudo=True)

    print("[*] Deployment Complete!")


if __name__ == "__main__":
    main()
