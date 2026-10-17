//! Scoring glue (§10): the form map and key in master space from the exam definition, the L1
//! snapshot from the reviewed booklets, and **incremental** rescoring: a review action patches
//! one booklet's row of the prepared response matrix and its L1 hash, then `score_prepared`
//! runs the whole key (A4: ≤ 50 ms for 10,000 sheets × 100 items).

use crate::AppError;
use crate::exam::ExamDefinition;
use crate::model::{Booklet, EffectiveKey, KeyCell, ResponseStatus, Session};
use omr_model::exceptions::Target;
use omr_model::{Exception, FormCode, Response, Scope, Severity, StudentKey};
use omr_scoring::run::hash_json;
use omr_scoring::{
    Adjustments, BookletL1, Form, FormMap, GradeOutcome, GradeScale, KeyEntry, KeyVersion,
    L1Response, MasterItem, PolicyVersion, Prepared, ResponseMatrix, ResponseSnapshot,
    RoundingPolicy, ScoreRun, Void, VoidMode, parse_points, score_prepared,
};
use std::collections::BTreeSet;

/// Master item label of question `q` (0-based) on form `f`.
pub fn item_label(key: &EffectiveKey, forms: &[String], f: usize, q: usize) -> String {
    if key.shared_items {
        format!("Q{}", q + 1)
    } else {
        format!("{}-Q{}", forms[f], q + 1)
    }
}

/// Master item of printed question `q` on form `f`.
pub fn master_item(key: &EffectiveKey, n_questions: usize, f: usize, q: usize) -> usize {
    if key.shared_items {
        q
    } else {
        f * n_questions + q
    }
}

/// The form map (§10.3): identical forms share master items; otherwise each form presents
/// its own items (no cross-form mapping declared).
pub fn form_map(def: &ExamDefinition, key: &EffectiveKey) -> Result<FormMap, AppError> {
    let n = usize::from(def.questions);
    let pts = |q: usize| -> Result<omr_scoring::Points, AppError> {
        let s = key.points.get(q).map(String::as_str).unwrap_or("1");
        parse_points(s).map_err(|e| AppError::Definition(format!("points `{s}`: {e}")))
    };
    let n_forms = if key.shared_items { 1 } else { def.forms.len() };
    let mut items = Vec::with_capacity(n * n_forms);
    for f in 0..n_forms {
        for q in 0..n {
            items.push(MasterItem::sba(
                item_label(key, &def.forms, f, q),
                def.options,
                pts(q)?,
            ));
        }
    }
    let forms = (0..def.forms.len())
        .map(|f| {
            let order: Vec<usize> = (0..n).map(|q| master_item(key, n, f, q)).collect();
            let maps: Vec<Vec<u8>> = (0..n).map(|_| (0..def.options).collect()).collect();
            Form::permuted(FormCode(f as u16 + 1), &order, &maps)
        })
        .collect();
    Ok(FormMap {
        items,
        forms,
        per_form_normalization: false,
    })
}

/// The key version and the voids it implies.
pub fn key_version(def: &ExamDefinition, key: &EffectiveKey) -> (KeyVersion, Vec<Void>) {
    let n = usize::from(def.questions);
    let n_forms = if key.shared_items { 1 } else { def.forms.len() };
    let mut entries = vec![None; n * n_forms];
    let mut voids = Vec::new();
    for f in 0..n_forms {
        for q in 0..n {
            let i = master_item(key, n, f, q);
            match key.forms.get(f).and_then(|k| k.get(q)) {
                Some(KeyCell::Options(v)) if v.len() == 1 => {
                    entries[i] = Some(KeyEntry::single(v[0], def.options));
                }
                Some(KeyCell::Options(v)) => {
                    entries[i] = Some(KeyEntry::any_of(v, def.options));
                }
                Some(KeyCell::Void) => {
                    let all: Vec<u8> = (0..def.options).collect();
                    entries[i] = Some(KeyEntry::any_of(&all, def.options));
                    voids.push(Void {
                        item: i,
                        mode: VoidMode::Drop,
                        reason: "ITEM_VOIDED_FLAWED".into(),
                    });
                }
                Some(KeyCell::Missing) | None => {}
            }
        }
    }
    (
        KeyVersion {
            version: key.version,
            reason: if key.inferred {
                "inferred".into()
            } else {
                "initial".into()
            },
            entries,
        },
        voids,
    )
}

/// The L1 response of one question (§10.1): pending while an exception is open, never blank.
fn l1_of(status: ResponseStatus, current: &Response) -> L1Response {
    match status {
        ResponseStatus::Pending => match current {
            Response::Unreadable { .. } => L1Response::Unreadable,
            _ => L1Response::Pending,
        },
        _ => L1Response::from(current),
    }
}

/// One booklet's L1 row (`None` when the booklet has no form). A held booklet (open MRK-015)
/// is pending at every position.
pub fn booklet_l1(b: &Booklet) -> Option<BookletL1> {
    let f = b.form?;
    let held = b.hold.is_some();
    let responses: Vec<L1Response> = b
        .questions
        .iter()
        .map(|q| {
            if held {
                L1Response::Pending
            } else {
                l1_of(q.status, &q.current)
            }
        })
        .collect();
    let open: BTreeSet<u16> = b
        .questions
        .iter()
        .enumerate()
        .filter(|(_, q)| held || q.status == ResponseStatus::Pending)
        .map(|(j, _)| j as u16)
        .collect();
    let mut l1 = BookletL1::present(b.serial, b.student(), FormCode(f as u16 + 1), responses);
    l1.student = b.identity.student.clone().map(StudentKey);
    l1.open_exceptions = open;
    Some(l1)
}

/// The scoring state of an exam: inputs, the prepared matrix and the current run.
#[derive(Clone, Debug)]
pub struct Scoring {
    /// Form map.
    pub forms: FormMap,
    /// Key version.
    pub key: KeyVersion,
    /// Policy version.
    pub policy: PolicyVersion,
    /// Adjustments (voids).
    pub adj: Adjustments,
    /// Booklet index per matrix row.
    pub rows: Vec<usize>,
    /// Row per booklet index.
    pub row_of: Vec<Option<usize>>,
    /// The L1 snapshot.
    pub snapshot: ResponseSnapshot,
    /// Prepared (projected) matrix.
    pub prep: Prepared,
    /// Per-row booklet hashes (for the incremental L1 hash).
    hashes: Vec<(u32, String)>,
    /// Per-row projection findings.
    row_exceptions: Vec<Vec<Exception>>,
    /// The current run, or why there is none.
    pub run: Result<ScoreRun, String>,
}

fn project_one(
    b: &BookletL1,
    forms: &FormMap,
) -> Result<(ResponseMatrix, Vec<Exception>), AppError> {
    ResponseMatrix::project(
        &ResponseSnapshot {
            booklets: vec![b.clone()],
        },
        forms,
    )
    .map_err(|e| AppError::Scoring(e.to_string()))
}

impl Scoring {
    /// Builds and runs scoring for a session (no key → `None`).
    pub fn build(session: &Session) -> Result<Option<Self>, AppError> {
        let Some(key) = &session.key else {
            return Ok(None);
        };
        let def = &session.definition;
        let forms = form_map(def, key)?;
        let (kv, voids) = key_version(def, key);
        let mut rows = Vec::new();
        let mut row_of = vec![None; session.booklets.len()];
        let mut booklets = Vec::new();
        for (i, b) in session.booklets.iter().enumerate() {
            if let Some(l1) = booklet_l1(b) {
                row_of[i] = Some(rows.len());
                rows.push(i);
                booklets.push(l1);
            }
        }
        let snapshot = ResponseSnapshot { booklets };
        let hashes = snapshot
            .booklets
            .iter()
            .map(|b| {
                Ok((
                    b.booklet.0,
                    hash_json(b).map_err(|e| AppError::Scoring(e.to_string()))?,
                ))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let mut row_exceptions = Vec::with_capacity(rows.len());
        for b in &snapshot.booklets {
            row_exceptions.push(project_one(b, &forms)?.1);
        }
        let prep = omr_scoring::prepare(&snapshot, &forms)
            .map_err(|e| AppError::Scoring(e.to_string()))?;
        let mut s = Self {
            forms,
            key: kv,
            policy: PolicyVersion::default(),
            adj: Adjustments {
                voids,
                ..Adjustments::default()
            },
            rows,
            row_of,
            snapshot,
            prep,
            hashes,
            row_exceptions,
            run: Err("not scored yet".into()),
        };
        s.rescore();
        Ok(Some(s))
    }

    /// Runs the score run on the prepared matrix (the regrade path).
    pub fn rescore(&mut self) {
        self.run = score_prepared(&self.prep, &self.forms, &self.key, &self.policy, &self.adj)
            .map_err(|e| match e {
                omr_scoring::ScoringError::Refused(ex) => format!(
                    "score run refused: {}",
                    ex.iter()
                        .map(|e| format!("{} {}", e.code, e.detail))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                other => other.to_string(),
            });
    }

    /// Patches booklet `bi`'s row after a review action and rescores (incremental L2).
    pub fn update_booklet(&mut self, bi: usize, b: &Booklet) -> Result<(), AppError> {
        let Some(row) = self.row_of.get(bi).copied().flatten() else {
            return Ok(());
        };
        let Some(l1) = booklet_l1(b) else {
            return Ok(());
        };
        let (one, ex) = project_one(&l1, &self.forms)?;
        let n = self.prep.matrix.n_rows;
        for i in 0..self.prep.matrix.n_items {
            self.prep.matrix.cells[i * n + row] = one.cells[i];
        }
        self.prep.matrix.rows[row] = one.rows[0].clone();
        self.hashes[row] = (
            l1.booklet.0,
            hash_json(&l1).map_err(|e| AppError::Scoring(e.to_string()))?,
        );
        let mut pairs = self.hashes.clone();
        pairs.sort();
        self.prep.l1_hash = hash_json(&pairs).map_err(|e| AppError::Scoring(e.to_string()))?;
        self.row_exceptions[row] = ex;
        self.prep.exceptions = self.row_exceptions.iter().flatten().cloned().collect();
        self.snapshot.booklets[row] = l1;
        self.rescore();
        Ok(())
    }

    /// Replaces the key (a new key version, L2) and rescores without re-projecting.
    pub fn set_key(&mut self, def: &ExamDefinition, key: &EffectiveKey) -> Result<(), AppError> {
        let forms = form_map(def, key)?;
        if forms != self.forms {
            return Err(AppError::Scoring(
                "the key change alters the form layout; regrade the exam".into(),
            ));
        }
        let (kv, voids) = key_version(def, key);
        self.key = kv;
        self.adj.voids = voids;
        self.rescore();
        Ok(())
    }

    /// Letter grades of the current run.
    pub fn grades(&self) -> Vec<Option<GradeOutcome>> {
        match &self.run {
            Ok(r) => r.grades(&GradeScale::saudi(), &RoundingPolicy::default()),
            Err(_) => Vec::new(),
        }
    }
}

/// The exam-scope exception of an inferred key (SCO-003 BLOCKER, `inferred_key`): the key is a
/// stand-in, so release is refused for every student (Appendix A: key missing).
pub fn inferred_key_exception() -> Exception {
    Exception::must("SCO-003", Severity::Blocker, Scope::Exam)
        .with_sub_reason("inferred_key")
        .with_target(Target::default())
        .with_detail(
            "INFERRED KEY: no answer key was supplied; each form's key is the modal confident \
             response per question. Results are provisional and can never be released.",
        )
}
