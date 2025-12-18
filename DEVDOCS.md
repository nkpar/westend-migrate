# Developer Documentation - Westend State-Trie Migration Bot

**Last Updated:** 2025-12-15

Comprehensive reference for development and operations. For getting started, see [README.md](README.md).

## Table of Contents

- [Architecture](#architecture)
- [Key Code Patterns](#key-code-patterns)
- [Operations Guide](#operations-guide)
- [Understanding Progress Metrics](#understanding-progress-metrics)
- [Monitoring Best Practices](#monitoring-best-practices)
- [Troubleshooting](#troubleshooting)
- [Security](#security)
- [Performance](#performance)
- [Testing](#testing)
- [References](#references)

---

## Architecture

### Core Components

| File | Purpose |
|------|---------|
| `src/main.rs` | Core loop: Query → Dry Run → Submit → Monitor |
| `src/utils.rs` | Helpers: notifications, error decoding, balance checks |
| `src/error.rs` | Custom error types (`PoolConflict`, `NonceStale`, etc.) |
| `run_remote.sh` | Automated remote deployment with monitoring |
| `monitor.py` | Local monitoring script with desktop notifications |
| `justfile` | Quick deployment commands |

### Transaction Flow

```
1. Query State     → Fetches MigrationProcess from StateTrieMigration pallet
2. Construct Witness → Converts Value<TypeId> → Value<()> using map_context
3. Dry Run        → Executes system_dryRun (requires --rpc-methods=unsafe)
4. Submit         → Signs and submits via subxt::dynamic
5. Verify         → Waits for finalization, checks balance
```

### Error Handling

The bot uses 16 structured error types with recoverability detection:

- **Recoverable:** Network timeouts, temporary RPC failures
- **Non-recoverable:** Invalid seed, balance decrease, max retries exceeded

---

## Key Code Patterns

### Witness Task Construction

```rust
let decoded = thunk.to_value()?;                    // Value<TypeId>
let witness_task = decoded.map_context(|_| ());     // Value<()>
// Pass directly to tx builder
```

### Finalization Wait

**Anti-pattern:** Breaking on `InBestBlock` (state not finalized)

```rust
// Correct pattern
match status {
    InBestBlock(_) => {
        info!("Included, waiting for finalization...");
        // DON'T break - keep watching
    }
    InFinalizedBlock(_) => {
        info!("FINALIZED");
        break;  // NOW it's safe
    }
}
```

### Double-Signing for Dry Run

Dry run takes time; original tx becomes stale (AncientBirthBlock error):

```rust
// Sign for dry run
let dry_run_tx = client.tx().create_signed(&tx, &signer, Default::default()).await?;
rpc.dry_run(dry_run_tx.encoded(), None).await?;

// Sign FRESH for submission
let fresh_tx = client.tx().create_signed(&tx, &signer, Default::default()).await?;
fresh_tx.submit_and_watch().await?;
```

### Seed Protection

```rust
#[arg(long, env = "SIGNER_SEED")]
seed: SecretString,

// Brief exposure in controlled scope
let signer = {
    let seed_str = config.seed.expose_secret();
    // Parse seed...
};  // Zeroized here
```

---

## Operations Guide

### Quick Start

```bash
# 1. Configure environment
cat > .env <<EOF
SIGNER_SEED="your mnemonic phrase"
SIGNER_ACCOUNT="5YourAccountAddress..."
SERVER=your-ssh-alias
EOF

# 2. Build and deploy
cargo build --release
scp ./target/release/westend-migrate $SERVER:~/

# 3. Run with monitoring (using justfile)
just deploy        # Build, copy to server, restart
just status        # Check bot status and nonce
just follow        # Follow remote bot logs
```

### Justfile Commands

| Command | Description |
|---------|-------------|
| `just monitor` | Start local monitor (notifications) |
| `just status` | Check bot, nonce, keys remaining |
| `just qs` | Quick status (nonce only, fast) |
| `just log` | View remote bot log (last 50 lines) |
| `just follow` | Follow remote bot log in real-time |
| `just deploy` | Build, copy to remote, restart |
| `just fresh` | Full restart with SSH socket cleanup |

### SSH Configuration

Required in `~/.ssh/config`:

```
Host *
    ControlMaster auto
    ControlPath ~/.ssh/sockets/%r@%h-%p
    ControlPersist yes
    ServerAliveInterval 30
    ServerAliveCountMax 3
    TCPKeepAlive yes
```

If you see "mux_client_request_session" errors: `rm ~/.ssh/sockets/*`

---

## Understanding Progress Metrics

### Two Different Systems

**Critical:** There are two progress metrics that measure different things.

#### 1. Pallet Counter (Fast, Bot Activity)

```bash
./westend-migrate --status --rpc-url ws://127.0.0.1:9944
```

Output: `Status: top=wip/356466 child=wip/0 size=27836416`

- `top_items`: Cumulative items scanned (NOT remaining)
- Increments ~1024 per transaction
- Use to verify bot is working

#### 2. Node RPC (Slow, Authoritative)

```bash
curl -s -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"state_trieMigrationStatus","params":[]}' \
  http://127.0.0.1:9944 | jq '.result'
```

Output:
```json
{
  "topRemainingToMigrate": 1828,
  "childRemainingToMigrate": 0,
  "totalTop": 2857304
}
```

- `topRemainingToMigrate`: Actual V0 keys remaining
- Progress: `(1 - remaining / total) * 100`
- Takes ~27 seconds (full trie scan)
- **This is the authoritative completion metric**

### Migration Phases

| Phase | Progress | Behavior |
|-------|----------|----------|
| **Bulk** | ~99% | Fast, metrics correlate, ~1024 items/tx |
| **Stragglers** | Final ~1% | Slow, metrics diverge, ~0.4 items/tx |

In stragglers phase, pallet counter keeps incrementing but node RPC decreases slowly. **This is NORMAL.**

See [ADR-001](docs/ADR-001-dual-metrics.md) for the full analysis.

---

## Monitoring Best Practices

### Real-Time (Every Transaction)

```bash
# Watch bot logs
tail -f migration.log | grep "Tx #"

# Expected output
INFO Tx #464 ✓
INFO Tx #465 ✓
```

### Periodic (Every 10-20 Transactions)

```bash
# Node RPC check (authoritative)
just status

# Or manually
ssh $SERVER "curl -s ... | jq '.result.topRemainingToMigrate'"
```

### Health Checks

- **Bot running:** Recent timestamps in logs
- **Balance unchanged:** Migrations are free
- **Nonce incrementing:** +1 per successful tx

### Notifications

| Event | Priority | Timeout |
|-------|----------|---------|
| Transaction success | Normal | 4s |
| Node status | Normal | 8s |
| Dad jokes | Low | 8s |
| Critical errors | Critical | Persistent |

---

## Troubleshooting

| Symptom | Diagnosis | Resolution |
|---------|-----------|------------|
| Pallet counter up, node RPC flat | Stragglers phase | Normal - use node RPC as authority |
| SSH connection drops | Stale control sockets | `rm ~/.ssh/sockets/*` |
| Node RPC timeout | Node busy | Increase `--max-time`, reduce check frequency |
| Balance decreased | Slashing or error | **STOP** - investigate on-chain events |
| Nonce stuck | Pending tx in pool | Use `--clear-pending` flag |

### Balance Decreased (CRITICAL)

```bash
# Kill bot immediately
pkill -f westend-migrate

# Check transaction history
# Visit: https://westend.subscan.io/account/YOUR_ADDRESS
```

Migration transactions should be FREE. Any balance decrease = slashing or dispatch error.

### SSH Connection Issues

```bash
# Check for stale sockets
ls -la ~/.ssh/sockets/

# Test connection
ssh -O check $SERVER

# Clear and retry
rm ~/.ssh/sockets/*
just fresh
```

---

## Security

### Seed Management

**NEVER:**
- Log seed to file
- Pass seed as CLI argument (shows in `ps`)
- Commit `.env` to git
- Share logs containing seed

**ALWAYS:**
- Use `SIGNER_SEED` environment variable
- Store seed in `.env` (gitignored)
- Use `secrecy::SecretString` in code

### Network Security

- SSH key authentication (no passwords)
- Local RPC preferred (`127.0.0.1:9944`)
- WSS encryption for remote connections
- Avoid public RPC (rate limits, no unsafe methods)

---

## Performance

### Transaction Lifecycle

```
Query storage:     ~50ms
Build tx:          ~10ms
Sign tx:           ~50ms
Dry run:           ~200ms
Submit:            ~100ms
Inclusion:         ~6s (1 block)
Finalization:      ~30s (5-6 blocks)
─────────────────────────
Total:             ~37 seconds per transaction
```

### Resource Usage

- Memory: ~50MB (bot)
- CPU: Minimal (<1% avg)
- Network: ~10KB per transaction
- Disk: ~1MB logs per day

### Tuning

```bash
# Smaller batches for slower nodes
./westend-migrate --runs 100 --item-limit 256 --size-limit 25600

# Tested max limits (used for fast completion)
--item-limit 30720 --size-limit 3072000
```

---

## Testing

### Unit Tests

29 tests covering:
- SCALE validity error decoding (13 variants)
- Migration status parsing
- Balance verification
- Error type classification
- RPC error parsing

```bash
cargo test
```

### Integration Testing

Manual operational testing via extended runs on Westend and Kusama Asset Hub.

---

## References

### Internal

- [README.md](README.md) - User-facing quickstart
- [CLAUDE.md](CLAUDE.md) - Quick reference for Claude Code
- [ADR-001](docs/ADR-001-dual-metrics.md) - Dual-metric decision record

### External

- [HackMD Migration Guide](https://hackmd.io/@kizi/HyoSO3lf9)
- [Parity TypeScript Bot](https://github.com/paritytech/polkadot-scripts/blob/master/src/services/state_trie_migration.ts)
- [Substrate RPC Docs](https://polkadot.js.org/docs/substrate/rpc/#state)
- [SSH Multiplexing Guide](https://en.wikibooks.org/wiki/OpenSSH/Cookbook/Multiplexing)

---

## Changelog

### 2025-12-17
- Added `monitor.py` Python monitoring script
- Added `justfile` for quick deployment commands
- Fixed completion detection (check `top_complete` only)
- Added dry-run nonce retry loop

### 2025-12-15
- Discovered dual-metric discrepancy (see ADR-001)
- Implemented node RPC status checks
- Fixed SSH multiplexing issues
- Added 29 unit tests
- Implemented `secrecy::SecretString` for seed protection

### 2025-12-14
- Added `--runs N` flag
- Implemented double-signing for dry-run freshness
- Added dad joke heartbeat system
- Public release
