# Westend Migration Bot - Quick Commands

# Default: show available commands
default:
    @just --list

# Start local monitor (watches remote bot, sends notifications)
monitor:
    @pkill -f monitor.py 2>/dev/null || true
    @rm -f /tmp/monitor.lock
    @source .env && python3 monitor.py

# Start monitor in background
monitor-bg:
    @pkill -f monitor.py 2>/dev/null || true
    @rm -f /tmp/monitor.lock
    @sleep 1
    @source .env && nohup python3 monitor.py > /dev/null 2>&1 &
    @echo "Monitor started in background"

# Stop local monitor
stop-monitor:
    @pkill -f monitor.py 2>/dev/null && echo "Monitor stopped" || echo "Monitor not running"
    @rm -f /tmp/monitor.lock

# Check status (bot, nonce, keys remaining)
status:
    #!/usr/bin/env bash
    source .env
    echo "=== Bot Status ==="
    ssh devbox "pgrep -f westend-migrate" && echo "Bot: RUNNING" || echo "Bot: STOPPED"
    echo ""
    echo "=== Nonce ==="
    ssh devbox "curl -s -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"system_accountNextIndex\",\"params\":[\"$SIGNER_ACCOUNT\"]}' http://127.0.0.1:9944" | jq '.result'
    echo ""
    echo "=== Keys Remaining ==="
    ssh devbox "curl -s --max-time 45 -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"state_trieMigrationStatus\",\"params\":[]}' http://127.0.0.1:9944" | jq '.result.topRemainingToMigrate'

# Quick status (just nonce, no slow RPC)
qs:
    #!/usr/bin/env bash
    source .env
    NONCE=$(ssh devbox "curl -s -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"system_accountNextIndex\",\"params\":[\"$SIGNER_ACCOUNT\"]}' http://127.0.0.1:9944" | jq -r '.result')
    BOT=$(ssh devbox "pgrep -f westend-migrate" > /dev/null && echo "✓" || echo "✗")
    echo "Bot: $BOT | Nonce: $NONCE"

# View remote bot log (last 50 lines)
log:
    @ssh devbox "tail -50 /tmp/westend-migrate.log"

# View local monitor log
mlog:
    @tail -30 migration.log

# Follow remote bot log
follow:
    @ssh devbox "tail -f /tmp/westend-migrate.log"

# Stop remote bot
stop-bot:
    @ssh devbox "pkill -f westend-migrate" && echo "Bot stopped" || echo "Bot not running"

# Build release binary locally
build:
    cargo build --release

# Restart remote bot (monitor will auto-restart it)
restart:
    #!/usr/bin/env bash
    ssh devbox "pkill -f westend-migrate" 2>/dev/null || true
    sleep 2
    pkill -f monitor.py 2>/dev/null || true
    rm -f /tmp/monitor.lock
    sleep 1
    source .env && nohup python3 monitor.py > /dev/null 2>&1 &
    echo "Monitor started, waiting for bot..."
    sleep 6
    source .env
    NONCE=$(ssh devbox "curl -s -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"system_accountNextIndex\",\"params\":[\"$SIGNER_ACCOUNT\"]}' http://127.0.0.1:9944" | jq -r '.result')
    BOT=$(ssh devbox "pgrep -f westend-migrate" > /dev/null && echo "✓" || echo "✗")
    echo "Bot: $BOT | Nonce: $NONCE"

# Deploy: build locally, copy to remote, restart
deploy:
    #!/usr/bin/env bash
    set -e
    echo "Building..."
    cargo build --release
    echo "Copying to remote..."
    scp target/release/westend-migrate devbox:~/westend-migrate
    echo "Restarting..."
    ssh devbox "pkill -f westend-migrate" 2>/dev/null || true
    sleep 2
    pkill -f monitor.py 2>/dev/null || true
    rm -f /tmp/monitor.lock
    sleep 1
    source .env && nohup python3 monitor.py > /dev/null 2>&1 &
    echo "Waiting for bot startup..."
    sleep 6
    source .env
    NONCE=$(ssh devbox "curl -s -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"system_accountNextIndex\",\"params\":[\"$SIGNER_ACCOUNT\"]}' http://127.0.0.1:9944" | jq -r '.result')
    BOT=$(ssh devbox "pgrep -f westend-migrate" > /dev/null && echo "✓" || echo "✗")
    echo "Deployed! Bot: $BOT | Nonce: $NONCE"

# Clean SSH sockets (fix connection issues)
clean-ssh:
    @rm -f ~/.ssh/sockets/* 2>/dev/null && echo "SSH sockets cleaned" || echo "No sockets to clean"

# Full restart with SSH cleanup
fresh:
    #!/usr/bin/env bash
    rm -f ~/.ssh/sockets/* 2>/dev/null
    echo "SSH sockets cleaned"
    ssh devbox "pkill -f westend-migrate" 2>/dev/null || true
    sleep 2
    pkill -f monitor.py 2>/dev/null || true
    rm -f /tmp/monitor.lock
    sleep 1
    source .env && nohup python3 monitor.py > /dev/null 2>&1 &
    echo "Monitor started, waiting for bot..."
    sleep 6
    source .env
    NONCE=$(ssh devbox "curl -s -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"system_accountNextIndex\",\"params\":[\"$SIGNER_ACCOUNT\"]}' http://127.0.0.1:9944" | jq -r '.result')
    BOT=$(ssh devbox "pgrep -f westend-migrate" > /dev/null && echo "✓" || echo "✗")
    echo "Bot: $BOT | Nonce: $NONCE"
