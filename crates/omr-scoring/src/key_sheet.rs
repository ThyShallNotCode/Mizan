//! Key sheets as a key source (spec §10.1 "Sources"): a teacher-filled answer sheet is read by
//! the key-blind recognizer like any other sheet, anywhere in the stack; this module routes
//! what was read into key entries. **Key-sheet responses are never auto-accepted**: every row
//! is a proposal that the instructor confirms on a rendered key summary, and a row that is not
//! one confidently read option needs the instructor's decision first.
//!
//! The publish checks of §10.1 apply to the rows: SCO-003 (BLOCKER) for a blank keyed item and
//! for a row not read confidently (no key entry until decided), SCO-004 (BLOCKER) for an option
//! the exam does not print, SCO-027 for several marks on a single-answer key without an
//! explicit multi-accept. Appendix A gives SCO-027 as a WARN ("confirm with a reason"); here it
//! is raised as a WARN and the row still waits for the instructor's explicit choice, so a
//! multi-mark is never published as a multi-accepted key by default.

use crate::key::KeyEntry;
use omr_model::exceptions::Target;
use omr_model::{Exception, Response, Scope, Severity};
use serde::{Deserialize, Serialize};

/// What one key-sheet row gives the key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum SheetCell {
    /// One option read confidently: the keyed answer (still to be confirmed with the sheet).
    Key {
        /// 0-based option.
        option: u8,
    },
    /// Several options read confidently: a multi-mark (SCO-027) the instructor resolves, as one
    /// option or as an explicit multi-accept.
    MultiMark {
        /// 0-based options, ascending.
        options: Vec<u8>,
    },
    /// Read confidently as blank: a blank keyed item (SCO-003).
    Blank,
    /// A mark on an option the exam does not print or use (SCO-004).
    NotPrinted {
        /// 0-based option.
        option: u8,
    },
    /// Not read confidently (light, ambiguous, erased or unreadable): no key entry until the
    /// instructor decides (SCO-003). Carries the recognizer's proposal, if any.
    Uncertain {
        /// The recognizer's most probable response.
        proposal: Option<Response>,
    },
}

impl SheetCell {
    /// Routes one recognised row: `confident` is the recognizer's auto-accept verdict, `options`
    /// the number of options the exam uses (m_i).
    pub fn route(response: &Response, confident: bool, options: u8) -> Self {
        let outside = |v: &[u8]| v.iter().copied().find(|o| *o >= options);
        if !confident {
            return SheetCell::Uncertain {
                proposal: (!matches!(response, Response::Unreadable { .. }))
                    .then(|| response.clone()),
            };
        }
        match response {
            Response::Single(o) if *o >= options => SheetCell::NotPrinted { option: *o },
            Response::Single(o) => SheetCell::Key { option: *o },
            Response::Blank => SheetCell::Blank,
            Response::Multi(v) | Response::Set(v) => match outside(v) {
                Some(o) => SheetCell::NotPrinted { option: o },
                None => {
                    let mut v = v.clone();
                    v.sort_unstable();
                    v.dedup();
                    SheetCell::MultiMark { options: v }
                }
            },
            Response::Unreadable { .. } => SheetCell::Uncertain { proposal: None },
        }
    }

    /// The keyed option of a confidently read single mark.
    pub fn key(&self) -> Option<u8> {
        match self {
            SheetCell::Key { option } => Some(*option),
            _ => None,
        }
    }

    /// True when the row needs the instructor's decision before the key can be confirmed.
    pub fn needs_decision(&self) -> bool {
        self.key().is_none()
    }

    /// The key entry of a confidently read single mark (`None` otherwise: never guessed).
    pub fn entry(&self, options: u8) -> Option<KeyEntry> {
        self.key().map(|o| KeyEntry::single(o, options))
    }

    /// The publish check (§10.1) a row raises until it is decided: SCO-003 (blank, or no
    /// confident mark: no key entry), SCO-004 (option not printed), SCO-027 (multi-mark).
    pub fn code(&self) -> Option<&'static str> {
        match self {
            SheetCell::Key { .. } => None,
            SheetCell::Blank | SheetCell::Uncertain { .. } => Some("SCO-003"),
            SheetCell::NotPrinted { .. } => Some("SCO-004"),
            SheetCell::MultiMark { .. } => Some("SCO-027"),
        }
    }

    /// Why the row needs a decision, for the key summary and refusals (R10: what to do).
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            SheetCell::Key { .. } => None,
            SheetCell::MultiMark { .. } => Some(
                "several marks on a single-answer question: choose the answer, or accept the \
                 marked options explicitly (A|C)",
            ),
            SheetCell::Blank => {
                Some("no mark: a blank keyed question; give its answer, or void it (*)")
            }
            SheetCell::NotPrinted { .. } => Some(
                "a mark on an option the exam does not use: give the answer, or void the \
                 question (*)",
            ),
            SheetCell::Uncertain { .. } => Some(
                "the mark was not read confidently (light, erased or ambiguous): check the \
                 crop and give the answer",
            ),
        }
    }
}

/// The publish findings (§10.1) of one key sheet's rows: SCO-003 for a blank or uncertain row,
/// SCO-004 for an option not printed, SCO-027 for a multi-mark. Each names the version and the
/// question (target `question`, 0-based); rows read as one confident option raise nothing.
pub fn findings(cells: &[SheetCell], version: &str) -> Vec<Exception> {
    let mut out = Vec::new();
    for (q, c) in cells.iter().enumerate() {
        let Some(code) = c.code() else {
            continue;
        };
        let severity = if code == "SCO-027" {
            Severity::Warn
        } else {
            Severity::Blocker
        };
        out.push(
            Exception::must(code, severity, Scope::Exam)
                .with_target(Target {
                    question: Some(q as u16),
                    ..Target::default()
                })
                .with_detail(format!(
                    "key sheet of version {version}, question {}: {}",
                    q + 1,
                    c.reason().unwrap_or_default()
                )),
        );
    }
    out
}

/// The rows of several key sheets of one version combined: equal rows stand, rows that differ
/// between the sheets become [`SheetCell::Uncertain`] (never one sheet's answer over another's).
pub fn combine(sheets: &[Vec<SheetCell>]) -> Vec<SheetCell> {
    let n = sheets.iter().map(Vec::len).max().unwrap_or(0);
    (0..n)
        .map(|q| {
            let mut it = sheets.iter().map(|s| s.get(q));
            let first = it.next().flatten();
            match first {
                Some(c) if it.all(|o| o == Some(c)) => c.clone(),
                _ => SheetCell::Uncertain { proposal: None },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_route_by_confidence_and_printed_options() {
        assert_eq!(
            SheetCell::route(&Response::Single(1), true, 5),
            SheetCell::Key { option: 1 }
        );
        assert_eq!(
            SheetCell::route(&Response::Single(4), true, 4),
            SheetCell::NotPrinted { option: 4 }
        );
        assert_eq!(
            SheetCell::route(&Response::Multi(vec![2, 0]), true, 5),
            SheetCell::MultiMark {
                options: vec![0, 2]
            }
        );
        assert_eq!(
            SheetCell::route(&Response::Blank, true, 5),
            SheetCell::Blank
        );
        // Never auto-accepted: an unconfident single is a proposal only.
        assert_eq!(
            SheetCell::route(&Response::Single(2), false, 5),
            SheetCell::Uncertain {
                proposal: Some(Response::Single(2))
            }
        );
        assert_eq!(
            SheetCell::route(&Response::Unreadable { bubbles: vec![1] }, true, 5),
            SheetCell::Uncertain { proposal: None }
        );
        assert_eq!(
            SheetCell::route(&Response::Single(1), true, 5).entry(5),
            Some(KeyEntry::single(1, 5))
        );
        assert_eq!(SheetCell::Blank.entry(5), None);
    }

    #[test]
    fn findings_use_the_publish_codes() {
        let cells = vec![
            SheetCell::Key { option: 0 },
            SheetCell::Blank,
            SheetCell::NotPrinted { option: 4 },
            SheetCell::MultiMark {
                options: vec![0, 2],
            },
            SheetCell::Uncertain { proposal: None },
        ];
        let f = findings(&cells, "A");
        let codes: Vec<(&str, Severity, Option<u16>)> = f
            .iter()
            .map(|e| (e.code.as_str(), e.severity, e.target.question))
            .collect();
        assert_eq!(
            codes,
            vec![
                ("SCO-003", Severity::Blocker, Some(1)),
                ("SCO-004", Severity::Blocker, Some(2)),
                ("SCO-027", Severity::Warn, Some(3)),
                ("SCO-003", Severity::Blocker, Some(4)),
            ]
        );
        assert!(f[0].detail.contains("version A, question 2"));
    }

    #[test]
    fn sheets_that_disagree_never_pick_one() {
        let a = vec![SheetCell::Key { option: 0 }, SheetCell::Key { option: 1 }];
        let b = vec![SheetCell::Key { option: 0 }, SheetCell::Key { option: 2 }];
        assert_eq!(
            combine(&[a, b]),
            vec![
                SheetCell::Key { option: 0 },
                SheetCell::Uncertain { proposal: None }
            ]
        );
    }
}
