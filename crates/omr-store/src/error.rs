//! Store errors and their Appendix A exceptions (SYS-010, SYS-011, SYS-015, SYS-016).

use crate::events::ChainError;
use crate::jcs::JcsError;
use omr_model::{Exception, Scope, Severity};
use thiserror::Error;

/// Everything that can go wrong in the project store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// File-system error.
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialisation error.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A value has no canonical JSON form.
    #[error("canonical json: {0}")]
    Canonical(#[from] JcsError),
    /// The linked SQLite is older than the §14.2 floor (3.51.3: WAL-reset corruption bug).
    #[error("SQLite {found} is below the required {required} (WAL-reset bug, §14.2)")]
    SqliteTooOld {
        /// Version found.
        found: String,
        /// Minimum version.
        required: String,
    },
    /// The project schema is newer than this app (SYS-016): open read-only.
    #[error("project schema version {found} is newer than this app ({supported}); read-only")]
    SchemaTooNew {
        /// `user_version` found.
        found: i64,
        /// Newest version this app knows.
        supported: i64,
    },
    /// The file is not an OMR project database (`application_id` mismatch).
    #[error("not an OMR project database (application_id {0:#x})")]
    NotAProject(i64),
    /// `integrity_check`, `quick_check` or `foreign_key_check` failed (SYS-010).
    #[error("database integrity failure: {0}")]
    Integrity(String),
    /// The audit chain failed verification (SYS-011).
    #[error("audit chain verification failed: {0}")]
    Chain(#[from] ChainError),
    /// Another process holds the project's writer lock.
    #[error("project is locked by another writer: {0}")]
    Locked(String),
    /// The project location is unsafe (SYS-015).
    #[error("unsafe project location: {0}")]
    UnsafeLocation(String),
    /// A referenced row does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// A write conflicts with an immutable or append-only row.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Input rejected by a store rule (reason codes, PII in payloads, bad identifiers).
    #[error("invalid: {0}")]
    Invalid(String),
    /// A stored object does not match its recorded hashes.
    #[error("object {address} is corrupt: {detail}")]
    ObjectCorrupt {
        /// BLAKE3 dedup address.
        address: String,
        /// What did not match.
        detail: String,
    },
    /// Checkpoint signing failed.
    #[error("checkpoint signing failed: {0}")]
    Signing(String),
    /// A command panicked; its transaction was rolled back and nothing was saved (§14.2).
    #[error("command panicked and was rolled back: {0}")]
    Panicked(String),
}

/// Result alias.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

impl StoreError {
    /// The Appendix A exception this error raises, if any.
    pub fn exception(&self) -> Option<Exception> {
        let (code, sev) = match self {
            StoreError::Integrity(_) => ("SYS-010", Severity::Blocker),
            StoreError::Chain(_) => ("SYS-011", Severity::Blocker),
            StoreError::UnsafeLocation(_) => ("SYS-015", Severity::Blocker),
            StoreError::SchemaTooNew { .. } => ("SYS-016", Severity::Warn),
            _ => return None,
        };
        Some(Exception::must(code, sev, Scope::Exam).with_detail(self.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_catalogue_codes() {
        let e = StoreError::Integrity("page 3".into()).exception().unwrap();
        assert_eq!(
            (e.code.as_str(), e.severity),
            ("SYS-010", Severity::Blocker)
        );
        let e = StoreError::SchemaTooNew {
            found: 9,
            supported: 2,
        }
        .exception()
        .unwrap();
        assert_eq!((e.code.as_str(), e.severity), ("SYS-016", Severity::Warn));
        let e = StoreError::UnsafeLocation("nfs".into())
            .exception()
            .unwrap();
        assert_eq!(e.code, "SYS-015");
        assert!(StoreError::NotFound("x".into()).exception().is_none());
    }
}
