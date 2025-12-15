# CLAUDE.md

Westend Asset Hub State-Trie Migration Bot - submits `continue_migrate` transactions to convert state from V0 to V1 trie format.

## Build & Run

```bash
cargo build --release
source .env && ./target/release/westend-migrate --runs 10
```

## Architecture

Uses subxt's **dynamic API** (not generated types):
- `subxt::dynamic::storage("StateTrieMigration", "MigrationProcess", vec![])`
- `subxt::dynamic::tx("StateTrieMigration", "continue_migrate", vec![...])`
- Type conversion: `decoded.map_context(|_| ())` converts `Value<TypeId>` → `Value<()>`

## Key Files

| File | Purpose |
|------|---------|
| `src/main.rs` | Bot logic, CLI, transaction submission |
| `src/utils.rs` | Helpers: notifications, error decoding, balance checks |
| `src/error.rs` | Custom error types |

## Critical Pitfalls

1. **Wait for finalization** - not just block inclusion
2. **Fresh signatures** - don't reuse signed transactions
3. **Balance verification** - migrations are free; decrease = slashing
4. **Never expose seeds** - use `.env` file
