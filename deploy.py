#!/usr/bin/env python3
import json
import os
import subprocess
import sys

# Configuration
PROJECT_SUBDIR = "vm_keep_alive_rs"
SECRETS_FILE = os.path.join(PROJECT_SUBDIR, "secrets.json")
DEMO_SECRETS_FILE = os.path.join(PROJECT_SUBDIR, "secrets_demo.json")
SERVICE_TEMPLATE = os.path.join(PROJECT_SUBDIR, "vm_keep_alive.service")
RUST_PROJECT_DIR = PROJECT_SUBDIR
BINARY_NAME = "vm_keep_alive_rs"
REMOTE_OPT_DIR = "/opt/vm_keep_alive"
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


def scp_to_tmp(host, user, local_path):
    """SCPs a file to /tmp/ on the remote host."""
    filename = os.path.basename(local_path)
    remote_path = f"/tmp/{filename}"
    print(f"[+] Uploading {local_path} -> {host}:{remote_path}")
    subprocess.check_call(
        f"scp -o StrictHostKeyChecking=no {local_path} {user}@{host}:{remote_path}",
        shell=True,
    )
    return remote_path


def run_remote_script(host, user, script_content):
    """Writes script to file, SCPs it, and executes it."""
    local_script = "deploy_script.sh"
    remote_script = "/tmp/deploy_script.sh"

    with open(local_script, "w") as f:
        f.write(script_content)

    try:
        scp_to_tmp(host, user, local_script)
        # Execute with bash. using sudo inside the script or invoking with sudo bash depends on setup.
        # We will assume the user has sudo rights and invoke with 'sudo bash' or script has sudo inside.
        # Let's run the script itself with sudo bash to be safe.
        print("[+] Executing remote installation script...")
        cmd = f"ssh -o StrictHostKeyChecking=no -t {user}@{host} 'sudo bash {remote_script}'"
        subprocess.check_call(cmd, shell=True)
    finally:
        if os.path.exists(local_script):
            os.remove(local_script)


def build_rust():
    print("[*] Building Rust binary...")
    run_local(f"cd {RUST_PROJECT_DIR} && cargo build --release")


def prepare_service_file():
    """Reads the template and returns the path to the temporary file."""
    with open(SERVICE_TEMPLATE, "r") as f:
        content = f.read()

    # We run as root by default in /opt
    content = content.replace("{{USER}}", "root")

    tmp_file = "vm_keep_alive.service.tmp"
    with open(tmp_file, "w") as f:
        f.write(content)
    return tmp_file


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

    # 2. Prepare Service File
    service_local_path = prepare_service_file()

    # 3. Upload Artifacts to /tmp
    try:
        remote_bin = scp_to_tmp(host, user, binary_local_path)
        remote_svc = scp_to_tmp(host, user, service_local_path)
    finally:
        if os.path.exists(service_local_path):
            os.remove(service_local_path)

    # 4. Generate & Run Remote Install Script
    install_script = f"""
set -e

echo "--> Stopping {SERVICE_NAME}..."
systemctl stop {SERVICE_NAME} || true

echo "--> Preparing Directory {REMOTE_OPT_DIR}..."
mkdir -p {REMOTE_OPT_DIR}
chmod 700 {REMOTE_OPT_DIR}

echo "--> Installing Binary..."
mv -f {remote_bin} {REMOTE_OPT_DIR}/{BINARY_NAME}
chmod 755 {REMOTE_OPT_DIR}/{BINARY_NAME}

echo "--> Installing Service..."
mv -f {remote_svc} /etc/systemd/system/{SERVICE_NAME}
systemctl daemon-reload

echo "--> Enabling & Starting Service..."
systemctl enable {SERVICE_NAME}
systemctl start {SERVICE_NAME}

echo "--> Waiting for service to settle (3s)..."
sleep 3

if systemctl is-active --quiet {SERVICE_NAME}; then
    echo "[OK] Service is running!"
    systemctl status {SERVICE_NAME} --no-pager
else
    echo "[!!] Service FAILED to start!"
    journalctl -u {SERVICE_NAME} -n 20 --no-pager
    exit 1
fi
"""
    run_remote_script(host, user, install_script)
    print("\n[*] Deployment Successful!")


if __name__ == "__main__":
    main()
