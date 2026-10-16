//! Errors of the scoring API (input shape and arithmetic limits). Domain findings are
//! Appendix A [`omr_model::Exception`]s, never these errors.

use omr_model::Exception;
use thiserror::Error;

/// Why a score run could not be produced.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScoringError {
    /// A booklet names a form the form map does not have.
    #[error("booklet {booklet}: unknown form code {form}")]
    UnknownForm {
        /// Booklet serial.
        booklet: u32,
        /// Form code.
        form: u16,
    },
    /// A booklet has a different number of responses than its form has positions.
    #[error("booklet {booklet}: {got} responses for {expected} printed positions")]
    LengthMismatch {
        /// Booklet serial.
        booklet: u32,
        /// Positions on the form.
        expected: usize,
        /// Responses given.
        got: usize,
    },
    /// A response names an option the item does not print.
    #[error("booklet {booklet}: option {option} at position {position} is not printed")]
    OptionOutOfRange {
        /// Booklet serial.
        booklet: u32,
        /// 0-based printed position.
        position: usize,
        /// Offending option.
        option: u8,
    },
    /// A response of the wrong kind for its item.
    #[error("booklet {booklet}: invalid response at position {position}: {reason}")]
    InvalidResponse {
        /// Booklet serial.
        booklet: u32,
        /// 0-based printed position.
        position: usize,
        /// Explanation.
        reason: String,
    },
    /// Publication validation refused the key, form map or policy (BLOCKER exceptions).
    #[error("refused by publish validation: {}", .0.iter().map(|e| format!("{} {}", e.code, e.detail)).collect::<Vec<_>>().join("; "))]
    Refused(Vec<Exception>),
    /// Exact arithmetic would overflow its fixed-width representation.
    #[error("exact arithmetic overflow: {0}")]
    Overflow(String),
    /// A lookup referred to something absent (e.g. an unknown item index).
    #[error("invalid reference: {0}")]
    InvalidReference(String),
}
