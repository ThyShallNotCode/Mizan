//! Identifiers (spec §6.4, §9.2, §14.3).

use serde::{Deserialize, Serialize};
use std::fmt;

/// An exam (assessment) identifier: a UUIDv7, stable across template revisions (§6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExamId(pub uuid::Uuid);

impl ExamId {
    /// A fresh, time-ordered exam ID.
    pub fn new_v7() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl fmt::Display for ExamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Template revision (10 bits in the page code, §6.4). Printing a revision freezes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TemplateRevision(pub u16);

/// Variant/form code (10 bits, §6.4): `0` = per-booklet shuffle, `1` = form A, `2` = form B, …
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FormCode(pub u16);

impl FormCode {
    /// Form letter (`A` for code 1) or `*` for per-booklet shuffles.
    pub fn letter(self) -> char {
        match self.0 {
            0 => '*',
            n if n <= 26 => (b'A' + (n - 1) as u8) as char,
            _ => '?',
        }
    }
}

/// Page (each printed side counts), 1-based (§6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageNo(pub u16);

/// Booklet serial (30 bits per exam); `0` = generic, non-personalised copy (§6.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BookletSerial(pub u32);

impl BookletSerial {
    /// The generic-copy serial `000000`.
    pub const GENERIC: Self = Self(0);
}

/// A normalised SIS student ID, always text (rosters keep leading zeros, §9.2).
/// Never a national ID / Iqama number (§9.2, §9.10).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StudentKey(pub String);

impl fmt::Display for StudentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
