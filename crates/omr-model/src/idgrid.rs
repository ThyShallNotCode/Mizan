//! The recognition → identity contract for bubbled ID and version grids (spec §8.8, §9.5).
//!
//! Recognition decodes each ID column as a single-answer question but never auto-accepts it;
//! it hands these per-column reads to `omr-identity`, which decodes against the roster.

use crate::fixed::LogitMilli;
use serde::{Deserialize, Serialize};

/// State of one ID-grid column (§9.9 row 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColumnState {
    /// Exactly one digit confidently marked.
    Single,
    /// No digit marked (a wildcard for roster decoding).
    Blank,
    /// More than one digit marked.
    Multi,
    /// Evidence unobservable (occlusion, cut-off, failed registration): never blank.
    Unreadable,
}

/// One column of the bubbled ID grid.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdColumnRead {
    /// Column position, 0 = leftmost digit.
    pub column: u8,
    /// Column state.
    pub state: ColumnState,
    /// Digits judged marked (0–9), ascending.
    pub marked: Vec<u8>,
    /// Per-digit selection log-odds (index = digit 0–9); used for roster-constrained decoding.
    pub logits: Vec<LogitMilli>,
}

/// The whole bubbled ID grid as read from one page.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct IdGridRead {
    /// Columns left to right.
    pub columns: Vec<IdColumnRead>,
}

impl IdGridRead {
    /// The digit string if every column is `Single`, else `None`.
    pub fn exact_digits(&self) -> Option<String> {
        self.columns
            .iter()
            .map(|c| match (c.state, c.marked.as_slice()) {
                (ColumnState::Single, [d]) => Some(char::from(b'0' + d)),
                _ => None,
            })
            .collect()
    }
}
