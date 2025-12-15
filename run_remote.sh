#!/bin/bash
# Usage: ./run_remote.sh [RUNS]
# Default: continuous (no --runs flag)
#
# Configure SERVER in .env or as environment variable:
#   SERVER=myserver  (SSH alias or user@host)

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

# Run remotely and pipe output
echo "Starting remote migration on $SERVER..." > migration.log

ssh "$SERVER" "export SIGNER_SEED='$SEED'; ~/westend-migrate --rpc-url ws://127.0.0.1:9944 --no-notify $RUNS_FLAG" 2>&1 | while read -r line; do
    # Log to file
    echo "$line" >> migration.log
    
    # Check for notification triggers
    if [[ "$line" == *"Tx #"*"✓"* ]]; then
        notify-send "Migration Progress" "$line" -t 5000
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
