# Operations Guide - Westend State-Trie Migration Bot

This document provides operational guidance for running and monitoring the state-trie migration bot in production.

## Quick Start

```bash
# 1. Configure environment
cat > .env <<EOF
SIGNER_SEED="your mnemonic phrase"
SERVER=your-ssh-alias
EOF

# 2. Build and deploy
cargo build --release
scp ./target/release/westend-migrate $SERVER:~/

# 3. Run with monitoring
./run_remote.sh 100  # Run 100 migrations
```

## Understanding Progress Metrics

### Two Different Measurement Systems

The migration has **two separate progress counters** that measure different things:

#### 1. Pallet Counter (MigrationProcess Storage)

**What it measures:** Cumulative work done by bot
**How to check:**
```bash
./westend-migrate --status --rpc-url ws://127.0.0.1:9944
```

**Output example:**
```
Status: top=wip/356466 child=wip/0 size=27836416
```

**Interpretation:**
- `top=356466`: Bot has processed 356,466 items total
- Increments by ~1024 per transaction
- **NOT** the number of items remaining
- Use this to verify bot is working

#### 2. Node RPC (state_trieMigrationStatus)

**What it measures:** Actual V0 keys remaining in state trie
**How to check:**
```bash
curl -s -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"state_trieMigrationStatus","params":[]}' \
  http://127.0.0.1:9944 | jq '.result'
```

**Output example:**
```json
{
  "topRemainingToMigrate": 1828,
  "childRemainingToMigrate": 0,
  "totalTop": 2857304,
  "totalChild": 0
}
```

**Interpretation:**
- `topRemainingToMigrate`: 1,828 V0 keys still need migration
- `totalTop`: 2,857,304 keys total in trie
- **Progress:** 99.93% complete (1828 / 2857304)
- **This is the authoritative completion metric**
- Takes ~27 seconds to run (scans entire trie)

### Why The Discrepancy?

During the "stragglers phase" (final ~1% of migration):

- Most items are already in V1 format
- Bot finds and migrates remaining V0 "stragglers"
- Pallet counter increments by ~1024 per tx (items scanned)
- Node RPC decreases by ~0.4 per tx (items actually migrated)

**This is NORMAL behavior.** The bot is working correctly.

## Migration Phases

### Phase 1: Bulk Migration (~99%)

**Characteristics:**
- Fast progress
- Both metrics correlate
- ~1024 V0 items migrated per tx
- High throughput

**Monitoring:** Check pallet counter for activity

### Phase 2: Stragglers (~1%)

**Characteristics:**
- Slow progress
- Pallet counter ≫ node RPC decrease
- Special keys: `:code`, system pallets, complex child tries
- Low throughput per tx

**Monitoring:** Check node RPC for actual progress

## Monitoring Best Practices

### Active Session Monitoring

**Watch bot logs:**
```bash
tail -f migration.log | grep "Tx #"
```

**Expected output:**
```
INFO Tx #464 ✓
INFO Tx #465 ✓
INFO Tx #466 ✓
```

**Desktop notifications:**
- Transaction success: Every tx
- Node status: Every 10 transactions
- Dad jokes: Every 60 seconds (heartbeat)
- Critical errors: Immediate

### Periodic Progress Checks

**Check every 10-20 transactions:**
```bash
# Node RPC (authoritative)
ssh $SERVER "curl -s -H 'Content-Type: application/json' \
  -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"state_trieMigrationStatus\",\"params\":[]}' \
  http://127.0.0.1:9944 | jq '.result.topRemainingToMigrate'"
```

**Log historical progress:**
```bash
# Create progress log
echo "$(date +%s) $(curl -s ... | jq '.result.topRemainingToMigrate')" >> progress.log

# Calculate trend
tail -20 progress.log | awk '{print $2}' | gnuplot ...
```

### Health Checks

**Verify bot is running:**
```bash
# Should see recent timestamps
tail -1 migration.log
```

**Verify account balance unchanged:**
```bash
./westend-migrate --status | grep "Balance"
# Expected: Same balance across checks (migrations are free)
```

**Verify nonce incrementing:**
```bash
./westend-migrate --status | grep "Nonce"
# Expected: Nonce increases by 1 per successful transaction
```

## Troubleshooting

### Migration Appears Stuck

**Symptom:** Pallet counter increasing, node RPC not decreasing

**Diagnosis:**
```bash
# Check node RPC
curl -s ... | jq '.result.topRemainingToMigrate'

# Check bot logs
tail -50 migration.log | grep "Tx #"
```

**Resolution:**
- If node RPC is decreasing (even slowly): **Bot is working correctly** - stragglers phase
- If node RPC unchanged for 20+ transactions: **Possible issue** - check logs for errors

### SSH Connection Drops

**Symptom:** `run_remote.sh` reconnects frequently

**Diagnosis:**
```bash
# Check for stale sockets
ls -la ~/.ssh/sockets/

# Test connection
ssh -O check $SERVER
```

**Resolution:**
```bash
# Clear stale sockets
rm ~/.ssh/sockets/*

# Verify SSH config
grep -A5 "ControlMaster" ~/.ssh/config
```

**Expected SSH config:**
```
Host *
    ControlMaster auto
    ControlPath ~/.ssh/sockets/%r@%h-%p
    ControlPersist yes
    ServerAliveInterval 30
    ServerAliveCountMax 3
    TCPKeepAlive yes
```

### Node RPC Timeout

**Symptom:** `check_node_status()` returns empty result

**Diagnosis:**
```bash
# Test RPC directly
ssh $SERVER "curl -s --max-time 5 http://127.0.0.1:9944"
```

**Resolution:**
1. Increase timeout in `run_remote.sh`: `--max-time 45`
2. Reduce check frequency: `NODE_CHECK_INTERVAL=20`
3. Check node health: `ssh $SERVER "systemctl status polkadot"`

### Balance Decreased

**Symptom:** Bot reports balance decreased

**CRITICAL - Stop immediately:**
```bash
# Kill bot
pkill -f westend-migrate

# Check transaction history
# Visit: https://westend.subscan.io/account/YOUR_ADDRESS
```

**Diagnosis:**
- Migration txs should be FREE for controller account
- Balance decrease = slashing or dispatch error
- Review recent transactions for errors

**Resolution:**
1. Investigate cause (check on-chain events)
2. Verify account still has controller permissions
3. Check for chain runtime upgrades
4. Resume only after identifying and fixing issue

### Nonce Not Incrementing

**Symptom:** Bot submitting transactions but nonce stuck

**Diagnosis:**
```bash
# Check pending transactions
ssh $SERVER "curl -s -X POST -H 'Content-Type: application/json' \
  -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"author_pendingExtrinsics\",\"params\":[]}' \
  http://127.0.0.1:9944 | jq 'length'"
```

**Resolution:**
```bash
# Clear pending transactions
./westend-migrate --clear-pending --rpc-url ws://127.0.0.1:9944

# Resume migration
./run_remote.sh
```

## Performance Tuning

### Adjusting Check Intervals

**In run_remote.sh:**
```bash
# More frequent checks (higher overhead)
NODE_CHECK_INTERVAL=5

# Less frequent checks (lower overhead)
NODE_CHECK_INTERVAL=20
```

**Trade-offs:**
- Lower interval: Better visibility, more notifications
- Higher interval: Less noise, reduced RPC load

### Adjusting Migration Batch Size

**Default:** Chain maximum (usually 512 items, 51200 bytes)

**Reduce for slower nodes:**
```bash
./westend-migrate --runs 100 --item-limit 256 --size-limit 25600
```

**Benefits of smaller batches:**
- Faster dry-run validation
- Lower memory usage
- Better transaction pool handling

**Costs:**
- More transactions needed
- Slightly lower overall throughput

## Notifications Configuration

### Desktop Notifications Priority

**run_remote.sh notification strategy:**
```
Progress:      Normal priority, 5s timeout
Node status:   Normal priority, 8s timeout
Dad jokes:     Low priority, 8s timeout (doesn't interrupt)
Critical:      Critical priority, persistent (no timeout)
```

### Disabling Notifications

**For headless servers:**
```bash
# run_remote.sh automatically passes --no-notify
./run_remote.sh

# Manual run
ssh $SERVER 'SIGNER_SEED="..." ./westend-migrate --no-notify'
```

## Migration Completion

### How to Know When Done

**Check node RPC:**
```bash
curl ... | jq '.result.topRemainingToMigrate'
# Expected: 0

curl ... | jq '.result.childRemainingToMigrate'
# Expected: 0
```

**Bot will report:**
```
Migration is COMPLETE - all items migrated
```

### Post-Migration Steps

1. **Verify completion:**
   ```bash
   ./westend-migrate --status
   ```

2. **Stop bot:**
   ```bash
   # run_remote.sh will exit automatically
   # Or manually: pkill -f westend-migrate
   ```

3. **Archive logs:**
   ```bash
   tar czf migration-logs-$(date +%Y%m%d).tar.gz migration.log progress.log
   ```

4. **Runtime upgrade** (requires governance):
   - Remove `pallet-state-trie-migration` from runtime
   - Submit runtime upgrade proposal

## Logging

### Log Files

**migration.log:**
- All bot output
- Transaction results
- Error messages
- Dad joke heartbeats

**Format:**
```
[2025-12-15 04:30:15] Tx #464 ✓
[2025-12-15 04:30:45] Tx #465 ✓
[2025-12-15 04:30:52] 💓 Why don't scientists trust atoms? They make up everything!
[2025-12-15 04:31:20] 📊 Node status: 1828 top + 0 child keys remaining
```

### Log Analysis

**Count successful transactions:**
```bash
grep "Tx #.*✓" migration.log | wc -l
```

**Calculate average time per transaction:**
```bash
grep "Tx #.*✓" migration.log | \
  awk '{print $1, $2}' | \
  awk '{t[$1]=t[$1]+1} END {for(i in t) print i, t[i]}' | \
  sort
```

**Extract node status history:**
```bash
grep "📊 Node status" migration.log | \
  sed 's/.*: \([0-9]*\) top.*/\1/' > remaining.dat
```

## Security Considerations

### Seed Management

**NEVER:**
- Log seed to file
- Pass seed as CLI argument (shows in `ps`)
- Commit `.env` to git
- Share logs containing seed

**ALWAYS:**
- Use `SIGNER_SEED` environment variable
- Store seed in `.env` (gitignored)
- Use `secrecy::SecretString` in code (automatic zeroization)

### Network Security

**SSH tunnels:**
- Use SSH key authentication (no passwords)
- Enable `ControlMaster` for connection reuse
- Use `ServerAliveInterval` to detect dead connections

**RPC endpoints:**
- Prefer local node (`127.0.0.1:9944`)
- Use `wss://` for remote connections (encrypted)
- Avoid public RPC for production (rate limits, no unsafe methods)

### Account Security

**Controller account:**
- Balance should remain constant
- Monitor for unexpected transactions
- Rotate seed if compromised
- Use hardware wallet for high-value accounts

## Appendix: Useful Commands

### Quick Status Check
```bash
./westend-migrate --status --rpc-url ws://127.0.0.1:9944 2>&1 | \
  grep -E "(Status|Balance|Nonce)"
```

### Progress Percentage
```bash
REMAINING=$(curl -s ... | jq '.result.topRemainingToMigrate')
TOTAL=$(curl -s ... | jq '.result.totalTop')
echo "scale=2; (1 - $REMAINING / $TOTAL) * 100" | bc
```

### Estimated Time to Completion
```bash
# Based on recent progress
RATE=0.4  # items per transaction
TX_TIME=45  # seconds per transaction
REMAINING=$(curl -s ... | jq '.result.topRemainingToMigrate')
echo "Estimated hours: $(echo "scale=2; $REMAINING / $RATE * $TX_TIME / 3600" | bc)"
```

### SSH Connection Test
```bash
ssh -O check $SERVER 2>&1 | head -1
# Expected: "Master running (pid=XXXXX)"
```

### Clear All State
```bash
# Remove logs
rm migration.log progress.log

# Reset counter
echo "0" > /tmp/migration_tx_counter

# Clear SSH sockets
rm ~/.ssh/sockets/*
```

## References

- [DEV_NOTES.md](/home/nkpar/projects/cc-test-local/DEV_NOTES.md) - Implementation details
- [CLAUDE.md](/home/nkpar/projects/cc-test-local/CLAUDE.md) - Quick reference
- [STATE_TRIE_MIGRATION.md](/home/nkpar/projects/cc-test-local/STATE_TRIE_MIGRATION.md) - Technical background
- [Substrate RPC documentation](https://polkadot.js.org/docs/substrate/rpc/)
- [SSH multiplexing guide](https://en.wikibooks.org/wiki/OpenSSH/Cookbook/Multiplexing)
