#!/bin/bash
# Usage: ./run_remote.sh [RUNS]
# Default: continuous (no --runs flag)
#
# Configure SERVER in .env or as environment variable:
#   SERVER=myserver  (SSH alias or user@host)
#
# Features:
#   - Auto-reconnects on SSH connection loss
#   - Desktop notifications for progress/errors
#   - Dad joke heartbeat notifications

set -o pipefail

# Prevent multiple instances
LOCKFILE="/tmp/run_remote.lock"
exec 200>"$LOCKFILE"
if ! flock -n 200; then
    echo "Another instance is already running (lockfile: $LOCKFILE)"
    exit 1
fi
echo $$ > "$LOCKFILE"

# Cleanup on exit
trap 'rm -f "$LOCKFILE"' EXIT

# Load config from .env file
if [[ -f .env ]]; then
    source .env
else
    echo "Error: .env file not found."
    echo "Create it with:"
    echo '  SIGNER_SEED="your mnemonic"'
    echo '  SERVER=your-ssh-alias'
    exit 1
fi

SERVER="${SERVER:-server}"
SEED="$SIGNER_SEED"
RUNS="${1:-}"

# Build runs flag if specified
RUNS_FLAG=""
if [[ -n "$RUNS" ]]; then
    RUNS_FLAG="--runs $RUNS"
fi

# Reconnection settings
MAX_RETRIES=0  # 0 = infinite retries
retry_count=0
# Fibonacci backoff: 5, 5, 10, 15, 25, 40, 65... (capped at 60)
FIB_PREV=0
FIB_CURR=5

# Node-level status check settings
NODE_CHECK_INTERVAL=10  # Check every N successful transactions
TX_COUNTER_FILE="/tmp/migration_tx_counter"
TX_START_TIME="/tmp/migration_start_time"
TX_ERRORS_FILE="/tmp/migration_errors"
echo "0" > "$TX_COUNTER_FILE"
echo "0" > "$TX_ERRORS_FILE"
date +%s > "$TX_START_TIME"
export NODE_CHECK_INTERVAL TX_COUNTER_FILE TX_START_TIME TX_ERRORS_FILE SERVER

# Strip ANSI escape codes
strip_ansi() {
    echo "$1" | sed 's/\x1b\[[0-9;]*m//g'
}
export -f strip_ansi

# Function to check node-level migration status
check_node_status() {
    local result
    # Use -o ControlPath=none to avoid conflicting with the main multiplexed connection
    result=$(ssh -o ControlPath=none "$SERVER" "curl -s --max-time 35 -H 'Content-Type: application/json' \
        -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"state_trieMigrationStatus\",\"params\":[]}' \
        http://127.0.0.1:9944" 2>/dev/null)

    if [[ -n "$result" ]]; then
        local top_remaining child_remaining
        top_remaining=$(echo "$result" | grep -o '"topRemainingToMigrate":[0-9]*' | cut -d: -f2)
        child_remaining=$(echo "$result" | grep -o '"childRemainingToMigrate":[0-9]*' | cut -d: -f2)

        if [[ -n "$top_remaining" ]]; then
            local total=$((top_remaining + child_remaining))
            local tx_count=$(cat "$TX_COUNTER_FILE" 2>/dev/null || echo 0)
            local msg="📊 $total keys remaining (top: $top_remaining)"
            echo "[$(date '+%Y-%m-%d %H:%M:%S')] Node: $msg | Session tx: $tx_count" >> migration.log
            notify-send "📊 Node Status" "$msg\nSession: $tx_count transactions" -t 8000
        fi
    fi
}
export -f check_node_status

# Function to run migration with output processing
run_migration() {
    # Kill any orphaned bot processes before starting (prevents lockfile conflicts)
    ssh "$SERVER" "pkill -9 westend-migrate 2>/dev/null; rm -f /tmp/westend-migrate.lock" 2>/dev/null || true

    ssh "$SERVER" "export SIGNER_SEED='$SEED'; ~/westend-migrate --rpc-url ws://127.0.0.1:9944 --no-notify $RUNS_FLAG" 2>&1 | while read -r line; do
        # Log to file
        echo "$line" >> migration.log

        # Check for notification triggers
        if [[ "$line" == *"Tx #"*"✓"* ]]; then
            # Increment counter
            tx_count=$(($(cat "$TX_COUNTER_FILE") + 1))
            echo "$tx_count" > "$TX_COUNTER_FILE"

            # Calculate stats
            start_ts=$(cat "$TX_START_TIME")
            now_ts=$(date +%s)
            elapsed=$((now_ts - start_ts))
            errors=$(cat "$TX_ERRORS_FILE")

            if (( elapsed > 0 )); then
                rate=$(echo "scale=1; $tx_count * 60 / $elapsed" | bc)
            else
                rate="--"
            fi

            # Parse tx number and block from line (e.g., "Tx #5 ✓ finalized in block 0x...")
            clean_line=$(strip_ansi "$line")
            tx_num=$(echo "$clean_line" | grep -oP 'Tx #\K[0-9]+' || echo "$tx_count")

            # Build informative notification
            notify-send "✓ Tx #$tx_num" "Session: $tx_count tx | ${rate}/min | $errors errors" -t 4000

            # Check node status every N transactions
            if (( tx_count % NODE_CHECK_INTERVAL == 0 )); then
                check_node_status &  # Run in background to not block
            fi
        elif [[ "$line" == *"Migration is COMPLETE"* ]]; then
            tx_count=$(cat "$TX_COUNTER_FILE")
            errors=$(cat "$TX_ERRORS_FILE")
            notify-send "🎉 Migration Complete!" "Total: $tx_count tx | $errors errors" -t 0
        elif [[ "$line" == *"BALANCE DECREASED"* ]]; then
            notify-send -u critical "⚠️ SLASHING DETECTED" "$(strip_ansi "$line")"
        elif [[ "$line" == *"Migration transaction failed"* ]]; then
            # Only notify on summary line (not "Dry run FAILED" which precedes it)
            errors=$(($(cat "$TX_ERRORS_FILE") + 1))
            echo "$errors" > "$TX_ERRORS_FILE"
            # Extract attempt number e.g. "(2/5)"
            attempt=$(echo "$line" | grep -oP '\(\d+/\d+\)' || echo "")
            notify-send -u normal "❌ Retry $attempt" "$(strip_ansi "$line" | head -c 200)" -t 6000
        elif [[ "$line" == *"Westend State-Trie Migration Bot"* ]]; then
            notify-send "🚀 Bot Started" "Connected to $SERVER" -t 5000
        elif [[ "$line" == *"💓"* ]]; then
            # Dad joke heartbeat - keep it!
            joke="${line#*💓 }"
            joke=$(strip_ansi "$joke")
            notify-send -u low "🤣 Dad Joke" "$joke" -t 8000
        fi
    done
    return ${PIPESTATUS[0]}  # Return SSH exit code, not while loop
}

# Main reconnection loop
echo "Starting remote migration on $SERVER..." > migration.log
echo "[$(date '+%Y-%m-%d %H:%M:%S')] Starting migration bot" >> migration.log

while true; do
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] Connecting to $SERVER..." >> migration.log

    run_start=$(date +%s)
    run_migration
    exit_code=$?
    run_duration=$(($(date +%s) - run_start))

    # Check exit status
    if [[ $exit_code -eq 0 ]]; then
        # Clean exit (--runs completed or migration finished)
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] Migration completed successfully" >> migration.log
        notify-send "Migration" "Completed successfully!" -t 10000
        break
    fi

    # Connection lost or error - reset Fibonacci if bot ran for >60s (was stable)
    if [[ $run_duration -gt 60 ]]; then
        # Bot was running fine, this is a fresh disconnect - reset backoff
        FIB_PREV=0
        FIB_CURR=5
        retry_count=0
    fi
    retry_count=$((retry_count + 1))
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] Connection lost (exit code: $exit_code). Retry #$retry_count" >> migration.log

    # Check retry limit
    if [[ $MAX_RETRIES -gt 0 && $retry_count -ge $MAX_RETRIES ]]; then
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] Max retries ($MAX_RETRIES) reached. Giving up." >> migration.log
        notify-send -u critical "Migration Failed" "Max retries reached after connection loss" -t 0
        exit 1
    fi

    # Notify and wait before retry (Fibonacci backoff)
    notify-send -u normal "Migration Reconnecting" "Connection lost. Retrying in ${FIB_CURR}s... (attempt #$retry_count)" -t $((FIB_CURR * 1000))
    sleep $FIB_CURR

    # Fibonacci backoff: next = prev + curr (capped at 60)
    FIB_NEXT=$((FIB_PREV + FIB_CURR))
    FIB_PREV=$FIB_CURR
    FIB_CURR=$FIB_NEXT
    if [[ $FIB_CURR -gt 60 ]]; then
        FIB_CURR=60
    fi
done
