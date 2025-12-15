# ADR-001: Dual-Metric Progress Tracking

**Date**: 2025-12-15
**Status**: Accepted

## Context

During operational deployment of the Westend state-trie migration bot, we discovered that the pallet's `MigrationProcess` storage counter does not accurately reflect actual migration progress. This became apparent in the "stragglers phase" when the bot's activity metrics diverged significantly from actual V0 keys remaining.

### Initial Assumptions (Incorrect)

We initially assumed:
1. `MigrationProcess.top_items` = number of items remaining to migrate
2. Progress could be tracked solely via pallet storage queries
3. Completion occurs when pallet reports all items processed

### Discovery

**Operational observation (Session 2025-12-15):**
```
Transaction range: Nonce 464 → 489 (25 transactions)
Pallet counter:    330,866 → 356,466 items (+25,600 items)
Node RPC:          1,838 → 1,828 remaining (-10 items)
Balance:           1009.96 WND (unchanged, confirming free transactions)
```

**Analysis:**
- Bot increments pallet counter by ~1024 items per transaction
- Actual V0 keys decrease by only ~0.4 items per transaction
- Discrepancy factor: ~2560x (pallet increment ÷ actual migration)

**Root cause:** Most items in the state trie were already migrated to V1 format. The bot's pallet counter tracks **items scanned**, not **items actually migrated**.

### Two Different APIs

#### 1. Pallet Storage: `MigrationProcess`

**Query:**
```rust
let query = subxt::dynamic::storage("StateTrieMigration", "MigrationProcess", vec![]);
let value = client.storage().at_latest().await?.fetch(&query).await?;
```

**Structure:**
```rust
MigrationTask {
    progress_top: Progress,      // LastKey(key) | Complete
    progress_child: Progress,    // LastKey(key) | Complete
    size: u32,                   // Total bytes processed
    top_items: u32,              // Cumulative items scanned
    child_items: u32,            // Cumulative child items scanned
}
```

**Characteristics:**
- Fast query (~50ms)
- Tracks bot activity (cumulative work done)
- Does NOT track items remaining
- Increments even when processing already-migrated items

#### 2. Node RPC: `state_trieMigrationStatus`

**Query:**
```bash
curl -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"state_trieMigrationStatus","params":[]}' \
  http://127.0.0.1:9944
```

**Response:**
```json
{
  "topRemainingToMigrate": 1828,
  "childRemainingToMigrate": 0,
  "totalTop": 2857304,
  "totalChild": 0
}
```

**Characteristics:**
- Slow query (~27 seconds - full trie scan)
- Authoritative source for actual V0 keys remaining
- Provides completion percentage: `(1 - remaining / total) * 100`
- Scans entire state trie to count V0 keys

## Options Considered

### Option 1: Rely Solely on Pallet Counter

**Approach:** Use `MigrationProcess.top_items` as the progress metric.

**Pros:**
- Fast to query (~50ms)
- No additional RPC calls needed
- Simple implementation

**Cons:**
- **Inaccurate in stragglers phase** (current state)
- Cannot determine actual completion percentage
- Users cannot estimate time to completion
- Misleading progress reports

**Rejected:** Insufficient accuracy for operational monitoring.

### Option 2: Rely Solely on Node RPC

**Approach:** Use `state_trieMigrationStatus` for all progress tracking.

**Pros:**
- Accurate progress metric
- Shows actual V0 keys remaining
- Provides completion percentage

**Cons:**
- Slow (~27 seconds per query)
- Cannot query on every transaction
- High overhead for continuous monitoring
- May timeout under load

**Rejected:** Too slow for frequent monitoring.

### Option 3: Dual-Metric System (SELECTED)

**Approach:** Use both metrics for different purposes:
- **Pallet counter:** Bot activity tracking (frequent, real-time)
- **Node RPC:** Actual progress tracking (periodic, authoritative)

**Pros:**
- Fast feedback on bot activity
- Accurate completion tracking
- Appropriate use of each API
- Efficient resource usage

**Cons:**
- More complex to explain to users
- Requires understanding two different metrics
- Need to document the difference clearly

**Selected:** Best balance of accuracy and performance.

## Decision

We will implement a **dual-metric progress tracking system**:

### Short-Term Monitoring (Pallet Counter)

**Use case:** Verify bot is actively working

**Query frequency:** Every transaction (real-time)

**Implementation:**
```rust
let status = parse_migration_status(&decoded);
info!("Status: top=wip/{} child=wip/{} size={}",
    status.top_items, status.child_items, status.size);
```

**User guidance:**
- Watch for incrementing `top_items` counter
- Confirms transactions are being submitted successfully
- Does NOT indicate completion percentage

### Long-Term Monitoring (Node RPC)

**Use case:** Track actual migration progress

**Query frequency:** Every 10-20 transactions (periodic)

**Implementation in `run_remote.sh`:**
```bash
NODE_CHECK_INTERVAL=10  # Check every N transactions

check_node_status() {
    result=$(ssh "$SERVER" "curl -s --max-time 35 \
        -H 'Content-Type: application/json' \
        -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"state_trieMigrationStatus\",\"params\":[]}' \
        http://127.0.0.1:9944")

    top_remaining=$(echo "$result" | grep -o '"topRemainingToMigrate":[0-9]*' | cut -d: -f2)
    notify-send "📊 Node status: ${top_remaining} keys remaining"
}

# Trigger check every 10 successful transactions
if (( tx_count % NODE_CHECK_INTERVAL == 0 )); then
    check_node_status &  # Background to not block
fi
```

**User guidance:**
- Check `topRemainingToMigrate` for actual progress
- Calculate completion: `(1 - remaining / total) * 100`
- Migration complete when `remaining == 0`

## Consequences

### Positive

1. **Accurate progress tracking** without sacrificing responsiveness
2. **Users can estimate completion time** using node RPC trends
3. **Bot activity visible** in real-time via pallet counter
4. **Efficient resource usage** (fast queries frequent, slow queries periodic)
5. **Clear operational metrics** for different time scales

### Negative

1. **User confusion** if not properly documented
   - Mitigation: Comprehensive docs in README.md, OPERATIONS.md, CLAUDE.md
2. **Complexity in monitoring scripts** (need to handle both APIs)
   - Mitigation: `run_remote.sh` automates dual-metric tracking
3. **Two sources of truth** can lead to contradictory interpretations
   - Mitigation: Clear documentation on when to use each metric

### Risks

1. **Node RPC may timeout** under heavy load
   - Mitigation: Timeout `--max-time 35`, retry logic, background execution
2. **Users may misinterpret pallet counter** as completion percentage
   - Mitigation: Explicit warnings in all documentation
3. **Node RPC performance may degrade** as state grows
   - Monitoring: Track RPC response times, adjust check interval if needed

## Implementation

### Documentation Updates

- [x] README.md: Added "Monitoring Progress" section with dual-metric explanation
- [x] CLAUDE.md: Added "Operational Insights" section
- [x] DEV_NOTES.md: Detailed session notes with discovery process
- [x] OPERATIONS.md: **NEW** - Comprehensive operational guide

### Code Changes

**run_remote.sh:**
```bash
# Added node status check function
check_node_status() { ... }

# Added periodic trigger
if (( tx_count % NODE_CHECK_INTERVAL == 0 )); then
    check_node_status &
fi
```

**No bot code changes needed** - bot already reports both metrics correctly.

### User Guidance

**Quick reference card added to README.md:**
```markdown
## Monitoring Progress

### Two Different Metrics

1. **Pallet Counter** (bot activity):
   Shows `top_items` - cumulative items processed

2. **Node RPC** (actual progress):
   Shows `topRemainingToMigrate` - V0 keys remaining (authoritative)
```

## Validation

### Metrics Collected (Session 2025-12-15)

**Data points:**
- 25 transactions observed
- Pallet counter increased linearly
- Node RPC decreased proportionally to actual work done
- No balance decrease (confirmed free transactions)

**Conclusion:** Dual-metric system accurately reflects both bot activity and actual migration progress.

### Performance Impact

**Pallet queries:** ~50ms each (negligible)
**Node RPC queries:** ~27 seconds each
- At 10-transaction intervals: ~27s every 7-10 minutes (acceptable)
- Background execution: No blocking of main bot loop

## Future Improvements

1. **Adaptive check interval:**
   ```bash
   # Decrease frequency as remaining count drops
   if (( remaining < 500 )); then
       NODE_CHECK_INTERVAL=5
   fi
   ```

2. **Historical trend analysis:**
   ```bash
   # Log node RPC results for ETA calculation
   echo "$(date +%s) $remaining" >> progress.log

   # Calculate migration rate
   tail -10 progress.log | awk '...' # Linear regression
   ```

3. **Alert thresholds:**
   ```bash
   # Notify if no progress in 50 transactions
   if (( tx_count - last_decrease > 50 )); then
       notify-send -u critical "Migration may be stuck"
   fi
   ```

## References

- [Substrate RPC documentation](https://polkadot.js.org/docs/substrate/rpc/#state)
- [DEV_NOTES.md Session 2025-12-15](/home/nkpar/projects/cc-test-local/DEV_NOTES.md#session-2025-12-15-operational-discovery)
- [OPERATIONS.md](/home/nkpar/projects/cc-test-local/OPERATIONS.md)
- [GitHub Issue - Initial investigation](https://github.com/nkpar/westend-migrate/commit/e1aae6b)

## Approval

**Approved by:** Development team
**Date:** 2025-12-15
**Implementation status:** Complete

## Amendments

None yet.
