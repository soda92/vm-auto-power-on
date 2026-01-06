#!/usr/bin/env python3
import subprocess
import json
import re
import os
import datetime

# --- Configuration ---
# Use absolute path to ensure Cron finds it
SECRET_FILE = os.path.expanduser("~/scripts/secrets.json")

# Target Range: win201 to win226
TARGET_PREFIX = "win"
TARGET_RANGE = (201, 226)


def load_secrets():
    """Reads host/user/pass from the locked JSON file."""
    try:
        with open(SECRET_FILE, "r") as f:
            return json.load(f)
    except Exception as e:
        print(f"[-] Error loading secrets: {e}")
        return None


def ssh_exec(command, secrets):
    """Executes command on ESXi via sshpass + ssh"""
    if not secrets:
        return None

    ssh_cmd = [
        "sshpass",
        "-p",
        secrets["esxi_password"],
        "ssh",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "LogLevel=QUIET",
        f"{secrets['esxi_user']}@{secrets['esxi_host']}",
        command,
    ]

    try:
        # We capture stdout. stderr is ignored to keep logs clean unless it fails hard.
        result = subprocess.run(ssh_cmd, capture_output=True, text=True, check=True)
        return result.stdout.strip()
    except subprocess.CalledProcessError as e:
        # If SSH fails (auth error, network down), log it
        print(f"[-] SSH Error running '{command}': {e.stderr}")
        return None


def get_vms(secrets):
    """Returns a dict of {vmid: vmname}"""
    raw_output = ssh_exec("vim-cmd vmsvc/getallvms", secrets)
    if not raw_output:
        return {}

    vms = {}
    for line in raw_output.splitlines():
        parts = line.strip().split()
        # Ensure line starts with a digit (VMID)
        if len(parts) > 1 and parts[0].isdigit():
            vms[parts[0]] = parts[1]
    return vms


def is_target_vm(name):
    """
    Filter logic:
    1. Must NOT have 'MAINTENANCE' (case insensitive).
    2. Must match 'winXXX' where XXX is 201-226.
    """
    name_lower = name.lower()

    if "maintenance" in name_lower:
        return False

    match = re.search(r"win(\d+)", name_lower)
    if match:
        num = int(match.group(1))
        if TARGET_RANGE[0] <= num <= TARGET_RANGE[1]:
            return True

    return False


def check_and_start():
    secrets = load_secrets()
    if not secrets:
        return

    print(f"--- Run: {datetime.datetime.now()} ---")
    vms = get_vms(secrets)

    for vmid, name in vms.items():
        if is_target_vm(name):
            # Check state
            state_output = ssh_exec(f"vim-cmd vmsvc/power.getstate {vmid}", secrets)

            if state_output and "Powered off" in state_output:
                print(f"[!] {name} (ID: {vmid}) is DOWN. Powering ON...")
                ssh_exec(f"vim-cmd vmsvc/power.on {vmid}", secrets)


if __name__ == "__main__":
    check_and_start()
