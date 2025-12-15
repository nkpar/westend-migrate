# Westend State-Trie Migration Bot

Rust implementation of a state-trie migration bot for Westend Asset Hub. Submits `continue_migrate` transactions to convert blockchain state from V0 to V1 trie format.

Based on [Parity's TypeScript implementation](https://github.com/paritytech/polkadot-scripts/blob/master/src/services/state_trie_migration.ts) and the [HackMD Migration Guide](https://hackmd.io/@kizi/HyoSO3lf9).

## Features

- Dynamic subxt API for storage queries and transaction building
- `system_dryRun` validation (catches dispatch errors before submission)
- Transaction pool conflict handling (nonce monitoring, priority detection)
- `--no-notify` flag for headless server deployment
- `--runs N` for controlled batch migrations
- Balance verification after each transaction (slashing detection)

## Installation

```bash
cargo build --release
```

## Usage

```bash
# Set up environment
echo 'SIGNER_SEED="your mnemonic phrase"' > .env

# Run migrations
source .env && ./target/release/westend-migrate

# Run exactly N migrations
source .env && ./target/release/westend-migrate --runs 10

# Dry run (check status only)
source .env && ./target/release/westend-migrate --dry-run --once

# Show migration status
source .env && ./target/release/westend-migrate --status
```

## CLI Options

| Flag | Description |
|------|-------------|
| `--rpc-url` | Westend RPC endpoint (default: public RPC) |
| `--runs N` | Submit exactly N migrations then exit |
| `--once` | Run single migration and exit |
| `--dry-run` | Check status only, don't submit transactions |
| `--status` | Show migration progress and exit |
| `--no-notify` | Disable desktop notifications |
| `--item-limit` | Items per transaction (0 = chain max) |
| `--size-limit` | Bytes per transaction (0 = chain max) |
| `--clear-pending` | Clear stuck transactions before starting |

## Server Deployment

For best results, run on a server with a local full node that has `--rpc-methods=unsafe` enabled:

```bash
# Deploy
scp ./target/release/westend-migrate server:~/

# Run with local RPC
ssh server 'SIGNER_SEED="..." ./westend-migrate --runs 10 --rpc-url ws://127.0.0.1:9944 --no-notify'
```

## License

MIT
