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
RETRY_DELAY=5  # seconds between reconnection attempts
retry_count=0

# Node-level status check settings
NODE_CHECK_INTERVAL=10  # Check every N successful transactions
TX_COUNTER_FILE="/tmp/migration_tx_counter"
echo "0" > "$TX_COUNTER_FILE"
export NODE_CHECK_INTERVAL TX_COUNTER_FILE SERVER

# Function to check node-level migration status
check_node_status() {
    local result
    result=$(ssh "$SERVER" "curl -s --max-time 35 -H 'Content-Type: application/json' \
        -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"state_trieMigrationStatus\",\"params\":[]}' \
        http://127.0.0.1:9944" 2>/dev/null)

    if [[ -n "$result" ]]; then
        local top_remaining child_remaining
        top_remaining=$(echo "$result" | grep -o '"topRemainingToMigrate":[0-9]*' | cut -d: -f2)
        child_remaining=$(echo "$result" | grep -o '"childRemainingToMigrate":[0-9]*' | cut -d: -f2)

        if [[ -n "$top_remaining" ]]; then
            local msg="📊 Node status: ${top_remaining} top + ${child_remaining} child keys remaining"
            echo "[$(date '+%Y-%m-%d %H:%M:%S')] $msg" >> migration.log
            notify-send "Migration Status" "$msg" -t 8000
        fi
    fi
}
export -f check_node_status

# Function to run migration with output processing
run_migration() {
    ssh "$SERVER" "export SIGNER_SEED='$SEED'; ~/westend-migrate --rpc-url ws://127.0.0.1:9944 --no-notify $RUNS_FLAG" 2>&1 | while read -r line; do
        # Log to file
        echo "$line" >> migration.log

        # Check for notification triggers
        if [[ "$line" == *"Tx #"*"✓"* ]]; then
            notify-send "Migration Progress" "$line" -t 5000

            # Increment counter and check node status every N transactions
            tx_count=$(($(cat "$TX_COUNTER_FILE") + 1))
            echo "$tx_count" > "$TX_COUNTER_FILE"
            if (( tx_count % NODE_CHECK_INTERVAL == 0 )); then
                check_node_status &  # Run in background to not block
            fi
        elif [[ "$line" == *"Migration is COMPLETE"* ]]; then
            notify-send "Migration Complete" "Job Done!" -t 10000
        elif [[ "$line" == *"BALANCE DECREASED"* ]]; then
            notify-send -u critical "CRITICAL WARNING" "$line"
        elif [[ "$line" == *"Westend State-Trie Migration Bot"* ]]; then
            notify-send "Westend Bot Started" "Running remotely on $SERVER" -t 5000
        elif [[ "$line" == *"💓"* ]]; then
            # Dad joke heartbeat - silent/low priority
            joke="${line#*💓 }"
            notify-send -u low "🤣" "$joke" -t 8000
        fi
    done
    return ${PIPESTATUS[0]}  # Return SSH exit code, not while loop
}

# Main reconnection loop
echo "Starting remote migration on $SERVER..." > migration.log
echo "[$(date '+%Y-%m-%d %H:%M:%S')] Starting migration bot" >> migration.log

while true; do
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] Connecting to $SERVER..." >> migration.log

    run_migration
    exit_code=$?

    # Check exit status
    if [[ $exit_code -eq 0 ]]; then
        # Clean exit (--runs completed or migration finished)
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] Migration completed successfully" >> migration.log
        notify-send "Migration" "Completed successfully!" -t 10000
        break
    fi

    # Connection lost or error
    retry_count=$((retry_count + 1))
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] Connection lost (exit code: $exit_code). Retry #$retry_count" >> migration.log

    # Check retry limit
    if [[ $MAX_RETRIES -gt 0 && $retry_count -ge $MAX_RETRIES ]]; then
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] Max retries ($MAX_RETRIES) reached. Giving up." >> migration.log
        notify-send -u critical "Migration Failed" "Max retries reached after connection loss" -t 0
        exit 1
    fi

    # Notify and wait before retry
    notify-send -u normal "Migration Reconnecting" "Connection lost. Retrying in ${RETRY_DELAY}s... (attempt #$retry_count)" -t $((RETRY_DELAY * 1000))
    sleep $RETRY_DELAY

    # Exponential backoff (cap at 60 seconds)
    RETRY_DELAY=$((RETRY_DELAY * 2))
    if [[ $RETRY_DELAY -gt 60 ]]; then
        RETRY_DELAY=60
    fi
done
