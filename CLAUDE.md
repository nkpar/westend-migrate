# CLAUDE.md

Westend Asset Hub State-Trie Migration Bot - submits `continue_migrate` transactions to convert state from V0 to V1 trie format.

## Build & Run

```bash
# Build
cargo build --release

# Run (requires seed in environment)
source .env && ./target/release/westend-migrate --runs 10

# Utilities
./target/release/list_pallets  # Check available pallets
```

## Architecture

1. **Query State**: Fetches `MigrationProcess` from `StateTrieMigration` pallet.
2. **Construct Witness**: Converts storage value `Value<TypeId>` → `Value<()>` using `decoded.map_context(|_| ())`.
3. **Dry Run**: Executes `system_dryRun` (requires `--rpc-methods=unsafe` on node) to catch `SizeUpperBoundExceeded` before signing.
4. **Submit**: Signs and submits transaction using `subxt::dynamic`.
5. **Verify**: Waits for **Finalization** and checks for balance decrease (slashing detection).

## Key Files

| File | Purpose |
|------|---------|
| `src/main.rs` | Core loop: Query -> Dry Run -> Submit -> Monitor |
| `src/utils.rs` | Helpers: notifications, error decoding, balance checks |
| `src/error.rs` | Custom error types (`PoolConflict`, `NonceStale`, etc.) |
| `src/bin/list_pallets.rs` | Utility to list chain pallets for verification |

## Critical Pitfalls & Implementation Details

1. **Unsafe RPC Required**: The bot relies on `system_dryRun` for safety. The connected node MUST have `--rpc-methods=unsafe`.
2. **Finalization vs Inclusion**: State changes are only reliable after *finalization*. The bot waits for `TxStatus::InFinalizedBlock`.
3. **Transaction Pool Conflicts**: 
   - Error `1014` (Priority too low) or `1010` (Invalid Transaction) often means a stuck pending transaction.
   - Bot monitors nonce changes to resolve these instead of blinding retrying.
4. **Stale Nonces**: "AncientBirthBlock" errors occur if dry-run takes too long. The bot re-signs transactions after dry-run to ensure freshness.
5. **Balance Verification**: Migrations should be free. Any balance decrease indicates slashing/error.
6. **Security**: Never expose seeds in CLI args; use `SIGNER_SEED` env var.