//! Consolidated error types for the migration bot
//!
//! This module provides structured error handling with specific variants
//! for different failure modes, enabling better error recovery and logging.

use crate::utils::ValidityError;
use thiserror::Error;

/// Main error type for the migration bot
#[derive(Error, Debug)]
pub enum MigrationError {
    // === Transaction Pool Errors ===
    /// Another transaction from this account is already in the pool
    #[error("Pool conflict: another transaction is pending (code 1014)")]
    PoolConflict,

    /// Transaction nonce is stale (already used or pending)
    #[error("Stale nonce: transaction already applied or pending")]
    NonceStale,

    /// Transaction nonce is too high (future)
    #[error("Future nonce: previous transaction not yet applied")]
    NonceFuture,

    /// Transaction temporarily banned from pool
    #[error("Transaction temporarily banned from pool")]
    TxBanned,

    // === Validation Errors ===
    /// Dry run detected a dispatch error
    #[error("Dry run dispatch error: {0}")]
    DryRunDispatchError(String),

    /// Size limit exceeded - would cause slashing
    #[error("SizeUpperBoundExceeded - reduce item_limit to avoid slashing")]
    SizeExceeded,

    /// Transaction validity error from dry run
    #[error("Transaction validity error: {0}")]
    ValidityError(ValidityError),

    // === Balance/Safety Errors ===
    /// Balance decreased after transaction - possible slashing
    #[error("Balance decreased by {lost_wnd:.6} WND - possible slashing detected!")]
    BalanceDecreased { lost_wnd: f64 },

    /// Account has zero balance
    #[error("Account has zero balance - transactions will fail")]
    ZeroBalance,

    // === Network Errors ===
    /// Failed to connect to RPC endpoint
    #[error("Failed to connect to RPC: {0}")]
    ConnectionFailed(String),

    /// RPC request failed
    #[error("RPC request failed: {0}")]
    RpcError(String),

    /// Transaction submission failed
    #[error("Transaction submission failed: {0}")]
    SubmissionFailed(String),

    /// Transaction was dropped from the pool
    #[error("Transaction dropped: {0}")]
    TxDropped(String),

    // === State Errors ===
    /// Migration is already complete (reserved for future use)
    #[allow(dead_code)]
    #[error("Migration is already complete")]
    MigrationComplete,

    /// Could not fetch migration progress (reserved for future use)
    #[allow(dead_code)]
    #[error("Could not fetch migration progress from chain")]
    NoMigrationProgress,

    // === Configuration Errors ===
    /// Invalid seed/mnemonic
    #[error("Invalid seed: {0}")]
    InvalidSeed(String),

    /// Too many consecutive errors
    #[error("Stopped after {count} consecutive errors. Last: {last_error}")]
    TooManyErrors { count: u32, last_error: String },

    // === Generic Errors ===
    /// Wrapped anyhow error for compatibility
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl MigrationError {
    /// Check if this error is recoverable (should retry)
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            MigrationError::PoolConflict
                | MigrationError::NonceStale
                | MigrationError::NonceFuture
                | MigrationError::TxBanned
                | MigrationError::RpcError(_)
        )
    }

    /// Check if this error indicates a pool conflict that requires waiting
    pub fn requires_pool_wait(&self) -> bool {
        matches!(
            self,
            MigrationError::PoolConflict | MigrationError::NonceStale
        )
    }

    /// Parse RPC error string into structured error
    pub fn from_rpc_error(err_str: &str) -> Self {
        if err_str.contains("1014") || err_str.contains("Priority is too low") {
            MigrationError::PoolConflict
        } else if err_str.contains("1010") || err_str.contains("bad signature") {
            MigrationError::NonceStale
        } else if err_str.contains("1012") || err_str.contains("temporarily banned") {
            MigrationError::TxBanned
        } else {
            MigrationError::SubmissionFailed(err_str.to_string())
        }
    }

    /// Convert from ValidityError
    pub fn from_validity_error(ve: ValidityError) -> Self {
        match ve {
            ValidityError::Stale => MigrationError::NonceStale,
            ValidityError::Future => MigrationError::NonceFuture,
            ValidityError::Priority => MigrationError::PoolConflict,
            _ => MigrationError::ValidityError(ve),
        }
    }
}

/// Result type alias for migration operations (reserved for future use)
#[allow(dead_code)]
pub type MigrationResult<T> = Result<T, MigrationError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_conflict_is_recoverable() {
        assert!(MigrationError::PoolConflict.is_recoverable());
        assert!(MigrationError::NonceStale.is_recoverable());
        assert!(MigrationError::TxBanned.is_recoverable());
    }

    #[test]
    fn test_balance_decrease_not_recoverable() {
        let err = MigrationError::BalanceDecreased { lost_wnd: 0.5 };
        assert!(!err.is_recoverable());
    }

    #[test]
    fn test_from_rpc_error_parsing() {
        assert!(matches!(
            MigrationError::from_rpc_error("Error 1014: Priority is too low"),
            MigrationError::PoolConflict
        ));

        assert!(matches!(
            MigrationError::from_rpc_error("Error 1010: bad signature"),
            MigrationError::NonceStale
        ));

        assert!(matches!(
            MigrationError::from_rpc_error("Error 1012: Transaction is temporarily banned"),
            MigrationError::TxBanned
        ));

        assert!(matches!(
            MigrationError::from_rpc_error("Some other error"),
            MigrationError::SubmissionFailed(_)
        ));
    }

    #[test]
    fn test_requires_pool_wait() {
        assert!(MigrationError::PoolConflict.requires_pool_wait());
        assert!(MigrationError::NonceStale.requires_pool_wait());
        assert!(!MigrationError::TxBanned.requires_pool_wait());
        assert!(!MigrationError::SizeExceeded.requires_pool_wait());
    }

    #[test]
    fn test_error_display() {
        let err = MigrationError::BalanceDecreased { lost_wnd: 1.234567 };
        assert!(err.to_string().contains("1.234567"));

        let err = MigrationError::TooManyErrors {
            count: 5,
            last_error: "test error".to_string(),
        };
        assert!(err.to_string().contains("5"));
        assert!(err.to_string().contains("test error"));
    }
}
