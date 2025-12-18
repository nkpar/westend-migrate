#!/usr/bin/env python3
"""
Migration Monitor - Watches remote bot and sends notifications.
Bot runs autonomously on server, this script monitors and restarts if needed.
"""

import subprocess
import json
import time
import os
import sys
import fcntl
from dataclasses import dataclass
from pathlib import Path
from datetime import datetime
from typing import Optional


def load_dotenv(path: str = ".env") -> dict:
    """Load .env file and return dict of key=value pairs."""
    env = {}
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    key, _, value = line.partition("=")
                    value = value.strip().strip('"').strip("'")
                    env[key.strip()] = value
    except FileNotFoundError:
        pass
    return env


# ============================================================================
# CONFIGURATION - Edit these values as needed
# ============================================================================

@dataclass
class Config:
    # Server
    server: str = "devbox"
    ssh_timeout: int = 30

    # Paths
    remote_log: str = "/tmp/westend-migrate.log"
    local_log: str = "migration.log"
    lockfile: str = "/tmp/monitor.lock"
    seed_file: str = "/tmp/.migration_seed"

    # Monitoring
    check_interval: int = 60  # seconds between checks
    max_stalls: int = 5       # restart after this many checks with no progress

    # RPC
    rpc_url: str = "ws://127.0.0.1:9944"
    rpc_http: str = "http://127.0.0.1:9944"
    account: str = ""  # Set from SIGNER_ACCOUNT env var

    # Bot settings
    bot_binary: str = "./westend-migrate"
    bot_args: str = "--rpc-url ws://127.0.0.1:9944 --item-limit 30720 --size-limit 3072000 --no-notify"


config = Config()
lockfile_handle = None

# ============================================================================
# UTILITIES
# ============================================================================

def log(msg: str):
    """Log message to file and stdout."""
    timestamp = datetime.now().strftime("%H:%M:%S")
    line = f"[{timestamp}] {msg}"
    print(line)
    with open(config.local_log, "a") as f:
        f.write(line + "\n")


def notify(title: str, message: str, urgency: str = "normal", timeout: int = 5000):
    """Send desktop notification."""
    try:
        subprocess.run(
            ["notify-send", "-u", urgency, "-t", str(timeout), title, message],
            capture_output=True,
            timeout=5
        )
    except Exception:
        pass


def ssh(cmd: str, timeout: int = None, retries: int = 3) -> tuple[bool, str]:
    """Run SSH command with retries, return (success, output)."""
    timeout = timeout or config.ssh_timeout
    for attempt in range(retries):
        try:
            result = subprocess.run(
                ["ssh", config.server, cmd],
                capture_output=True,
                text=True,
                timeout=timeout
            )
            if result.returncode == 0:
                return True, result.stdout.strip()
            if result.stderr and "Connection refused" not in result.stderr:
                return False, result.stdout.strip()
        except subprocess.TimeoutExpired:
            pass
        except Exception:
            pass
        if attempt < retries - 1:
            time.sleep(2)
    return False, ""


def rpc_call(method: str, params: list = None, timeout: int = 10) -> Optional[dict]:
    """Make RPC call via SSH curl."""
    params = params or []
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    cmd = f"curl -s --max-time {timeout} -H 'Content-Type: application/json' -d '{payload}' {config.rpc_http}"
    ok, output = ssh(cmd, timeout + 5)
    if ok and output:
        try:
            return json.loads(output)
        except json.JSONDecodeError:
            pass
    return None


# ============================================================================
# BOT MANAGEMENT
# ============================================================================

def is_bot_running() -> bool:
    """Check if bot process is running on remote."""
    ok, output = ssh("pgrep -f westend-migrate", timeout=10)
    return ok and output.strip() != ""


def stop_bot():
    """Stop bot on remote."""
    ssh("pkill -f westend-migrate", timeout=10)
    log("Bot stopped")


def start_bot(seed: str) -> bool:
    """Start bot on remote with seed passed via temp file."""
    log("Starting bot on remote...")

    # Write seed to temp file
    ok, _ = ssh(f"echo '{seed}' > {config.seed_file} && chmod 600 {config.seed_file}")
    if not ok:
        log("ERROR: Failed to write seed file")
        return False

    # Start bot reading seed from file then deleting it
    start_cmd = (
        f"cd ~ && nohup bash -c '"
        f"export SIGNER_SEED=\"$(cat {config.seed_file})\"; "
        f"rm -f {config.seed_file}; "
        f"{config.bot_binary} {config.bot_args} 2>&1"
        f"' > {config.remote_log} 2>&1 &"
    )

    ok, _ = ssh(start_cmd, timeout=15)
    if not ok:
        log("ERROR: Failed to start bot")
        return False

    time.sleep(3)
    if is_bot_running():
        log("Bot started successfully")
        return True
    else:
        log("ERROR: Bot failed to start")
        return False


# ============================================================================
# MONITORING FUNCTIONS
# ============================================================================

def get_nonce() -> Optional[int]:
    """Get current account nonce."""
    result = rpc_call("system_accountNextIndex", [config.account])
    if result and "result" in result:
        return result["result"]
    return None


def get_keys_remaining() -> Optional[int]:
    """Get remaining keys to migrate (slow - scans trie)."""
    result = rpc_call("state_trieMigrationStatus", [], timeout=40)
    if result and "result" in result:
        return result["result"].get("topRemainingToMigrate")
    return None


def get_new_dad_jokes(last_line: int) -> tuple[list[str], int]:
    """Get new dad jokes from remote log."""
    ok, output = ssh(f"wc -l < {config.remote_log}")
    if not ok:
        return [], last_line

    try:
        total_lines = int(output.strip())
    except ValueError:
        return [], last_line

    if total_lines <= last_line:
        return [], last_line

    ok, output = ssh(f"tail -n +{last_line + 1} {config.remote_log}")
    if not ok:
        return [], last_line

    jokes = []
    for line in output.split("\n"):
        if "\U0001f493" in line:  # heart emoji
            joke = line.split("\U0001f493")[-1].strip()
            if joke:
                jokes.append(joke)

    return jokes, total_lines


def check_for_errors() -> Optional[dict]:
    """Check for critical errors in bot log."""
    ok, output = ssh(f"tail -50 {config.remote_log}")
    if not ok:
        return None

    if "Balance decreased" in output or "SLASHING" in output:
        return {"critical": True, "msg": "Balance decreased - possible slashing!"}

    if "5/5" in output and "consecutive" in output.lower():
        return {"critical": True, "msg": "Max retries reached"}

    return None


# ============================================================================
# LOCK MANAGEMENT
# ============================================================================

def acquire_lock() -> bool:
    """Acquire exclusive lock to prevent multiple instances."""
    try:
        global lockfile_handle
        lockfile_handle = open(config.lockfile, "w")
        fcntl.flock(lockfile_handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except (IOError, OSError):
        return False


def release_lock():
    """Release the lock."""
    global lockfile_handle
    if lockfile_handle:
        try:
            fcntl.flock(lockfile_handle, fcntl.LOCK_UN)
            lockfile_handle.close()
        except Exception:
            pass


# ============================================================================
# MAIN MONITOR LOOP
# ============================================================================

def main():
    # Load .env file directly
    dotenv = load_dotenv()

    seed = dotenv.get("SIGNER_SEED") or os.environ.get("SIGNER_SEED")
    if not seed:
        print("ERROR: SIGNER_SEED not found in .env or environment")
        sys.exit(1)

    account = dotenv.get("SIGNER_ACCOUNT") or os.environ.get("SIGNER_ACCOUNT")
    if not account:
        print("ERROR: SIGNER_ACCOUNT not found in .env or environment")
        sys.exit(1)

    server = dotenv.get("SERVER") or os.environ.get("SERVER")
    if server:
        config.server = server

    config.account = account
    print(f"Using account: {account[:10]}...{account[-8:]}")
    print(f"Targeting server: {config.server}")

    # Acquire lock
    if not acquire_lock():
        print("Another monitor instance is already running")
        sys.exit(1)

    # Initialize log
    with open(config.local_log, "a") as f:
        f.write(f"=== Migration Monitor Started ===\n")
        f.write(f"[{datetime.now().strftime('%Y-%m-%d %H:%M:%S')}] Monitoring {config.server}\n")

    notify("Monitor Started", f"Watching migration on {config.server}")

    # Start bot if not running
    if not is_bot_running():
        if not start_bot(seed):
            release_lock()
            sys.exit(1)

    # Get initial state
    raw_nonce = get_nonce()
    raw_keys = get_keys_remaining()

    last_nonce = raw_nonce if raw_nonce is not None else 0
    last_keys = raw_keys if raw_keys is not None else 0
    last_log_line = 0
    stall_count = 0

    log(f"Initial: nonce={last_nonce}, keys={last_keys}")

    try:
        while True:
            time.sleep(config.check_interval)

            # Check SSH connectivity
            ok, _ = ssh("echo ok", timeout=5)
            if not ok:
                log("SSH connection failed, retrying...")
                notify("SSH Failed", f"Cannot reach {config.server}", timeout=5000)
                continue

            # Check if bot is running
            if not is_bot_running():
                log("Bot not running, restarting...")
                notify("Bot Stopped", "Restarting bot...")
                start_bot(seed)
                continue

            # Check for critical errors
            status = check_for_errors()
            if status and status.get("critical"):
                notify("CRITICAL", status["msg"], urgency="critical", timeout=0)
                log(f"CRITICAL: {status['msg']} - stopping monitor")
                break

            # Forward dad jokes
            jokes, last_log_line = get_new_dad_jokes(last_log_line)
            if jokes:
                log(f"Forwarding {len(jokes)} dad joke(s)")
            for joke in jokes:
                notify("Dad Joke", joke, urgency="low", timeout=8000)

            # Get current state
            raw_nonce = get_nonce()
            raw_keys = get_keys_remaining()

            current_nonce = raw_nonce if raw_nonce is not None else last_nonce
            current_keys = raw_keys if raw_keys is not None else last_keys

            nonce_diff = current_nonce - last_nonce
            keys_diff = last_keys - current_keys

            log(f"nonce={current_nonce} (+{nonce_diff}) | keys={current_keys} (-{keys_diff})")

            # Check for stalls
            if nonce_diff == 0 and keys_diff == 0:
                stall_count += 1
                log(f"No progress, stall count: {stall_count}/{config.max_stalls}")

                if stall_count >= config.max_stalls:
                    log("Too many stalls, restarting bot...")
                    notify("Restarting", "Bot stalled, restarting...")
                    stop_bot()
                    time.sleep(2)
                    start_bot(seed)
                    stall_count = 0
            else:
                stall_count = 0
                # Notify on any progress
                if nonce_diff > 0 or keys_diff > 0:
                    notify("Progress", f"{current_keys} keys left | +{nonce_diff} tx | -{keys_diff} keys")

            last_nonce = current_nonce
            last_keys = current_keys

    except KeyboardInterrupt:
        log("Monitor stopped by user")
    finally:
        release_lock()


if __name__ == "__main__":
    main()
