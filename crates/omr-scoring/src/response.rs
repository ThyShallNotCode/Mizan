//! L1 responses and their L2 projection into master space as a compact [`ResponseMatrix`]
//! (spec §10.1 "Data structures", §10.3).

use crate::error::ScoringError;
use crate::form::FormMap;
use crate::item::{ItemKind, MAX_OPTIONS, mask_of};
use crate::num::Points;
use num_traits::Signed;
use omr_model::exceptions::Target;
use omr_model::{BookletSerial, Exception, FormCode, Response, Scope, Severity, StudentKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A reviewed response at one printed position (L1, §10.1). Options are printed indices.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum L1Response {
    /// One option.
    Single(u8),
    /// Options of a multi-select item.
    Set(Vec<u8>),
    /// Nothing marked.
    Blank,
    /// A confirmed multi-mark on a single-answer item.
    Multi(Vec<u8>),
    /// Unobservable: never scored as blank (invariant 4); the item is `Pending`.
    Unreadable,
    /// Unresolved review state: the item is `Pending`.
    Pending,
    /// A score-box value d (0 ≤ d ≤ M_i).
    Points(Points),
    /// Skip-logic condition false (§6.5).
    NotApplicable,
}

impl From<&Response> for L1Response {
    fn from(r: &Response) -> Self {
        match r {
            Response::Single(o) => L1Response::Single(*o),
            Response::Blank => L1Response::Blank,
            Response::Multi(v) => L1Response::Multi(v.clone()),
            Response::Set(v) => L1Response::Set(v.clone()),
            Response::Unreadable { .. } => L1Response::Unreadable,
        }
    }
}

impl L1Response {
    /// True when something was marked or entered.
    pub fn is_answered(&self) -> bool {
        match self {
            L1Response::Single(_) | L1Response::Points(_) => true,
            L1Response::Set(v) | L1Response::Multi(v) => !v.is_empty(),
            _ => false,
        }
    }
}

/// Attendance from the absent list or roster (§9.9, §10.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Attendance {
    /// Sat the assessment.
    #[default]
    Present,
    /// Absent with an accepted excuse.
    AbsentExcused,
    /// Absent without excuse.
    AbsentUnexcused,
}

/// One booklet's reviewed responses (L1) in printed order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BookletL1 {
    /// Booklet serial (unique within the snapshot).
    pub booklet: BookletSerial,
    /// Bound student, if identified.
    pub student: Option<StudentKey>,
    /// Form the booklet was printed as.
    pub form: FormCode,
    /// Attendance status; absent rows normally carry no responses.
    pub attendance: Attendance,
    /// The administrative "absent" bubble was filled (SCO-031 if answers are present).
    pub absent_bubble: bool,
    /// Responses per printed position (empty for an absent student).
    pub responses: Vec<L1Response>,
    /// Printed positions covered by an open review exception (so `Unreadable` there is
    /// expected and does not raise SCO-008).
    pub open_exceptions: BTreeSet<u16>,
}

impl BookletL1 {
    /// A present booklet with `responses` and no open exceptions.
    pub fn present(
        booklet: u32,
        student: Option<&str>,
        form: FormCode,
        responses: Vec<L1Response>,
    ) -> Self {
        Self {
            booklet: BookletSerial(booklet),
            student: student.map(|s| StudentKey(s.to_owned())),
            form,
            attendance: Attendance::Present,
            absent_bubble: false,
            responses,
            open_exceptions: BTreeSet::new(),
        }
    }

    /// An absent student (no responses).
    pub fn absent(booklet: u32, student: Option<&str>, form: FormCode, excused: bool) -> Self {
        Self {
            booklet: BookletSerial(booklet),
            student: student.map(|s| StudentKey(s.to_owned())),
            form,
            attendance: if excused {
                Attendance::AbsentExcused
            } else {
                Attendance::AbsentUnexcused
            },
            absent_bubble: false,
            responses: Vec::new(),
            open_exceptions: BTreeSet::new(),
        }
    }
}

/// The L1 snapshot a score run reads (§10.1 signature `l1: &ResponseSnapshot`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResponseSnapshot {
    /// Booklets in a stable order (rows of every matrix follow it).
    pub booklets: Vec<BookletL1>,
}

/// Cell encoding of [`ResponseMatrix`]: bits 0–25 option set (or side-table index for
/// points), bits 26–29 state (§10.1).
pub mod cell {
    /// Shift of the state field.
    pub const STATE_SHIFT: u32 = 26;
    /// Mask of the payload field.
    pub const PAYLOAD: u32 = (1 << STATE_SHIFT) - 1;
    /// Options answered (payload = master option mask; >1 bit on a choice item = Multi).
    pub const ANSWERED: u32 = 0;
    /// Nothing marked.
    pub const BLANK: u32 = 1;
    /// Item not presented on this booklet's form (or student absent).
    pub const NOT_PRESENTED: u32 = 2;
    /// Skip-logic condition false.
    pub const NOT_APPLICABLE: u32 = 3;
    /// Unreadable or unresolved: never scored as blank.
    pub const PENDING: u32 = 4;
    /// Score-box value (payload = side-table index).
    pub const POINTS: u32 = 5;

    /// Packs a state and payload.
    pub const fn pack(state: u32, payload: u32) -> u32 {
        (state << STATE_SHIFT) | (payload & PAYLOAD)
    }

    /// State field.
    pub const fn state(c: u32) -> u32 {
        c >> STATE_SHIFT
    }

    /// Payload field.
    pub const fn payload(c: u32) -> u32 {
        c & PAYLOAD
    }
}

/// A decoded master-space cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    /// Master option mask.
    Answered(u32),
    /// Blank.
    Blank,
    /// Not presented / absent.
    NotPresented,
    /// Not applicable.
    NotApplicable,
    /// Pending.
    Pending,
    /// Score-box value.
    Points(Points),
}

/// Per-row metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowMeta {
    /// Booklet serial.
    pub booklet: BookletSerial,
    /// Bound student.
    pub student: Option<StudentKey>,
    /// Index into `FormMap::forms`.
    pub form: usize,
    /// `Some(excused)` when the student is absent (no answers).
    pub absent: Option<bool>,
}

/// N × I master-space cells, column-major (`cells[i * n_rows + n]`), 4 bytes per cell:
/// 10k × 100 = 4 MB (§10.1). Independent of the key, so a key change never re-projects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseMatrix {
    /// Booklets (rows).
    pub n_rows: usize,
    /// Master items (columns).
    pub n_items: usize,
    /// Encoded cells, column-major.
    pub cells: Vec<u32>,
    /// Score-box side table.
    pub points: Vec<Points>,
    /// Row metadata.
    pub rows: Vec<RowMeta>,
}

impl ResponseMatrix {
    /// Raw cell of row `n`, item `i`.
    #[inline]
    pub fn raw(&self, n: usize, i: usize) -> u32 {
        self.cells[i * self.n_rows + n]
    }

    /// Column of item `i`.
    pub fn column(&self, i: usize) -> &[u32] {
        &self.cells[i * self.n_rows..(i + 1) * self.n_rows]
    }

    /// Decoded cell.
    pub fn cell(&self, n: usize, i: usize) -> Cell {
        let c = self.raw(n, i);
        match cell::state(c) {
            cell::ANSWERED => Cell::Answered(cell::payload(c)),
            cell::BLANK => Cell::Blank,
            cell::NOT_APPLICABLE => Cell::NotApplicable,
            cell::PENDING => Cell::Pending,
            cell::POINTS => Cell::Points(self.points[cell::payload(c) as usize]),
            _ => Cell::NotPresented,
        }
    }

    /// L2 projection (§10.3): each printed response (f, j, ℓ) becomes (π_f(j), σ(ℓ)).
    ///
    /// Raises SCO-008 (ERROR, Booklet) for an `Unreadable`/`Pending` response not covered by
    /// an open exception, MRK-013 (ERROR, Item) for a score-box value above M_i, and SCO-031
    /// (WARN, Booklet) when the absent bubble (or absent list) disagrees with present answers;
    /// such a booklet is scored, never auto-zeroed.
    pub fn project(
        snapshot: &ResponseSnapshot,
        forms: &FormMap,
    ) -> Result<(Self, Vec<Exception>), ScoringError> {
        let n_rows = snapshot.booklets.len();
        let n_items = forms.items.len();
        let mut cells = vec![cell::pack(cell::NOT_PRESENTED, 0); n_rows * n_items];
        let mut points = Vec::new();
        let mut rows = Vec::with_capacity(n_rows);
        let mut ex = Vec::new();
        for (n, b) in snapshot.booklets.iter().enumerate() {
            let f = forms.form_index(b.form).ok_or(ScoringError::UnknownForm {
                booklet: b.booklet.0,
                form: b.form.0,
            })?;
            let form = &forms.forms[f];
            let answered = b.responses.iter().any(L1Response::is_answered);
            let absent = if b.attendance != Attendance::Present || b.absent_bubble {
                if answered {
                    ex.push(
                        Exception::must("SCO-031", Severity::Warn, Scope::Booklet)
                            .with_target(Target {
                                booklet: Some(b.booklet.0),
                                ..Target::default()
                            })
                            .with_detail(
                                "absent mark but answers present; scored, never auto-zeroed",
                            ),
                    );
                    None
                } else {
                    Some(b.attendance == Attendance::AbsentExcused)
                }
            } else {
                None
            };
            rows.push(RowMeta {
                booklet: b.booklet,
                student: b.student.clone(),
                form: f,
                absent,
            });
            if absent.is_some() {
                continue;
            }
            if b.responses.len() != form.positions.len() {
                return Err(ScoringError::LengthMismatch {
                    booklet: b.booklet.0,
                    expected: form.positions.len(),
                    got: b.responses.len(),
                });
            }
            for (j, (pos, r)) in form.positions.iter().zip(&b.responses).enumerate() {
                let i = pos.item;
                let Some(item) = forms.items.get(i) else {
                    return Err(ScoringError::InvalidReference(format!(
                        "form {}: position {j} maps to unknown master item {i}",
                        b.form.0
                    )));
                };
                let target = Target {
                    booklet: Some(b.booklet.0),
                    question: Some(j as u16),
                    ..Target::default()
                };
                let check_opts = |opts: &[u8]| -> Result<u32, ScoringError> {
                    for &o in opts {
                        if o >= item.options || o >= MAX_OPTIONS {
                            return Err(ScoringError::OptionOutOfRange {
                                booklet: b.booklet.0,
                                position: j,
                                option: o,
                            });
                        }
                    }
                    Ok(mask_of(opts))
                };
                let c = match r {
                    L1Response::Single(o) => {
                        let m = check_opts(&[*o])?;
                        cell::pack(cell::ANSWERED, form.map_mask(j, m))
                    }
                    L1Response::Set(v) | L1Response::Multi(v) => {
                        let m = check_opts(v)?;
                        if m == 0 {
                            cell::pack(cell::BLANK, 0)
                        } else {
                            cell::pack(cell::ANSWERED, form.map_mask(j, m))
                        }
                    }
                    L1Response::Blank => cell::pack(cell::BLANK, 0),
                    L1Response::NotApplicable => cell::pack(cell::NOT_APPLICABLE, 0),
                    L1Response::Unreadable | L1Response::Pending => {
                        if !b.open_exceptions.contains(&(j as u16)) {
                            ex.push(
                                Exception::must("SCO-008", Severity::Error, Scope::Booklet)
                                    .with_target(target)
                                    .with_detail(format!(
                                        "{r:?} response at position {} reached scoring with no \
                                         open exception; item pending, back to review",
                                        j + 1
                                    )),
                            );
                        }
                        cell::pack(cell::PENDING, 0)
                    }
                    L1Response::Points(d) => {
                        if item.kind != ItemKind::ScoreBox {
                            return Err(ScoringError::InvalidResponse {
                                booklet: b.booklet.0,
                                position: j,
                                reason: "points response on a non-score-box item".into(),
                            });
                        }
                        if d.is_negative() || *d > item.max_points {
                            ex.push(
                                Exception::must("MRK-013", Severity::Error, Scope::Item)
                                    .with_target(target)
                                    .with_detail(format!(
                                        "score-box value {d} outside [0, {}]",
                                        item.max_points
                                    )),
                            );
                            cell::pack(cell::PENDING, 0)
                        } else {
                            if points.len() >= cell::PAYLOAD as usize {
                                return Err(ScoringError::Overflow(
                                    "score-box side table full".into(),
                                ));
                            }
                            points.push(*d);
                            cell::pack(cell::POINTS, (points.len() - 1) as u32)
                        }
                    }
                };
                if item.kind == ItemKind::ScoreBox
                    && matches!(
                        r,
                        L1Response::Single(_) | L1Response::Set(_) | L1Response::Multi(_)
                    )
                {
                    return Err(ScoringError::InvalidResponse {
                        booklet: b.booklet.0,
                        position: j,
                        reason: "option response on a score-box item".into(),
                    });
                }
                cells[i * n_rows + n] = c;
            }
        }
        Ok((
            Self {
                n_rows,
                n_items,
                cells,
                points,
                rows,
            },
            ex,
        ))
    }
}
