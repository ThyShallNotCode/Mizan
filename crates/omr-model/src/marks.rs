//! Mark classes, cancel modes and question responses (spec §8.7, §8.8).

use crate::exceptions::Exception;
use crate::fixed::LogitMilli;
use serde::{Deserialize, Serialize};

/// Appearance class of one bubble (§8.7 cascade output; §8.8 table rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarkClass {
    /// Nothing but the printed outline and label.
    Empty,
    /// Faint graphite left by an erasure.
    ErasedResidue,
    /// A deliberate, mostly solid fill.
    Filled,
    /// A deliberate but light fill (soft pencil, low pressure).
    LightFill,
    /// A fill with a cross through it (a cancellation in `cross_cancels`).
    CrossedOut,
    /// A tick or an X mark rather than a fill.
    CheckOrX,
    /// The option label circled instead of filled.
    CircledLabel,
    /// A dot or partial fill.
    PartialOrDot,
    /// A stray stroke or paper damage.
    StrayOrDamage,
    /// Correction fluid or tape.
    Whiteout,
}

/// Per-exam marking convention; mandatory, **no default** (§6.5, §8.8, lint L14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CancelMode {
    /// Students erase to change an answer; crossing out goes to review.
    EraseOnly,
    /// Crossing out a fill cancels it.
    CrossCancels,
    /// A tick selects and a fill cancels (AMC/SDAPS convention).
    FillCancelsCheck,
}

/// The response decided for one question (§8.8 outcome table).
///
/// Option indices are 0-based positions on the printed form (variant order, not master order;
/// the score run maps them through π_f/σ_f, §10.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Response {
    /// Exactly one option selected.
    Single(u8),
    /// No option selected.
    Blank,
    /// More than one option selected on a single-answer item.
    Multi(Vec<u8>),
    /// Selected options of a multi-select item.
    Set(Vec<u8>),
    /// Evidence was unobservable for these bubbles: **never** treated as blank (invariant 4).
    Unreadable {
        /// Bubbles that could not be observed.
        bubbles: Vec<u8>,
    },
}

impl Response {
    /// True for [`Response::Unreadable`].
    pub fn is_unreadable(&self) -> bool {
        matches!(self, Response::Unreadable { .. })
    }
}

/// Decision for one question: the best response, whether it is auto-accepted, and why not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionOutcome {
    /// 0-based question position on the form.
    pub question: u16,
    /// The most probable response (the reviewer's starting point when not auto-accepted).
    pub response: Response,
    /// True only when every §8.8 auto-accept condition holds.
    pub auto_accepted: bool,
    /// Log-odds of the decided response.
    pub logit: LogitMilli,
    /// Exceptions raised for this question (MRK-xxx, CAP-xxx, REG-xxx).
    pub exceptions: Vec<Exception>,
}
