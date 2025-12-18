//! Westend State-Trie Migration Bot
//!
//! This bot submits signed migration transactions to help complete
//! the V0 → V1 state trie migration on Westend.
//!
//! Based on: https://hackmd.io/@kizi/HyoSO3lf9
//!
//! Mirrors the TypeScript implementation:
//! const currentTask = await api.query.stateTrieMigration.migrationProcess();
//! const tx = api.tx.stateTrieMigration.continueMigrate(limits, sizeUpperLimit, currentTask);

mod error;
mod utils;

use anyhow::{Context, Result};
use clap::Parser;
use error::MigrationError;
use secrecy::{ExposeSecret, SecretString};
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use fs2::FileExt;
use subxt::{
    backend::{legacy::LegacyRpcMethods, rpc::RpcClient},
    dynamic::{At, Value},
    rpc_params,
    tx::Signer,
    OnlineClient, PolkadotConfig,
};
use subxt_signer::{bip39::Mnemonic, sr25519::Keypair};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use utils::{
    check_balance_decrease, decode_validity_error, disable_notifications, fetch_dad_joke,
    parse_migration_status, send_notification, MigrationStatus, ValidityError,
};

const DEFAULT_WESTEND_RPC: &str = "wss://westend-asset-hub-rpc.polkadot.io";

// Timing constants
const BLOCK_TIME_SECS: u64 = 6;
const PENDING_TX_TIMEOUT_ITERATIONS: u32 = 20;
const NONCE_RETRY_WAIT_SECS: u64 = 30;
const RETRY_WAIT_SECS: u64 = 12;
const BANNED_TX_WAIT_SECS: u64 = 60;
const HEARTBEAT_INTERVAL_SECS: u64 = 60;
const MAX_CONSECUTIVE_ERRORS: u32 = 5; // Stop after this many consecutive failures

/// Timer showing local date/time
struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let now = chrono::Local::now();
        write!(w, "{}", now.format("%m-%d %H:%M:%S"))
    }
}

/// State-trie migration bot for Westend
#[derive(Parser)]
#[command(name = "westend-migrate")]
#[command(about = "Bot to run signed state-trie migration on Westend")]
struct Cli {
    /// Westend RPC endpoint
    #[arg(short, long, default_value = DEFAULT_WESTEND_RPC, env = "WESTEND_RPC")]
    rpc_url: String,

    /// Secret seed phrase or hex seed for signing transactions.
    /// The seed is stored in memory-protected storage and zeroized on drop.
    /// WARNING: Use environment variable SIGNER_SEED for security
    #[arg(long, env = "SIGNER_SEED")]
    seed: SecretString,

    /// Number of items to migrate per transaction (0 = use chain max)
    #[arg(long, default_value = "0")]
    item_limit: u32,

    /// Size limit in bytes per transaction (0 = use chain max)
    #[arg(long, default_value = "0")]
    size_limit: u32,

    /// Delay between migration transactions (seconds)
    #[arg(long, default_value = "0")]
    delay_secs: u64,

    /// Run once and exit (don't loop)
    #[arg(long)]
    once: bool,

    /// Number of successful migrations to submit before exiting (0 = unlimited)
    #[arg(long, default_value = "0")]
    runs: u32,

    /// Dry run - check status only, don't submit transactions
    #[arg(long)]
    dry_run: bool,

    /// Show migration status and pending transactions, then exit
    #[arg(long)]
    status: bool,

    /// Clear pending transactions from the pool before starting
    #[arg(long)]
    clear_pending: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Disable desktop notifications (useful for headless servers)
    #[arg(long)]
    no_notify: bool,
}

struct MigrationBot {
    client: OnlineClient<PolkadotConfig>,
    rpc: LegacyRpcMethods<PolkadotConfig>,
    raw_rpc: RpcClient,
    signer: Keypair,
    config: Cli,
    dry_run_supported: AtomicBool,
    shutdown: CancellationToken,
}

impl MigrationBot {
    async fn new(config: Cli) -> Result<Self> {
        info!("Connecting to {}", config.rpc_url);

        // Create RPC client for dry_run calls
        let rpc_client = RpcClient::from_url(&config.rpc_url).await.map_err(|e| {
            MigrationError::ConnectionFailed(format!("Failed to create RPC client: {}", e))
        })?;
        let rpc = LegacyRpcMethods::<PolkadotConfig>::new(rpc_client.clone());

        // Create OnlineClient from the same RPC client
        let client = OnlineClient::<PolkadotConfig>::from_rpc_client(rpc_client.clone())
            .await
            .map_err(|e| {
                MigrationError::ConnectionFailed(format!("Failed to connect to Westend: {}", e))
            })?;

        let genesis = client.genesis_hash();
        info!("Connected to chain with genesis: {:?}", genesis);

        // Parse the seed from SecretString (zeroizes on drop)
        // We expose the secret briefly only during parsing, then it's protected in the Keypair
        let signer = {
            let seed_str = config.seed.expose_secret();
            if seed_str.starts_with("0x") {
                // Hex seed - use zeroizing buffer
                let seed_bytes = hex::decode(seed_str.trim_start_matches("0x"))
                    .map_err(|e| MigrationError::InvalidSeed(format!("Invalid hex: {}", e)))?;
                let seed_array: [u8; 32] = seed_bytes.try_into().map_err(|_| {
                    MigrationError::InvalidSeed("Seed must be 32 bytes".to_string())
                })?;
                // Note: seed_array will be zeroized by scope exit
                Keypair::from_secret_key(seed_array)
                    .map_err(|e| MigrationError::InvalidSeed(format!("Invalid seed: {:?}", e)))?
            } else {
                // Mnemonic phrase
                let mnemonic = Mnemonic::parse(seed_str).map_err(|e| {
                    MigrationError::InvalidSeed(format!("Invalid mnemonic: {:?}", e))
                })?;
                Keypair::from_phrase(&mnemonic, None).map_err(|e| {
                    MigrationError::InvalidSeed(format!("Failed to derive: {:?}", e))
                })?
            }
        };

        let account_id = <Keypair as Signer<PolkadotConfig>>::account_id(&signer);
        info!("Using account: {}", account_id);

        Ok(Self {
            client,
            rpc,
            raw_rpc: rpc_client,
            signer,
            config,
            dry_run_supported: AtomicBool::new(true), // Assume supported until proven otherwise
            shutdown: CancellationToken::new(),
        })
    }

    /// Query current migration task from storage
    /// Returns both the raw Value (for tx) and parsed status (for display)
    async fn get_migration_task(&self) -> Result<Option<(Value<()>, MigrationStatus)>> {
        // Query MigrationProcess - this is what we pass to continue_migrate
        let progress_query =
            subxt::dynamic::storage("StateTrieMigration", "MigrationProcess", vec![]);

        let task_thunk = self
            .client
            .storage()
            .at_latest()
            .await?
            .fetch(&progress_query)
            .await?;

        match task_thunk {
            Some(thunk) => {
                // Get the decoded value for inspection
                let decoded = thunk.to_value()?;

                // Parse status for display
                let status = parse_migration_status(&decoded);

                // Convert Value<TypeId> to Value<()> for use in transaction
                // This is the key - we pass the queried value directly like TypeScript does
                let witness_task = decoded.map_context(|_| ());

                Ok(Some((witness_task, status)))
            }
            None => {
                info!("No migration progress found - migration may not be active");
                Ok(None)
            }
        }
    }

    /// Set SignedMigrationMaxLimits on chain (requires controller permission)
    async fn set_max_limits(&self, size: u32, item: u32) -> Result<()> {
        let limits = Value::named_composite([
            ("size", Value::u128(size as u128)),
            ("item", Value::u128(item as u128)),
        ]);

        let tx = subxt::dynamic::tx("StateTrieMigration", "set_signed_max_limits", vec![limits]);

        let signed_tx = self
            .client
            .tx()
            .create_signed(&tx, &self.signer, Default::default())
            .await
            .context("Failed to create set_signed_max_limits tx")?;

        let mut progress = signed_tx
            .submit_and_watch()
            .await
            .context("Failed to submit set_signed_max_limits tx")?;

        // Wait for finalization
        while let Some(status) = progress.next().await {
            match status? {
                subxt::tx::TxStatus::InBestBlock(block) => {
                    info!("set_signed_max_limits included in block: {:?} (waiting for finalization...)", block.block_hash());
                }
                subxt::tx::TxStatus::InFinalizedBlock(block) => {
                    info!(
                        "set_signed_max_limits FINALIZED in block: {:?}",
                        block.block_hash()
                    );
                    break;
                }
                subxt::tx::TxStatus::Error { message } => {
                    return Err(anyhow::anyhow!("set_signed_max_limits failed: {}", message));
                }
                subxt::tx::TxStatus::Dropped { message } => {
                    return Err(anyhow::anyhow!(
                        "set_signed_max_limits dropped: {}",
                        message
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Wait for pending transaction to finalize by monitoring nonce changes
    async fn wait_for_pending_tx(&self) {
        info!("Monitoring account nonce for pending tx finalization...");

        // Get current nonce
        let account_id = <Keypair as Signer<PolkadotConfig>>::account_id(&self.signer);
        let initial_nonce = match self.get_account_nonce(&account_id).await {
            Ok(n) => n,
            Err(_) => {
                warn!(
                    "Could not get nonce, falling back to {}s wait",
                    NONCE_RETRY_WAIT_SECS
                );
                tokio::time::sleep(Duration::from_secs(NONCE_RETRY_WAIT_SECS)).await;
                return;
            }
        };

        info!("Current nonce: {}, waiting for change...", initial_nonce);

        // Poll every block time until nonce changes or timeout
        for i in 0..PENDING_TX_TIMEOUT_ITERATIONS {
            tokio::time::sleep(Duration::from_secs(BLOCK_TIME_SECS)).await;

            match self.get_account_nonce(&account_id).await {
                Ok(new_nonce) if new_nonce != initial_nonce => {
                    info!(
                        "Nonce changed: {} -> {}, pending tx finalized!",
                        initial_nonce, new_nonce
                    );
                    return;
                }
                Ok(_) => {
                    if i % 5 == 4 {
                        info!(
                            "Still waiting for pending tx... ({}s)",
                            (i + 1) as u64 * BLOCK_TIME_SECS
                        );
                    }
                }
                Err(e) => {
                    warn!("Nonce query failed: {:?}", e);
                }
            }
        }

        warn!("Timeout waiting for pending tx, proceeding anyway...");
    }

    /// Get account nonce using system_accountNextIndex RPC
    /// This includes pending transactions, unlike storage queries
    async fn get_account_nonce(&self, account_id: &subxt::utils::AccountId32) -> Result<u32> {
        // Use RPC call which includes pending transactions
        let params = rpc_params![account_id.to_string()];
        let nonce: u32 = self
            .raw_rpc
            .request("system_accountNextIndex", params)
            .await
            .context("Failed to get account nonce via RPC")?;
        Ok(nonce)
    }

    /// Query SignedMigrationMaxLimits from chain
    async fn get_max_limits(&self) -> Result<Option<(u32, u32)>> {
        let limits_query =
            subxt::dynamic::storage("StateTrieMigration", "SignedMigrationMaxLimits", vec![]);

        let limits_thunk = self
            .client
            .storage()
            .at_latest()
            .await?
            .fetch(&limits_query)
            .await?;

        match limits_thunk {
            Some(thunk) => {
                let decoded = thunk.to_value()?;
                let size = decoded.at("size").and_then(|v| v.as_u128()).unwrap_or(0) as u32;
                let item = decoded.at("item").and_then(|v| v.as_u128()).unwrap_or(0) as u32;
                Ok(Some((size, item)))
            }
            None => Ok(None),
        }
    }

    /// Check account balance
    async fn check_balance(&self) -> Result<u128> {
        let account_id = <Keypair as Signer<PolkadotConfig>>::account_id(&self.signer);

        let balance_query = subxt::dynamic::storage(
            "System",
            "Account",
            vec![Value::from_bytes(AsRef::<[u8]>::as_ref(&account_id))],
        );

        let account_info = self
            .client
            .storage()
            .at_latest()
            .await?
            .fetch(&balance_query)
            .await?;

        match account_info {
            Some(info) => {
                let value = info.to_value()?;
                if let Some(data) = value.at("data") {
                    if let Some(free) = data.at("free") {
                        if let Some(balance) = free.as_u128() {
                            return Ok(balance);
                        }
                    }
                }
                Ok(0)
            }
            None => Ok(0),
        }
    }

    /// Get pending extrinsics from the transaction pool (requires unsafe RPC)
    async fn get_pending_extrinsics(&self) -> Result<Vec<String>> {
        use subxt::backend::rpc::RpcParams;

        let result: Vec<String> = self
            .raw_rpc
            .request("author_pendingExtrinsics", RpcParams::new())
            .await
            .context("Failed to get pending extrinsics (requires --rpc-methods=unsafe)")?;

        Ok(result)
    }

    /// Remove a specific extrinsic from the pool by its hash (requires unsafe RPC)
    async fn remove_extrinsic(&self, ext_hash: &str) -> Result<Vec<String>> {
        use subxt::backend::rpc::RpcParams;

        let mut params = RpcParams::new();
        params.push(vec![ext_hash])?;

        let result: Vec<String> = self
            .raw_rpc
            .request("author_removeExtrinsic", params)
            .await
            .context("Failed to remove extrinsic")?;

        Ok(result)
    }

    /// Show status information and pending transactions
    async fn show_status(&self) -> Result<()> {
        info!("=== Migration Status ===");

        // Get migration task
        if let Some((_, status)) = self.get_migration_task().await? {
            info!(
                "Top trie:   {} ({} items)",
                if status.top_complete {
                    "COMPLETE"
                } else {
                    "In Progress"
                },
                status.top_items
            );
            info!(
                "Child trie: {} ({} items)",
                if status.child_complete {
                    "COMPLETE"
                } else {
                    "In Progress"
                },
                status.child_items
            );
            info!("Total size migrated: {} bytes", status.size);
        } else {
            warn!("No migration progress found");
        }

        // Get balance
        let balance = self.check_balance().await?;
        let balance_wnd = balance as f64 / 1_000_000_000_000.0;
        info!("Account balance: {:.4} WND", balance_wnd);

        // Get nonce
        let account_id = <Keypair as Signer<PolkadotConfig>>::account_id(&self.signer);
        let nonce = self.get_account_nonce(&account_id).await?;
        info!("Account nonce: {}", nonce);

        // Get pending extrinsics
        info!("\n=== Transaction Pool ===");
        match self.get_pending_extrinsics().await {
            Ok(pending) => {
                if pending.is_empty() {
                    info!("No pending transactions in pool");
                } else {
                    info!("Pending transactions: {}", pending.len());
                    for (i, ext) in pending.iter().enumerate() {
                        // Show first 20 chars of each extrinsic
                        let preview = if ext.len() > 40 {
                            format!("{}...{}", &ext[..20], &ext[ext.len() - 16..])
                        } else {
                            ext.clone()
                        };
                        debug!("  [{}] {}", i, preview);
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Could not get pending extrinsics: {} (requires --rpc-methods=unsafe)",
                    e
                );
            }
        }

        Ok(())
    }

    /// Clear all pending extrinsics from our account
    async fn clear_pending_transactions(&self) -> Result<usize> {
        info!("Checking for pending transactions to clear...");

        let pending = match self.get_pending_extrinsics().await {
            Ok(p) => p,
            Err(e) => {
                warn!("Could not get pending extrinsics: {}", e);
                return Ok(0);
            }
        };

        if pending.is_empty() {
            info!("No pending transactions to clear");
            return Ok(0);
        }

        info!(
            "Found {} pending transaction(s), attempting to clear...",
            pending.len()
        );

        let mut cleared = 0;
        for ext in &pending {
            // Calculate the blake2-256 hash of the extrinsic
            use blake2::{Blake2s256, Digest};
            let ext_bytes = hex::decode(ext.trim_start_matches("0x")).unwrap_or_default();
            let hash = Blake2s256::digest(&ext_bytes);
            let hash_hex = format!("0x{}", hex::encode(hash));

            match self.remove_extrinsic(&hash_hex).await {
                Ok(removed) => {
                    if !removed.is_empty() {
                        info!("Removed extrinsic: {}", hash_hex);
                        cleared += 1;
                    }
                }
                Err(e) => {
                    debug!("Could not remove extrinsic {}: {}", hash_hex, e);
                }
            }
        }

        if cleared > 0 {
            info!("Cleared {} pending transaction(s)", cleared);
        } else {
            info!("No transactions could be cleared (may not be from our account)");
        }

        Ok(cleared)
    }

    /// Submit a continue_migrate transaction
    /// Mirrors TypeScript: api.tx.stateTrieMigration.continueMigrate(limits, sizeUpperLimit, currentTask)
    async fn submit_migration(&self, witness_task: Value<()>) -> Result<()> {
        info!(
            "Tx: items={}, size={}",
            self.config.item_limit, self.config.size_limit
        );

        // Capture nonce before submission for timeout verification
        let account_id = <Keypair as Signer<PolkadotConfig>>::account_id(&self.signer);
        let expected_nonce = self.get_account_nonce(&account_id).await.unwrap_or(0);

        // MigrationLimits { size: u32, item: u32 }
        let limits = Value::named_composite([
            ("size", Value::u128(self.config.size_limit as u128)),
            ("item", Value::u128(self.config.item_limit as u128)),
        ]);

        // real_size_upper: u32 - TypeScript uses sizeLimit * 2
        let real_size_upper = Value::u128((self.config.size_limit * 2) as u128);

        // Build the continue_migrate call
        // Parameters: limits, real_size_upper, witness_task
        let tx = subxt::dynamic::tx(
            "StateTrieMigration",
            "continue_migrate",
            vec![limits, real_size_upper, witness_task],
        );

        // Create signed transaction for dry run validation
        // Retry loop handles stale nonce (when previous tx finalized between nonce fetch and dry run)
        const MAX_DRY_RUN_RETRIES: u32 = 3;
        let mut dry_run_tx = None;

        for retry in 0..MAX_DRY_RUN_RETRIES {
            // Re-sign transaction to get fresh nonce
            let signed_tx = self
                .client
                .tx()
                .create_signed(&tx, &self.signer, Default::default())
                .await
                .context("Failed to create signed tx for dry run")?;

            // DRY RUN using system_dryRun RPC - actually executes the call
            // This catches dispatch errors (like SizeUpperBoundExceeded) that would cause slashing
            // NOTE: system_dryRun requires --rpc-methods=unsafe on the node
            // We remember if it's not supported to skip on future calls
            if self.dry_run_supported.load(Ordering::Relaxed) {
                info!("Dry run...");

                let tx_bytes = signed_tx.encoded();
                let dry_run_result = self.rpc.dry_run(tx_bytes, None).await;

                use subxt::backend::legacy::rpc_methods::DryRunResult;
                match dry_run_result {
                    Ok(dry_run_bytes) => {
                        // Store raw bytes for detailed error analysis
                        let raw_bytes = dry_run_bytes.0.clone();

                        match dry_run_bytes.into_dry_run_result(&self.client.metadata()) {
                            Ok(DryRunResult::Success) => {
                                info!("Dry run OK");
                                dry_run_tx = Some(signed_tx);
                                break; // Success - exit retry loop
                            }
                            Ok(DryRunResult::DispatchError(dispatch_err)) => {
                                let err_str = format!("{:?}", dispatch_err);
                                error!("Dry run FAILED - dispatch error: {}", err_str);

                                if err_str.contains("SizeUpperBoundExceeded") {
                                    return Err(MigrationError::SizeExceeded.into());
                                }
                                return Err(MigrationError::DryRunDispatchError(err_str).into());
                            }
                            Ok(DryRunResult::TransactionValidityError) => {
                                // Decode the raw bytes to get detailed validity error
                                let validity_error = decode_validity_error(&raw_bytes);

                                // If stale nonce, retry immediately with fresh signature
                                if matches!(validity_error, ValidityError::Stale) && retry < MAX_DRY_RUN_RETRIES - 1 {
                                    warn!("Dry run got stale nonce, re-signing tx (attempt {}/{})", retry + 1, MAX_DRY_RUN_RETRIES);
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                    continue; // Retry with fresh nonce
                                }

                                error!(
                                    "Dry run FAILED - transaction validity error: {}",
                                    validity_error
                                );
                                return Err(MigrationError::from_validity_error(validity_error).into());
                            }
                            Err(e) => {
                                warn!("Could not decode dry run result: {:?}", e);
                                dry_run_tx = Some(signed_tx);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        // Public RPCs don't allow system_dryRun - remember and skip future calls
                        let err_str = format!("{:?}", e);
                        if err_str.contains("unsafe") {
                            warn!(
                                "system_dryRun not available (requires --rpc-methods=unsafe on node)"
                            );
                            warn!("Disabling dry run for this session - USE AT YOUR OWN RISK!");
                            self.dry_run_supported.store(false, Ordering::Relaxed);
                            dry_run_tx = Some(signed_tx);
                            break;
                        } else {
                            error!("Dry run RPC error: {}", err_str);
                            return Err(MigrationError::RpcError(err_str).into());
                        }
                    }
                }
            } else {
                // Dry run not supported, just use the signed tx
                dry_run_tx = Some(signed_tx);
                break;
            }
        }

        let dry_run_tx = dry_run_tx.ok_or_else(|| {
            MigrationError::DryRunDispatchError("Failed to create valid transaction after retries".to_string())
        })?;

        if self.config.dry_run {
            info!("[DRY RUN] Would submit continue_migrate transaction");
            return Ok(());
        }

        // Create FRESH signed transaction for submission
        // This avoids AncientBirthBlock errors when dry run takes time
        let fresh_signed_tx = self
            .client
            .tx()
            .create_signed(&tx, &self.signer, Default::default())
            .await
            .context("Failed to create fresh signed tx for submission")?;

        // Submit the freshly-signed transaction and watch
        let mut progress = match fresh_signed_tx.submit_and_watch().await {
            Ok(p) => p,
            Err(e) => {
                let err_str = format!("{:?}", e);
                let migration_err = MigrationError::from_rpc_error(&err_str);

                // Log appropriate warning based on error type
                match &migration_err {
                    MigrationError::PoolConflict => {
                        warn!(
                            "TX POOL CONFLICT: Another transaction pending, waiting for it to clear..."
                        );
                    }
                    MigrationError::NonceStale => {
                        warn!(
                            "TX REJECTED (bad signature/nonce): Pool may have stale tx, waiting..."
                        );
                    }
                    MigrationError::TxBanned => {
                        warn!("TX BANNED: Transaction temporarily banned, waiting...");
                    }
                    _ => {}
                }

                return Err(migration_err.into());
            }
        };

        // Wait for FINALIZATION (not just inclusion) - this is critical!
        // TypeScript bot uses sendAndFinalize() which waits for finalization
        // State only propagates reliably after finalization
        //
        // IMPORTANT: Add timeout because WebSocket subscriptions can lose events
        let finalization_timeout = Duration::from_secs(120); // 2 minutes max wait
        let start_time = Instant::now();
        let mut included = false;

        while let Some(status) = progress.next().await {
            // Check timeout
            if start_time.elapsed() > finalization_timeout {
                warn!("Finalization timeout after {:?}, checking nonce...", start_time.elapsed());

                // Verify TX was applied by checking if nonce changed
                let current_nonce = self.get_account_nonce(&account_id).await?;
                if current_nonce > expected_nonce {
                    info!("Nonce advanced ({} -> {}), TX was finalized (missed event)", expected_nonce, current_nonce);
                    return Ok(());
                } else {
                    return Err(MigrationError::SubmissionFailed(
                        "Finalization timeout - TX may be stuck".to_string()
                    ).into());
                }
            }

            match status? {
                subxt::tx::TxStatus::Broadcasted { num_peers } => {
                    info!("Broadcast to {} peers", num_peers);
                }
                subxt::tx::TxStatus::InBestBlock(block) => {
                    info!("Included {:?}...", block.block_hash());
                    included = true;
                    // Don't break here - continue waiting for finalization
                }
                subxt::tx::TxStatus::InFinalizedBlock(block) => {
                    info!("Finalized {:?}", block.block_hash());

                    let events = block.fetch_events().await?;
                    for evt in events.iter().flatten() {
                        if evt.pallet_name() == "StateTrieMigration" {
                            info!("  → {}.{}", evt.pallet_name(), evt.variant_name());
                        }
                    }
                    break; // Only break after finalization
                }
                subxt::tx::TxStatus::Error { message } => {
                    error!("Transaction error: {}", message);
                    return Err(MigrationError::SubmissionFailed(message).into());
                }
                subxt::tx::TxStatus::Dropped { message } => {
                    warn!("Transaction dropped: {}", message);
                    return Err(MigrationError::TxDropped(message).into());
                }
                _ => {}
            }
        }

        // If we exit the loop without breaking (stream ended), check if TX succeeded
        if included {
            warn!("Progress stream ended without finalization event, checking nonce...");
            let current_nonce = self.get_account_nonce(&account_id).await?;
            if current_nonce > expected_nonce {
                info!("Nonce advanced ({} -> {}), TX was finalized (stream ended early)", expected_nonce, current_nonce);
                return Ok(());
            }
        }

        Ok(())
    }

    /// Run the migration bot
    async fn run(&mut self) -> Result<()> {
        // Handle --status flag
        if self.config.status {
            return self.show_status().await;
        }

        // Handle --clear-pending flag
        if self.config.clear_pending {
            self.clear_pending_transactions().await?;
            if self.config.once || self.config.runs == 0 {
                return Ok(()); // Exit after clearing if --once or no runs specified
            }
        }

        info!("Starting migration bot...");
        send_notification(
            "Westend Bot Started",
            "Bot is running and monitoring migration.",
            false,
        );

        // Spawn heartbeat task (shows dad jokes every 60s) with graceful shutdown
        let shutdown_token = self.shutdown.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
            interval.tick().await; // Skip first immediate tick
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        debug!("Heartbeat task shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        if let Some(joke) = fetch_dad_joke().await {
                            info!("💓 {}", joke);
                        }
                    }
                }
            }
        });

        // Check chain limits and determine what to use
        let current_limits = self.get_max_limits().await?;

        // If config is 0, use chain max; otherwise use config value
        match current_limits {
            Some((max_size, max_item)) => {
                // Use half of chain max if config is 0.
                // Rationale: Using 50% of max limits provides safety margin for:
                // 1. State changes between query and submission (witness_task mismatch)
                // 2. Avoiding SizeUpperBoundExceeded errors that could cause slashing
                // 3. Leaving room for other transactions in the block
                // The TypeScript reference also uses conservative limits.
                if self.config.item_limit == 0 {
                    self.config.item_limit = max_item / 2;
                }
                if self.config.size_limit == 0 {
                    self.config.size_limit = max_size / 2;
                }

                // Check if we need to update chain limits (config exceeds chain max)
                if self.config.item_limit > max_item || self.config.size_limit > max_size {
                    info!(
                        "Updating chain limits: items={}, size={}",
                        self.config.item_limit, self.config.size_limit
                    );
                    self.set_max_limits(self.config.size_limit, self.config.item_limit)
                        .await?;
                } else {
                    info!(
                        "Using limits: items={}, size={}",
                        self.config.item_limit, self.config.size_limit
                    );
                }
            }
            None => {
                // No chain limits set - use sensible defaults if config is 0
                if self.config.item_limit == 0 {
                    self.config.item_limit = 4096;
                }
                if self.config.size_limit == 0 {
                    self.config.size_limit = 409600;
                }
                info!(
                    "Setting chain limits: items={}, size={}",
                    self.config.item_limit, self.config.size_limit
                );
                self.set_max_limits(self.config.size_limit, self.config.item_limit)
                    .await?;
            }
        }

        // Check balance
        let balance = self.check_balance().await?;
        info!("Account balance: {} units", balance);

        if balance == 0 {
            warn!("Account has zero balance! Transactions will fail.");
            if !self.config.dry_run {
                return Err(MigrationError::ZeroBalance.into());
            }
        }

        // Track successful migrations for --runs limit
        let mut successful_runs: u32 = 0;
        let mut consecutive_errors: u32 = 0;
        let target_runs = self.config.runs;

        if target_runs > 0 {
            info!("Will submit {} migration transaction(s)", target_runs);
        }

        loop {
            // Get current migration task
            let (witness_task, status) = match self.get_migration_task().await? {
                Some(result) => result,
                None => {
                    warn!("Could not fetch migration progress");
                    if self.config.once {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(self.config.delay_secs)).await;
                    continue;
                }
            };

            info!(
                "Status: top={}/{} child={}/{} size={}",
                if status.top_complete { "done" } else { "wip" },
                status.top_items,
                if status.child_complete { "done" } else { "wip" },
                status.child_items,
                status.size
            );

            if status.is_complete() {
                info!("Migration is COMPLETE!");
                send_notification(
                    "Migration Complete",
                    "The Westend state trie migration is complete!",
                    false,
                );
                break;
            }

            // Check balance BEFORE tx (migration should be FREE for controller)
            let balance_before = self.check_balance().await?;

            // Submit migration transaction
            match self.submit_migration(witness_task).await {
                Ok(()) => {
                    successful_runs += 1;
                    info!("Tx #{} ✓", successful_runs);

                    let runs_left = if target_runs > 0 {
                        (target_runs - successful_runs).to_string()
                    } else {
                        "Unlimited".to_string()
                    };
                    let msg = format!(
                        "Migrated {} items ({} bytes)\nRun: {} | Remaining: {}",
                        self.config.item_limit, self.config.size_limit, successful_runs, runs_left
                    );
                    send_notification("Transaction Confirmed", &msg, false);

                    // Check balance AFTER tx - should be unchanged (free tx)
                    let balance_after = self.check_balance().await?;
                    if let Some(lost_wnd) = check_balance_decrease(balance_before, balance_after) {
                        error!(
                            "⚠️  BALANCE DECREASED by {:.6} WND! Possible slashing!",
                            lost_wnd
                        );
                        error!("Before: {}, After: {}", balance_before, balance_after);
                        send_notification(
                            "CRITICAL WARNING",
                            &format!("Balance decreased by {:.6} WND! Bot stopped.", lost_wnd),
                            true,
                        );
                        // Stop immediately if we're losing funds
                        return Err(MigrationError::BalanceDecreased { lost_wnd }.into());
                    } else {
                        info!("Balance OK (free tx)");
                    }

                    // Check if we've reached target runs
                    if target_runs > 0 && successful_runs >= target_runs {
                        info!("Done: {} migrations", successful_runs);
                        break;
                    }
                }
                Err(e) => {
                    // Try to downcast to MigrationError for structured handling
                    let migration_err = e.downcast_ref::<MigrationError>();

                    if let Some(err) = migration_err {
                        if err.requires_pool_wait() {
                            // Pool has pending tx - wait for it to finalize (not counted as error)
                            warn!("Pool conflict detected, waiting for pending tx to finalize...");
                            consecutive_errors = 0; // Reset on recoverable error
                            self.wait_for_pending_tx().await;
                        } else if matches!(err, MigrationError::TxBanned) {
                            // Temporarily banned - wait longer (not counted as error)
                            warn!("TX temporarily banned, waiting {}s...", BANNED_TX_WAIT_SECS);
                            consecutive_errors = 0; // Reset on recoverable error
                            tokio::time::sleep(Duration::from_secs(BANNED_TX_WAIT_SECS)).await;
                        } else if err.is_recoverable() {
                            // Other recoverable errors - retry with backoff
                            warn!("Recoverable error: {}, retrying...", err);
                            tokio::time::sleep(Duration::from_secs(RETRY_WAIT_SECS)).await;
                        } else {
                            // Non-recoverable error
                            consecutive_errors += 1;
                            error!(
                                "Migration transaction failed ({}/{}): {}",
                                consecutive_errors, MAX_CONSECUTIVE_ERRORS, err
                            );

                            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                                error!("Too many consecutive errors, stopping bot");
                                return Err(MigrationError::TooManyErrors {
                                    count: consecutive_errors,
                                    last_error: err.to_string(),
                                }
                                .into());
                            }

                            warn!("Waiting {} seconds before retry...", RETRY_WAIT_SECS);
                            tokio::time::sleep(Duration::from_secs(RETRY_WAIT_SECS)).await;
                        }
                    } else {
                        // Unknown error type - treat as non-recoverable
                        consecutive_errors += 1;
                        error!(
                            "Migration transaction failed ({}/{}): {:?}",
                            consecutive_errors, MAX_CONSECUTIVE_ERRORS, e
                        );

                        if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                            error!("Too many consecutive errors, stopping bot");
                            return Err(MigrationError::TooManyErrors {
                                count: consecutive_errors,
                                last_error: e.to_string(),
                            }
                            .into());
                        }

                        warn!("Waiting {} seconds before retry...", RETRY_WAIT_SECS);
                        tokio::time::sleep(Duration::from_secs(RETRY_WAIT_SECS)).await;
                    }
                }
            }

            if self.config.once {
                info!("--once flag set, exiting after single run");
                break;
            }

            // Wait before next iteration (if configured)
            if self.config.delay_secs > 0 {
                info!(
                    "Waiting {} seconds before next migration...",
                    self.config.delay_secs
                );
                tokio::time::sleep(Duration::from_secs(self.config.delay_secs)).await;
            }
        }

        // Signal shutdown to background tasks
        self.shutdown.cancel();
        Ok(())
    }
}

const LOCKFILE_PATH: &str = "/tmp/westend-migrate.lock";

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Acquire exclusive lock to prevent multiple instances
    let lockfile = File::create(LOCKFILE_PATH)
        .context("Failed to create lockfile")?;

    if lockfile.try_lock_exclusive().is_err() {
        eprintln!("ERROR: Another instance is already running (lockfile: {})", LOCKFILE_PATH);
        eprintln!("If this is incorrect, delete the lockfile and try again.");
        std::process::exit(1);
    }
    // Lock is held for the lifetime of the process and released on exit

    // Disable desktop notifications if running headless
    if cli.no_notify {
        disable_notifications();
    }

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| log_level.into()),
        )
        .with_timer(LocalTimer)
        .with_target(false)
        .compact()
        .init();

    info!(
        "Westend State-Trie Migration Bot v{}",
        env!("CARGO_PKG_VERSION")
    );

    let mut bot = MigrationBot::new(cli).await?;
    bot.run().await?;

    Ok(())
}
