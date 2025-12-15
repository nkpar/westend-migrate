use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use subxt::dynamic::{At, Value};
use tracing::debug;

/// Global flag to disable desktop notifications
static NOTIFICATIONS_DISABLED: AtomicBool = AtomicBool::new(false);

/// Disable desktop notifications globally
pub fn disable_notifications() {
    NOTIFICATIONS_DISABLED.store(true, Ordering::Relaxed);
}

/// Structured validity error types for better matching
#[derive(Debug, Clone, PartialEq)]
pub enum ValidityError {
    /// Nonce too low - transaction already applied or pending
    Stale,
    /// Nonce too high - previous transaction not yet applied
    Future,
    /// Transaction priority too low - pool conflict (used in from_validity_error matching)
    #[allow(dead_code)]
    Priority,
    /// Unable to pay fees (insufficient balance)
    Payment,
    /// Bad signature or proof
    BadProof,
    /// Transaction from too old block (mortality expired)
    AncientBirthBlock,
    /// Would exhaust block resources
    ExhaustsResources,
    /// Other error with description
    Other(String),
}

impl fmt::Display for ValidityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidityError::Stale => write!(
                f,
                "Stale - nonce too low (transaction already applied or pending)"
            ),
            ValidityError::Future => {
                write!(f, "Future - nonce too high (previous tx not yet applied)")
            }
            ValidityError::Priority => write!(f, "Priority too low - pool conflict"),
            ValidityError::Payment => {
                write!(f, "Payment - unable to pay fees (insufficient balance)")
            }
            ValidityError::BadProof => write!(f, "BadProof - invalid signature"),
            ValidityError::AncientBirthBlock => {
                write!(f, "AncientBirthBlock - transaction mortality expired")
            }
            ValidityError::ExhaustsResources => {
                write!(f, "ExhaustsResources - would exhaust block resources")
            }
            ValidityError::Other(s) => write!(f, "{}", s),
        }
    }
}

/// Parsed migration status for display
#[derive(Debug)]
pub struct MigrationStatus {
    pub top_complete: bool,
    pub child_complete: bool,
    pub size: u64,
    pub top_items: u64,
    pub child_items: u64,
}

impl MigrationStatus {
    pub fn is_complete(&self) -> bool {
        self.top_complete && self.child_complete
    }
}

/// Parse migration status from a subxt Value
/// Checks if progress variants are named "Complete"
pub fn parse_migration_status<T: std::fmt::Debug>(decoded: &Value<T>) -> MigrationStatus {
    let top_complete = decoded
        .at("progress_top")
        .map(|v| format!("{:?}", v).contains("Complete"))
        .unwrap_or(false);

    let child_complete = decoded
        .at("progress_child")
        .map(|v| format!("{:?}", v).contains("Complete"))
        .unwrap_or(false);

    let size = decoded.at("size").and_then(|v| v.as_u128()).unwrap_or(0) as u64;
    let top_items = decoded
        .at("top_items")
        .and_then(|v| v.as_u128())
        .unwrap_or(0) as u64;
    let child_items = decoded
        .at("child_items")
        .and_then(|v| v.as_u128())
        .unwrap_or(0) as u64;

    MigrationStatus {
        top_complete,
        child_complete,
        size,
        top_items,
        child_items,
    }
}

/// Decode TransactionValidityError from raw dry_run result bytes
/// The dry_run result is: Result<Result<(), DispatchError>, TransactionValidityError>
/// When we get TransactionValidityError, the bytes start with 0x01 (Err variant)
///
/// Returns a structured ValidityError for easier matching in the caller.
pub fn decode_validity_error(raw_bytes: &[u8]) -> ValidityError {
    if raw_bytes.is_empty() {
        return ValidityError::Other("empty response".to_string());
    }

    // TransactionValidityError is an enum with variants:
    // 0 = Invalid(InvalidTransaction)
    // 1 = Unknown(UnknownTransaction)
    //
    // InvalidTransaction variants:
    // 0 = Call, 1 = Payment, 2 = Future, 3 = Stale, 4 = BadProof,
    // 5 = AncientBirthBlock, 6 = ExhaustsResources, 7 = Custom(u8),
    // 8 = BadMandatory, 9 = MandatoryValidation, 10 = BadSigner
    //
    // UnknownTransaction variants:
    // 0 = CannotLookup, 1 = NoUnsignedValidator, 2 = Custom(u8)

    // First byte after Result::Err marker (0x01) indicates error type
    let error_start = if raw_bytes[0] == 0x01 { 1 } else { 0 };
    if raw_bytes.len() <= error_start {
        return ValidityError::Other(format!("raw: 0x{}", hex::encode(raw_bytes)));
    }

    let validity_type = raw_bytes.get(error_start).unwrap_or(&255);
    let sub_type = raw_bytes.get(error_start + 1).unwrap_or(&255);

    match validity_type {
        0 => {
            // Invalid transaction - return structured types for common cases
            match sub_type {
                1 => ValidityError::Payment,
                2 => ValidityError::Future,
                3 => ValidityError::Stale,
                4 => ValidityError::BadProof,
                5 => ValidityError::AncientBirthBlock,
                6 => ValidityError::ExhaustsResources,
                0 => ValidityError::Other(
                    "Invalid::Call - the call of the transaction is not expected".to_string(),
                ),
                7 => {
                    let custom = raw_bytes.get(error_start + 2).unwrap_or(&0);
                    ValidityError::Other(format!("Invalid::Custom({})", custom))
                }
                8 => ValidityError::Other(
                    "Invalid::BadMandatory - mandatory dispatch failed".to_string(),
                ),
                9 => ValidityError::Other(
                    "Invalid::MandatoryValidation - mandatory validation failed".to_string(),
                ),
                10 => ValidityError::BadProof, // BadSigner maps to BadProof
                _ => ValidityError::Other(format!("Invalid::Unknown({})", sub_type)),
            }
        }
        1 => {
            // Unknown transaction
            match sub_type {
                0 => ValidityError::Other(
                    "Unknown::CannotLookup - could not look up information".to_string(),
                ),
                1 => ValidityError::Other(
                    "Unknown::NoUnsignedValidator - no validator for unsigned tx".to_string(),
                ),
                2 => {
                    let custom = raw_bytes.get(error_start + 2).unwrap_or(&0);
                    ValidityError::Other(format!("Unknown::Custom({})", custom))
                }
                _ => ValidityError::Other(format!("Unknown::Unknown({})", sub_type)),
            }
        }
        _ => ValidityError::Other(format!(
            "UnrecognizedError(type={}, raw=0x{})",
            validity_type,
            hex::encode(raw_bytes)
        )),
    }
}

/// Fetch a random dad joke from icanhazdadjoke.com
pub async fn fetch_dad_joke() -> Option<String> {
    #[derive(serde::Deserialize)]
    struct JokeResponse {
        joke: String,
    }

    let client = reqwest::Client::new();
    match client
        .get("https://icanhazdadjoke.com/")
        .header("Accept", "application/json")
        .header("User-Agent", "WestendMigrationBot/0.1")
        .send()
        .await
    {
        Ok(resp) => match resp.json::<JokeResponse>().await {
            Ok(j) => Some(j.joke),
            Err(e) => {
                debug!("Failed to parse dad joke: {:?}", e);
                None
            }
        },
        Err(e) => {
            debug!("Failed to fetch dad joke: {:?}", e);
            None
        }
    }
}

/// Send a desktop notification
pub fn send_notification(summary: &str, body: &str, is_error: bool) {
    // Skip if notifications are disabled (e.g., running on headless server)
    if NOTIFICATIONS_DISABLED.load(Ordering::Relaxed) {
        return;
    }

    use notify_rust::{Notification, Timeout, Urgency};

    let (timeout, urgency) = if is_error {
        (Timeout::Never, Urgency::Critical)
    } else {
        (Timeout::Milliseconds(5000), Urgency::Normal)
    };

    if let Err(e) = Notification::new()
        .summary(summary)
        .body(body)
        .appname("Westend Migration Bot")
        .timeout(timeout)
        .urgency(urgency)
        .show()
    {
        tracing::warn!("Failed to send notification: {:?}", e);
    }
}

/// Convert balance from units to WND (12 decimals)
pub fn units_to_wnd(units: u128) -> f64 {
    units as f64 / 1_000_000_000_000.0
}

/// Check if balance decreased (possible slashing)
pub fn check_balance_decrease(before: u128, after: u128) -> Option<f64> {
    if after < before {
        Some(units_to_wnd(before - after))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use subxt::ext::scale_value::Composite;

    // ==================== ValidityError Tests ====================

    #[test]
    fn test_validity_error_display() {
        assert_eq!(
            ValidityError::Stale.to_string(),
            "Stale - nonce too low (transaction already applied or pending)"
        );
        assert_eq!(
            ValidityError::Future.to_string(),
            "Future - nonce too high (previous tx not yet applied)"
        );
        assert_eq!(
            ValidityError::Priority.to_string(),
            "Priority too low - pool conflict"
        );
        assert_eq!(
            ValidityError::Payment.to_string(),
            "Payment - unable to pay fees (insufficient balance)"
        );
        assert_eq!(
            ValidityError::BadProof.to_string(),
            "BadProof - invalid signature"
        );
        assert_eq!(
            ValidityError::AncientBirthBlock.to_string(),
            "AncientBirthBlock - transaction mortality expired"
        );
        assert_eq!(
            ValidityError::ExhaustsResources.to_string(),
            "ExhaustsResources - would exhaust block resources"
        );
        assert_eq!(
            ValidityError::Other("custom error".to_string()).to_string(),
            "custom error"
        );
    }

    #[test]
    fn test_validity_error_equality() {
        assert_eq!(ValidityError::Stale, ValidityError::Stale);
        assert_ne!(ValidityError::Stale, ValidityError::Future);
        assert_eq!(
            ValidityError::Other("test".to_string()),
            ValidityError::Other("test".to_string())
        );
    }

    // ==================== decode_validity_error Tests ====================

    #[test]
    fn test_decode_empty_bytes() {
        let result = decode_validity_error(&[]);
        assert!(matches!(result, ValidityError::Other(s) if s.contains("empty")));
    }

    #[test]
    fn test_decode_invalid_payment() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x01 = Payment
        let bytes = vec![0x01, 0x00, 0x01];
        assert_eq!(decode_validity_error(&bytes), ValidityError::Payment);
    }

    #[test]
    fn test_decode_invalid_future() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x02 = Future
        let bytes = vec![0x01, 0x00, 0x02];
        assert_eq!(decode_validity_error(&bytes), ValidityError::Future);
    }

    #[test]
    fn test_decode_invalid_stale() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x03 = Stale
        let bytes = vec![0x01, 0x00, 0x03];
        assert_eq!(decode_validity_error(&bytes), ValidityError::Stale);
    }

    #[test]
    fn test_decode_invalid_bad_proof() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x04 = BadProof
        let bytes = vec![0x01, 0x00, 0x04];
        assert_eq!(decode_validity_error(&bytes), ValidityError::BadProof);
    }

    #[test]
    fn test_decode_invalid_ancient_birth_block() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x05 = AncientBirthBlock
        let bytes = vec![0x01, 0x00, 0x05];
        assert_eq!(
            decode_validity_error(&bytes),
            ValidityError::AncientBirthBlock
        );
    }

    #[test]
    fn test_decode_invalid_exhausts_resources() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x06 = ExhaustsResources
        let bytes = vec![0x01, 0x00, 0x06];
        assert_eq!(
            decode_validity_error(&bytes),
            ValidityError::ExhaustsResources
        );
    }

    #[test]
    fn test_decode_invalid_call() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x00 = Call
        let bytes = vec![0x01, 0x00, 0x00];
        let result = decode_validity_error(&bytes);
        assert!(matches!(result, ValidityError::Other(s) if s.contains("Call")));
    }

    #[test]
    fn test_decode_invalid_custom() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x07 = Custom, 0x42 = custom value
        let bytes = vec![0x01, 0x00, 0x07, 0x42];
        let result = decode_validity_error(&bytes);
        assert!(matches!(result, ValidityError::Other(s) if s.contains("Custom(66)")));
    }

    #[test]
    fn test_decode_invalid_bad_signer() {
        // 0x01 = Err variant, 0x00 = Invalid, 0x0a = BadSigner (maps to BadProof)
        let bytes = vec![0x01, 0x00, 0x0a];
        assert_eq!(decode_validity_error(&bytes), ValidityError::BadProof);
    }

    #[test]
    fn test_decode_unknown_cannot_lookup() {
        // 0x01 = Err variant, 0x01 = Unknown, 0x00 = CannotLookup
        let bytes = vec![0x01, 0x01, 0x00];
        let result = decode_validity_error(&bytes);
        assert!(matches!(result, ValidityError::Other(s) if s.contains("CannotLookup")));
    }

    #[test]
    fn test_decode_unknown_no_unsigned_validator() {
        // 0x01 = Err variant, 0x01 = Unknown, 0x01 = NoUnsignedValidator
        let bytes = vec![0x01, 0x01, 0x01];
        let result = decode_validity_error(&bytes);
        assert!(matches!(result, ValidityError::Other(s) if s.contains("NoUnsignedValidator")));
    }

    #[test]
    fn test_decode_without_err_marker() {
        // No 0x01 prefix - start parsing from byte 0
        let bytes = vec![0x00, 0x03]; // Invalid::Stale
        assert_eq!(decode_validity_error(&bytes), ValidityError::Stale);
    }

    #[test]
    fn test_decode_short_bytes() {
        // Only error marker, no content
        let bytes = vec![0x01];
        let result = decode_validity_error(&bytes);
        assert!(matches!(result, ValidityError::Other(_)));
    }

    // ==================== MigrationStatus Tests ====================

    #[test]
    fn test_migration_status_incomplete() {
        let status = MigrationStatus {
            top_complete: false,
            child_complete: false,
            size: 1000,
            top_items: 500,
            child_items: 0,
        };
        assert!(!status.is_complete());
    }

    #[test]
    fn test_migration_status_partial_complete() {
        let status = MigrationStatus {
            top_complete: true,
            child_complete: false,
            size: 5000,
            top_items: 1000,
            child_items: 500,
        };
        assert!(!status.is_complete());
    }

    #[test]
    fn test_migration_status_fully_complete() {
        let status = MigrationStatus {
            top_complete: true,
            child_complete: true,
            size: 10000,
            top_items: 2000,
            child_items: 1000,
        };
        assert!(status.is_complete());
    }

    // ==================== Balance Utilities Tests ====================

    #[test]
    fn test_units_to_wnd_conversion() {
        // 1 WND = 10^12 units
        assert_eq!(units_to_wnd(1_000_000_000_000), 1.0);
        assert_eq!(units_to_wnd(500_000_000_000), 0.5);
        assert_eq!(units_to_wnd(0), 0.0);

        // Test precision
        let result = units_to_wnd(1_234_567_890_123);
        assert!((result - 1.234567890123).abs() < 1e-10);
    }

    #[test]
    fn test_check_balance_decrease_no_change() {
        assert_eq!(check_balance_decrease(1000, 1000), None);
    }

    #[test]
    fn test_check_balance_decrease_increased() {
        assert_eq!(check_balance_decrease(1000, 2000), None);
    }

    #[test]
    fn test_check_balance_decrease_detected() {
        // 1 WND lost
        let result = check_balance_decrease(2_000_000_000_000, 1_000_000_000_000);
        assert!(result.is_some());
        assert!((result.unwrap() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_check_balance_decrease_small_amount() {
        // 0.001 WND lost
        let result = check_balance_decrease(1_001_000_000_000, 1_000_000_000_000);
        assert!(result.is_some());
        assert!((result.unwrap() - 0.001).abs() < 1e-10);
    }

    // ==================== Parse Migration Status Tests ====================

    #[test]
    fn test_parse_migration_status_parsing() {
        // Construct a Value mimicking the structure on chain
        // MigrationProcess {
        //   progress_top: Progress::Complete,
        //   progress_child: Progress::ToStart,
        //   size: 100,
        //   top_items: 10,
        //   child_items: 20
        // }
        let value = Value::named_composite([
            (
                "progress_top",
                Value::variant("Complete", Composite::named::<&str, _>([])),
            ),
            (
                "progress_child",
                Value::variant("ToStart", Composite::named::<&str, _>([])),
            ),
            ("size", Value::u128(100)),
            ("top_items", Value::u128(10)),
            ("child_items", Value::u128(20)),
        ]);

        let status = parse_migration_status(&value);

        assert_eq!(status.top_complete, true);
        assert_eq!(status.child_complete, false);
        assert_eq!(status.size, 100);
        assert_eq!(status.top_items, 10);
        assert_eq!(status.child_items, 20);
    }
}
