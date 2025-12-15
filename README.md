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

### Using run_remote.sh

The included `run_remote.sh` script provides automated remote deployment with monitoring:

```bash
# Configure .env file
cat > .env <<EOF
SIGNER_SEED="your mnemonic phrase"
SERVER=your-ssh-alias
EOF

# Run continuous migration
./run_remote.sh

# Run specific number of migrations
./run_remote.sh 50
```

**Features:**
- Auto-reconnects on SSH connection loss
- Desktop notifications for progress/errors
- Dad joke heartbeat (confirms bot is alive)
- Periodic node-level status checks (every 10 transactions)

**SSH Config Requirements** (`~/.ssh/config`):
```
Host *
    ControlMaster auto
    ControlPath ~/.ssh/sockets/%r@%h-%p
    ControlPersist yes
    ServerAliveInterval 30
    TCPKeepAlive yes
```

## Monitoring Progress

### Two Different Metrics

**Important:** There are two ways to measure migration progress:

1. **Pallet Counter** (bot activity):
   ```bash
   ./westend-migrate --status
   ```
   Shows `top_items` - cumulative items processed (~1024 per tx)

2. **Node RPC** (actual progress):
   ```bash
   curl -s -H "Content-Type: application/json" \
     -d '{"jsonrpc":"2.0","id":1,"method":"state_trieMigrationStatus","params":[]}' \
     http://127.0.0.1:9944 | jq '.result'
   ```
   Shows `topRemainingToMigrate` - actual V0 keys left (authoritative)

**Migration phases:**
- **Bulk phase** (~99%): Fast progress, pallet and node metrics correlate
- **Stragglers phase** (final ~1%): Pallet counter increases but node RPC decreases slowly - this is NORMAL

The node RPC takes ~27 seconds to run (full trie scan), so check it periodically, not continuously.

## License

MIT
