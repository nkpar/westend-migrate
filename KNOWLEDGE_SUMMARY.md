# Knowledge Summary - Westend State-Trie Migration Bot

**Last Updated:** 2025-12-15
**Project:** Westend Asset Hub State-Trie Migration Bot
**Repository:** https://github.com/nkpar/westend-migrate

This document provides a high-level summary of the project's current state, key learnings, and operational knowledge.

## Project Status

### Current State (2025-12-15)

**Migration Progress:** 99.93% complete (stragglers phase)
- Node RPC: 1,828 V0 keys remaining out of 2,857,304 total
- Pallet counter: ~356,466 items processed (cumulative work)
- Account balance: Stable at ~1009.96 WND (no slashing)
- Nonce progression: ~489 transactions successfully finalized

**Operational Mode:** Running via `run_remote.sh` on remote server with local node access

**Performance:** ~1 transaction per 30-60 seconds (including finalization wait)

## Key Architectural Components

### Core Bot (`src/main.rs`)
- Dynamic subxt API for storage queries and transactions
- Dry-run validation (requires `--rpc-methods=unsafe`)
- Finalization wait (not just inclusion)
- Balance verification (detects slashing)
- Nonce conflict resolution
- Dad joke heartbeat (connectivity confirmation)

### Error Handling (`src/error.rs`)
- 16 structured error types
- Recoverability detection
- Pool conflict handling
- RPC error parsing

### Utilities (`src/utils.rs`)
- SCALE-encoded validity error decoder
- Balance verification helpers
- Migration status parser
- Desktop notification system (with disable flag)

### Deployment Script (`run_remote.sh`)
- SSH multiplexing with connection persistence
- Auto-reconnect on connection loss
- Desktop notifications for progress
- Periodic node RPC status checks (every 10 transactions)
- File-based transaction counter
- Background async status checks

## Critical Operational Discoveries

### 1. Dual-Metric Progress System

**The Problem:** Pallet counter and actual progress diverge significantly in stragglers phase.

**The Solution:** Use both metrics for different purposes:
- **Pallet `top_items`**: Bot activity tracking (fast, real-time)
- **Node RPC `topRemainingToMigrate`**: Actual progress (slow, authoritative)

**Implementation:** See [ADR-001](/home/nkpar/projects/cc-test-local/docs/ADR-001-dual-metrics.md)

**Observed metrics:**
```
25 transactions (nonces 464 → 489)
Pallet counter:  +25,600 items
Node RPC:        -10 V0 keys
Ratio:           2560:1 (work done : actual migration)
```

### 2. Migration Phases

**Bulk Phase (~99%):**
- Fast progress
- Metrics correlate
- Standard batching
- ~1024 V0 items per tx

**Stragglers Phase (final ~1%):**
- Slow progress
- Metrics diverge
- Special keys (`:code`, system pallets)
- ~0.4 V0 items per tx (most items already V1)

### 3. SSH Multiplexing Requirements

**Problem:** Stale control sockets cause "mux_client_request_session" errors

**Solution:** Enhanced SSH config with:
```
ControlMaster auto
ControlPath ~/.ssh/sockets/%r@%h-%p
ControlPersist yes
ServerAliveInterval 30
ServerAliveCountMax 3
TCPKeepAlive yes
```

**Benefits:**
- Connection reuse
- Automatic reconnection
- Reduced latency for repeated commands

### 4. Node RPC Performance

**Method:** `state_trieMigrationStatus`
**Duration:** ~27 seconds (full state trie scan)
**Frequency:** Every 10-20 transactions (not continuous)
**Implementation:** Background execution to avoid blocking

## Documentation Structure

### For Developers

1. **[DEV_NOTES.md](/home/nkpar/projects/cc-test-local/DEV_NOTES.md)**
   - Complete session-by-session development log
   - Implementation challenges and solutions
   - Code patterns and anti-patterns
   - Unit testing details
   - Security improvements (seed protection)

2. **[src/error.rs](/home/nkpar/projects/cc-test-local/src/error.rs)**
   - Structured error types
   - Recovery strategies
   - RPC error parsing

3. **[src/utils.rs](/home/nkpar/projects/cc-test-local/src/utils.rs)**
   - Comprehensive unit tests (29 tests total)
   - SCALE validity error decoder
   - Balance verification utilities

### For Operations

1. **[OPERATIONS.md](/home/nkpar/projects/cc-test-local/OPERATIONS.md)** ⭐ PRIMARY OPERATIONAL GUIDE
   - Quick start instructions
   - Dual-metric system explained
   - Monitoring best practices
   - Troubleshooting guide
   - Performance tuning
   - Security considerations

2. **[run_remote.sh](/home/nkpar/projects/cc-test-local/run_remote.sh)**
   - Automated remote deployment
   - Notification system
   - Node status checks
   - Connection management

3. **[CLAUDE.md](/home/nkpar/projects/cc-test-local/CLAUDE.md)**
   - Quick reference for Claude Code
   - Key pitfalls and workarounds
   - Operational insights summary
   - Monitoring commands

### For Understanding

1. **[STATE_TRIE_MIGRATION.md](/home/nkpar/projects/cc-test-local/STATE_TRIE_MIGRATION.md)**
   - V0 vs V1 trie format explanation
   - Migration architecture overview
   - Current status with both metrics
   - Monitoring section with discrepancy explanation

2. **[README.md](/home/nkpar/projects/cc-test-local/README.md)**
   - User-facing documentation
   - Installation and usage
   - CLI options
   - run_remote.sh features
   - Dual-metric monitoring

3. **[docs/ADR-001-dual-metrics.md](/home/nkpar/projects/cc-test-local/docs/ADR-001-dual-metrics.md)**
   - Architecture decision record
   - Context and discovery process
   - Options considered
   - Implementation details
   - Consequences and risks

## Key Code Patterns

### 1. Witness Task Construction

**Challenge:** TypeScript passes storage value directly, Rust needs type conversion

**Solution:**
```rust
let decoded = thunk.to_value()?;                    // Value<TypeId>
let witness_task = decoded.map_context(|_| ());     // Value<()>
// Pass directly to tx builder
```

### 2. Finalization Wait

**Anti-pattern:** Breaking on `InBestBlock` (state not finalized)

**Correct pattern:**
```rust
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

### 3. Double-Signing for Dry Run

**Problem:** Dry run takes time, original tx becomes stale (AncientBirthBlock)

**Solution:**
```rust
// Sign for dry run
let dry_run_tx = client.tx().create_signed(&tx, &signer, Default::default()).await?;
rpc.dry_run(dry_run_tx.encoded(), None).await?;

// Sign FRESH for submission
let fresh_tx = client.tx().create_signed(&tx, &signer, Default::default()).await?;
fresh_tx.submit_and_watch().await?;
```

### 4. Background Status Checks

**run_remote.sh pattern:**
```bash
export -f check_node_status  # Allow function use in SSH subshells

if (( tx_count % NODE_CHECK_INTERVAL == 0 )); then
    check_node_status &  # Background - doesn't block log parsing
fi
```

## Security Highlights

### Seed Protection

**Implementation:** `secrecy::SecretString` with automatic zeroization

**Usage:**
```rust
#[arg(long, env = "SIGNER_SEED")]
seed: SecretString,

// Brief exposure in controlled scope
let signer = {
    let seed_str = config.seed.expose_secret();
    // Parse seed...
};  // Zeroized here
```

**Benefits:**
- Memory protection
- Debug output shows `SecretString(...)` not actual value
- Prevents accidental logging

### Network Security

- SSH key authentication (no passwords)
- ControlMaster for connection reuse
- Local RPC preferred (`127.0.0.1:9944`)
- WSS encryption for remote connections

## Monitoring Strategy

### Real-Time (Every Transaction)

**Watch:** Pallet counter via bot logs
```bash
tail -f migration.log | grep "Tx #"
```

**Confirms:** Bot is actively submitting transactions

### Periodic (Every 10-20 Transactions)

**Check:** Node RPC via curl
```bash
curl -s ... | jq '.result.topRemainingToMigrate'
```

**Confirms:** Actual migration progress

### Alerting

**Desktop notifications:**
- Progress: Every successful tx
- Node status: Every 10 transactions
- Dad jokes: Every 60 seconds (heartbeat)
- Critical: Balance decrease, connection loss

## Troubleshooting Quick Reference

| Symptom | Diagnosis | Resolution |
|---------|-----------|------------|
| Pallet counter up, node RPC flat | Stragglers phase | Normal - use node RPC as authority |
| SSH connection drops | Stale control sockets | `rm ~/.ssh/sockets/*` |
| Node RPC timeout | Node busy or rate-limited | Increase `--max-time`, reduce check frequency |
| Balance decreased | Slashing or error | STOP - investigate on-chain events |
| Nonce stuck | Pending transaction in pool | Use `--clear-pending` flag |

## Performance Metrics

**Transaction lifecycle:**
```
Query storage:     ~50ms
Build tx:          ~10ms
Sign tx:           ~50ms
Dry run:           ~200ms (if available)
Submit:            ~100ms
Inclusion:         ~6s (1 block)
Finalization:      ~30s (5-6 blocks)
Total:             ~37 seconds per transaction
```

**Resource usage:**
- Memory: ~50MB (bot)
- CPU: Minimal (<1% avg, spikes during signing)
- Network: ~10KB per transaction
- Disk: Log files (~1MB per day)

## Future Improvements

### Monitoring Enhancements

1. **Adaptive check interval** based on remaining count
2. **Historical trend analysis** for ETA calculation
3. **Alert thresholds** for stuck migration detection
4. **Progress graph generation** from logs

### Performance Optimizations

1. **Batch size tuning** based on node response times
2. **Parallel dry-run validation** (if multiple nodes available)
3. **Transaction pipelining** (sign next while waiting for finalization)

### Operational Features

1. **Automated recovery** from common error states
2. **Health metrics export** (Prometheus format)
3. **Slack/Discord integration** for notifications
4. **Web dashboard** for real-time monitoring

## Testing Coverage

**Unit tests:** 29 tests total
- `utils.rs`: 22 tests (validity errors, status parsing, balance checks)
- `error.rs`: 5 tests (recoverability, RPC parsing, display)
- All passing as of 2025-12-15

**Integration testing:** Manual operational testing via extended runs

**Coverage areas:**
- SCALE validity error decoding (13 variants)
- Migration status parsing
- Balance verification
- Error type classification
- RPC error parsing

## References

### Internal Documentation
- [OPERATIONS.md](/home/nkpar/projects/cc-test-local/OPERATIONS.md) - Primary operational guide
- [DEV_NOTES.md](/home/nkpar/projects/cc-test-local/DEV_NOTES.md) - Complete development log
- [ADR-001](/home/nkpar/projects/cc-test-local/docs/ADR-001-dual-metrics.md) - Dual-metric decision record

### External Resources
- [HackMD Migration Guide](https://hackmd.io/@kizi/HyoSO3lf9)
- [Parity TypeScript Bot](https://github.com/paritytech/polkadot-scripts/blob/master/src/services/state_trie_migration.ts)
- [Substrate RPC Docs](https://polkadot.js.org/docs/substrate/rpc/#state)
- [SSH Multiplexing Guide](https://en.wikibooks.org/wiki/OpenSSH/Cookbook/Multiplexing)

## Changelog

### 2025-12-15: Operational Discovery Session
- Discovered dual-metric discrepancy
- Implemented node RPC status checks in `run_remote.sh`
- Fixed SSH multiplexing issues
- Created comprehensive documentation suite
- Added [ADR-001](/home/nkpar/projects/cc-test-local/docs/ADR-001-dual-metrics.md)
- Created [OPERATIONS.md](/home/nkpar/projects/cc-test-local/OPERATIONS.md)

### 2025-12-15: Code Quality Improvements
- Added 29 unit tests
- Implemented `secrecy::SecretString` for seed protection
- Consolidated error types in `src/error.rs`
- Added balance verification utilities

### 2025-12-14: Server Deployment Enhancements
- Added `--runs N` flag
- Implemented double-signing for dry-run freshness
- Improved log formatting with `SecsTimer`
- Added dad joke heartbeat system

### 2025-12-14: Public Release
- Made repository public
- Added `--no-notify` flag
- Security cleanup (removed credentials)
- Branch protection enabled

## Contributors

- **Primary Developer:** nkpar
- **AI Assistant:** Claude Code (Anthropic)

## License

MIT
