//! Master items and item types (spec §10.3, §10.4).

use crate::num::Points;
use serde::{Deserialize, Serialize};

/// Most options an item can print (A–Z); option sets are 26-bit masks (§10.1).
pub const MAX_OPTIONS: u8 = 26;

/// How a multi-select item is answered (§10.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SelectMode {
    /// "Pick N": the stem tells the student how many options to mark.
    PickN(u8),
    /// "Select all that apply".
    SelectAll,
}

/// Item types supported by the MVP rule table (§10.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemKind {
    /// Single best answer, 2–26 options; several accepted answers allowed (Remark "OR").
    SingleBestAnswer,
    /// True/false: a two-option single best answer.
    TrueFalse,
    /// Multi-select with a keyed set K and a partial-credit rule (§10.4).
    MultiSelect(SelectMode),
    /// Score box or imported points column: s = d with 0 ≤ d ≤ M_i (§10.1 `imported`).
    ScoreBox,
}

impl ItemKind {
    /// SBA, TF (and grid-in, not in the MVP) are indivisible evidence (SCO-034, §11.2).
    pub fn is_indivisible(self) -> bool {
        matches!(self, ItemKind::SingleBestAnswer | ItemKind::TrueFalse)
    }

    /// True for items answered by choosing exactly one option.
    pub fn is_choice(self) -> bool {
        matches!(self, ItemKind::SingleBestAnswer | ItemKind::TrueFalse)
    }
}

/// A master item {label, type, M_i, m_i} (§10.3). CLO mappings live in `omr-outcomes`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MasterItem {
    /// Stable display label in master order, e.g. `Q12`.
    pub label: String,
    /// Item type.
    pub kind: ItemKind,
    /// Printed options m_i (0 for score boxes).
    pub options: u8,
    /// Item maximum M_i (> 0).
    pub max_points: Points,
}

impl MasterItem {
    /// A single-best-answer item.
    pub fn sba(label: impl Into<String>, options: u8, max_points: Points) -> Self {
        Self {
            label: label.into(),
            kind: ItemKind::SingleBestAnswer,
            options,
            max_points,
        }
    }

    /// A true/false item.
    pub fn true_false(label: impl Into<String>, max_points: Points) -> Self {
        Self {
            label: label.into(),
            kind: ItemKind::TrueFalse,
            options: 2,
            max_points,
        }
    }

    /// A multi-select item.
    pub fn multi_select(
        label: impl Into<String>,
        options: u8,
        mode: SelectMode,
        max_points: Points,
    ) -> Self {
        Self {
            label: label.into(),
            kind: ItemKind::MultiSelect(mode),
            options,
            max_points,
        }
    }

    /// A score box (or imported points column) with maximum M_i.
    pub fn score_box(label: impl Into<String>, max_points: Points) -> Self {
        Self {
            label: label.into(),
            kind: ItemKind::ScoreBox,
            options: 0,
            max_points,
        }
    }

    /// Mask of the printed options O_i.
    pub fn option_mask(&self) -> u32 {
        if self.options == 0 {
            0
        } else {
            (1u32 << self.options) - 1
        }
    }
}

/// Bit mask from 0-based option indices (indices ≥ 26 are ignored).
pub fn mask_of(options: &[u8]) -> u32 {
    options
        .iter()
        .filter(|&&o| o < MAX_OPTIONS)
        .fold(0u32, |m, &o| m | (1 << o))
}

/// 0-based option indices of a mask, ascending.
pub fn options_of(mask: u32) -> Vec<u8> {
    (0..MAX_OPTIONS).filter(|&o| mask & (1 << o) != 0).collect()
}
