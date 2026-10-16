//! The score run: `score(l1, forms, key, policy, adj) -> ScoreRun` (spec §10.1 "Signature").
//!
//! Pure and deterministic: the same inputs give a byte-identical [`ScoreRun`] and the same
//! Run ID, so an identical regrade is a no-op for a memoizing caller.

use crate::error::ScoringError;
use crate::form::FormMap;
use crate::grade::{GradeOutcome, GradeScale};
use crate::item::ItemKind;
use crate::key::KeyVersion;
use crate::l3::{CompiledKey, ItemScoreMatrix, ItemState, mark_not_reached, score_matrix};
use crate::l4::{Adjustments, Counted, StudentResult, VoidMode, apply_l4};
use crate::num::{Exact, Points, RoundingPolicy, widen};
use crate::policy::PolicyVersion;
use crate::response::{ResponseMatrix, ResponseSnapshot, cell};
use crate::curve::FrozenCurve;
use crate::num::LetterBasis;
use num_traits::{ToPrimitive, Zero};
use omr_model::exceptions::Target;
use omr_model::{Exception, Scope, Severity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};

/// Engine identity folded into every Run ID (stands in for the engine manifest hash, §14.6).
pub const ENGINE: &str = concat!("omr-scoring/", env!("CARGO_PKG_VERSION"));

/// Lowercase hex SHA-384.
pub fn sha384_hex(bytes: &[u8]) -> String {
    let d = Sha384::digest(bytes);
    let mut s = String::with_capacity(96);
    for b in d.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// SHA-384 of the canonical JSON of `v` (serde field order, no whitespace).
pub fn hash_json<T: Serialize>(v: &T) -> Result<String, ScoringError> {
    serde_json::to_vec(v)
        .map(|b| sha384_hex(&b))
        .map_err(|e| ScoringError::InvalidReference(format!("hash serialization: {e}")))
}

/// `l1_hash` over sorted (booklet_id, booklet_response_hash) pairs (§10.1), so one
/// correction rehashes one booklet.
pub fn l1_hash(l1: &ResponseSnapshot) -> Result<String, ScoringError> {
    let mut pairs = l1
        .booklets
        .iter()
        .map(|b| Ok((b.booklet.0, hash_json(b)?)))
        .collect::<Result<Vec<_>, ScoringError>>()?;
    pairs.sort();
    hash_json(&pairs)
}

/// The key-independent half of a run: the L2 projection plus its exceptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// Master-space response matrix.
    pub matrix: ResponseMatrix,
    /// `l1_hash` of the snapshot.
    pub l1_hash: String,
    /// Projection findings (SCO-008, SCO-031, MRK-013).
    pub exceptions: Vec<Exception>,
}

/// Projects the L1 snapshot through the form map (done once per L1 change, never per key change).
pub fn prepare(l1: &ResponseSnapshot, forms: &FormMap) -> Result<Prepared, ScoringError> {
    let (matrix, exceptions) = ResponseMatrix::project(l1, forms)?;
    Ok(Prepared {
        matrix,
        l1_hash: l1_hash(l1)?,
        exceptions,
    })
}

/// Static metadata of one master item in a run.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemMeta {
    /// Label.
    pub label: String,
    /// Type.
    pub kind: ItemKind,
    /// m_i.
    pub options: u8,
    /// M_i.
    pub max_points: Points,
    /// Bonus item.
    pub bonus: bool,
    /// Void policy, if voided.
    pub void: Option<VoidMode>,
    /// Keyed (full-credit) master options.
    pub keyed: u32,
}

/// An immutable score run (L2–L4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreRun {
    /// SHA-384 over {l1, form map, key, policy, adjustments, engine} hashes.
    pub run_id: String,
    /// Key version used.
    pub key_version: u32,
    /// Policy version used.
    pub policy_version: u32,
    /// Master items.
    pub items: Vec<ItemMeta>,
    /// L3 item scores (uncurved, penalties included).
    pub l3: ItemScoreMatrix,
    /// L4 step-1 counted flags, column-major.
    pub counted: Vec<u8>,
    /// Per-student L4 results, in snapshot row order.
    pub students: Vec<StudentResult>,
    /// The curve applied, frozen.
    pub curve: Option<FrozenCurve>,
    /// All findings of the run (validation warnings, projection, L3, L4, checks).
    pub exceptions: Vec<Exception>,
}

impl ScoreRun {
    /// Number of booklets.
    pub fn n_rows(&self) -> usize {
        self.l3.n_rows
    }

    /// Exact uncurved L3 score s_{n,i}.
    pub fn item_score(&self, n: usize, i: usize) -> Points {
        self.l3.score(n, i)
    }

    /// L3 state of (n, i).
    pub fn item_state(&self, n: usize, i: usize) -> ItemState {
        self.l3.state(n, i)
    }

    /// L4 step-1 counted flag of (n, i).
    pub fn counted(&self, n: usize, i: usize) -> Counted {
        Counted::from_u8(self.counted[i * self.l3.n_rows + n])
    }

    /// M_a: Σ non-bonus M_i over items not voided by `Drop` (the assessment maximum used for
    /// the item grade share g_i = W_a·M_i/M_a, §11.2).
    pub fn max_non_bonus(&self) -> Exact {
        self.items
            .iter()
            .filter(|m| !m.bonus && m.void != Some(VoidMode::Drop))
            .map(|m| widen(&m.max_points))
            .sum()
    }

    /// True if any BLOCKER or ERROR is open in this run.
    pub fn blocks_release(&self) -> bool {
        self.exceptions.iter().any(|e| e.severity.blocks_release())
    }

    /// Assessment-level L6 letters (Remark parity: scale per test). Absent and not-scorable
    /// students get `None`. With `letter_basis = Rounded` the letter maps from the value
    /// rounded under `rounding`.
    pub fn grades(&self, scale: &GradeScale, rounding: &RoundingPolicy) -> Vec<Option<GradeOutcome>> {
        self.students
            .iter()
            .map(|s| {
                let x = s.final_pct?;
                let basis = match rounding.letter_basis {
                    LetterBasis::Rounded => rounding.round(&x),
                    LetterBasis::Unrounded => x,
                };
                GradeOutcome::letter_for(scale, &basis)
            })
            .collect()
    }
}

/// Configuration of the MRK-012 row-shift window test (§10.3; heuristic, unverified).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RowShiftConfig {
    /// Window length (≥ 6).
    pub window: usize,
    /// h_s threshold (≥ 5).
    pub min_shifted: usize,
    /// h₀ ceiling (≤ 1).
    pub max_aligned: usize,
}

impl Default for RowShiftConfig {
    fn default() -> Self {
        Self {
            window: 6,
            min_shifted: 5,
            max_aligned: 1,
        }
    }
}

/// Scores an L1 snapshot end to end (prepare + [`score_prepared`]).
pub fn score(
    l1: &ResponseSnapshot,
    forms: &FormMap,
    key: &KeyVersion,
    policy: &PolicyVersion,
    adj: &Adjustments,
) -> Result<ScoreRun, ScoringError> {
    let prep = prepare(l1, forms)?;
    score_prepared(&prep, forms, key, policy, adj)
}

/// Scores a prepared matrix: the key-change (regrade) path, budgeted at ≤ 50 ms for
/// 10,000 booklets × 100 items (§10.1, A4).
///
/// Publish validation runs first; any BLOCKER (SCO-003, SCO-004, SCO-029) refuses the run
/// with [`ScoringError::Refused`]. Non-blocking findings are carried in the run.
pub fn score_prepared(
    prep: &Prepared,
    forms: &FormMap,
    key: &KeyVersion,
    policy: &PolicyVersion,
    adj: &Adjustments,
) -> Result<ScoreRun, ScoringError> {
    let mut findings = forms.validate();
    findings.extend(key.validate(forms));
    findings.extend(policy.lint(forms));
    let blockers: Vec<Exception> = findings
        .iter()
        .filter(|e| e.severity == Severity::Blocker)
        .cloned()
        .collect();
    if !blockers.is_empty() {
        return Err(ScoringError::Refused(blockers));
    }
    let rm = &prep.matrix;
    let ck = CompiledKey::compile(forms, key, policy, &rm.points)?;
    let mut l3 = score_matrix(rm, &ck);
    let mut exceptions = findings;
    exceptions.extend(prep.exceptions.iter().cloned());
    exceptions.extend(mark_not_reached(&mut l3, rm, forms, policy));
    let max_scaled: Vec<i64> = (0..forms.items.len()).map(|i| ck.max_scaled(i)).collect();
    let l4 = apply_l4(rm, &l3, &max_scaled, forms, policy, adj);
    exceptions.extend(l4.exceptions.iter().cloned());
    exceptions.extend(row_shift_check(rm, forms, key, RowShiftConfig::default()));
    exceptions.extend(near_chance_check(rm, &l3, forms, key));
    let items: Vec<ItemMeta> = forms
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| ItemMeta {
            label: it.label.clone(),
            kind: it.kind,
            options: it.options,
            max_points: it.max_points,
            bonus: policy.bonus.contains(&i),
            void: adj.voids.iter().find(|v| v.item == i).map(|v| v.mode),
            keyed: key.entry(i).map_or(0, |e| e.keyed_mask()),
        })
        .collect();
    let run_id = {
        #[derive(Serialize)]
        struct RunIdInput<'a> {
            l1_hash: &'a str,
            form_map_hash: String,
            key_hash: String,
            policy_hash: String,
            adjustments_hash: String,
            engine: &'a str,
        }
        hash_json(&RunIdInput {
            l1_hash: &prep.l1_hash,
            form_map_hash: hash_json(forms)?,
            key_hash: hash_json(key)?,
            policy_hash: hash_json(policy)?,
            adjustments_hash: hash_json(adj)?,
            engine: ENGINE,
        })?
    };
    Ok(ScoreRun {
        run_id,
        key_version: key.version,
        policy_version: policy.version,
        items,
        l3,
        counted: l4.counted,
        students: l4.students,
        curve: l4.curve,
        exceptions,
    })
}

/// MRK-012 (WARN, Booklet): in a window of printed positions, h_s ≥ 5 responses equal the
/// key at j + s (s = ±1) while h₀ ≤ 1 match the key in place (§10.3). Flag only; shifting
/// is a manual L1 correction.
pub fn row_shift_check(
    rm: &ResponseMatrix,
    forms: &FormMap,
    key: &KeyVersion,
    cfg: RowShiftConfig,
) -> Vec<Exception> {
    const NO_KEY: u8 = 255;
    const NO_RESP: u8 = 254;
    let mut ex = Vec::new();
    if cfg.window == 0 {
        return ex;
    }
    // Per form: item per printed position, σ⁻¹ tables, printed single-key letters.
    struct Tab {
        items: Vec<usize>,
        inv: Vec<[u8; 32]>,
        kp: Vec<u8>,
    }
    let tabs: Vec<Tab> = (0..forms.forms.len())
        .map(|f| {
            let form = &forms.forms[f];
            let pk = key.printed_key(forms, f);
            Tab {
                items: form.positions.iter().map(|p| p.item).collect(),
                inv: form
                    .positions
                    .iter()
                    .map(|p| {
                        let mut t = [NO_RESP; 32];
                        for (l, &o) in p.option_map.iter().enumerate() {
                            t[o as usize & 31] = l as u8;
                        }
                        t
                    })
                    .collect(),
                kp: form
                    .positions
                    .iter()
                    .zip(&pk)
                    .map(|(p, e)| {
                        let choice = forms.items.get(p.item).is_some_and(|it| it.kind.is_choice());
                        match e {
                            Some(e) if choice => e.single_key().unwrap_or(NO_KEY),
                            _ => NO_KEY,
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    let mut resp: Vec<u8> = Vec::new();
    for (n, row) in rm.rows.iter().enumerate() {
        if row.absent.is_some() {
            continue;
        }
        let t = &tabs[row.form];
        let len = t.items.len();
        if len < cfg.window {
            continue;
        }
        resp.clear();
        for (j, &i) in t.items.iter().enumerate() {
            let c = rm.cells[i * rm.n_rows + n];
            let m = cell::payload(c);
            resp.push(if cell::state(c) == cell::ANSWERED && m.is_power_of_two() {
                t.inv[j][m.trailing_zeros() as usize & 31]
            } else {
                NO_RESP
            });
        }
        let kp = &t.kp;
        let k_at = |k: usize| kp.get(k).copied().unwrap_or(NO_KEY);
        let e0 = |j: usize| usize::from(resp[j] == kp[j]);
        let ep = |j: usize| usize::from(resp[j] == k_at(j + 1));
        let em = |j: usize| usize::from(j > 0 && resp[j] == kp[j - 1]);
        let (mut h0, mut hp, mut hm) = (0usize, 0usize, 0usize);
        let mut hit: Option<(usize, i32)> = None;
        for j in 0..len {
            h0 += e0(j);
            hp += ep(j);
            hm += em(j);
            if j >= cfg.window {
                let o = j - cfg.window;
                h0 -= e0(o);
                hp -= ep(o);
                hm -= em(o);
            }
            if j + 1 >= cfg.window && h0 <= cfg.max_aligned {
                if hp >= cfg.min_shifted {
                    hit = Some((j + 1 - cfg.window, 1));
                } else if hm >= cfg.min_shifted {
                    hit = Some((j + 1 - cfg.window, -1));
                }
                if hit.is_some() {
                    break;
                }
            }
        }
        if let Some((start, s)) = hit {
            ex.push(
                Exception::must("MRK-012", Severity::Warn, Scope::Booklet)
                    .with_target(Target {
                        booklet: Some(row.booklet.0),
                        question: Some(start as u16),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "answers at positions {}–{} match the key shifted by {s:+}; \
                         instructor check only, never auto-corrected",
                        start + 1,
                        start + cfg.window
                    )),
            );
        }
    }
    ex
}

/// SCO-005 (ERROR, Booklet, sub-reason `near_chance`): the best score under every form's key,
/// over single-answer items, is ≤ E₀ + 2·SD₀ with E₀ = Σ_j M_j·a_j/m_j and
/// SD₀ = √(Σ_j M_j²·(a_j/m_j)·(1 − a_j/m_j)) (§10.3). The threshold is advisory (`f64`).
pub fn near_chance_check(
    rm: &ResponseMatrix,
    l3: &ItemScoreMatrix,
    forms: &FormMap,
    key: &KeyVersion,
) -> Vec<Exception> {
    let mut ex = Vec::new();
    let thresholds: Vec<Option<f64>> = forms
        .forms
        .iter()
        .map(|f| {
            let (mut e0, mut v0, mut k) = (0.0f64, 0.0f64, 0usize);
            for p in &f.positions {
                let it = &forms.items[p.item];
                if !it.kind.is_choice() {
                    continue;
                }
                let Some(e) = key.entry(p.item) else { continue };
                let mi = it.max_points.to_f64().unwrap_or(0.0);
                let q = f64::from(e.full_credit_count()) / f64::from(it.options);
                e0 += mi * q;
                v0 += mi * mi * q * (1.0 - q);
                k += 1;
            }
            (k > 0).then(|| e0 + 2.0 * v0.sqrt())
        })
        .collect();
    // Declared-form score over choice items (positive credit), accumulated column-wise in
    // scaled integers: no per-cell rational arithmetic.
    let mut own = vec![0i64; rm.n_rows];
    for (i, it) in forms.items.iter().enumerate() {
        if !it.kind.is_choice() {
            continue;
        }
        for (o, &v) in own.iter_mut().zip(l3.column(i)) {
            *o += v.max(0);
        }
    }
    let d = l3.denom as f64;
    let alt_score = |f: usize, n: usize, declared: usize| -> f64 {
        // Number-right score of row n's printed answers read as form f.
        let mut s = Points::zero();
        let dform = &forms.forms[declared];
        let form = &forms.forms[f];
        for (j, p) in dform.positions.iter().enumerate() {
            let c = rm.raw(n, p.item);
            let m = cell::payload(c);
            if cell::state(c) != cell::ANSWERED || m.count_ones() != 1 {
                continue;
            }
            let printed = dform.unmap_mask(j, m);
            let Some(fp) = form.positions.get(j) else { continue };
            let it = &forms.items[fp.item];
            if !it.kind.is_choice() {
                continue;
            }
            let master = form.map_mask(j, printed).trailing_zeros() as usize;
            if let Some(crate::key::KeyEntry::Credits { credits, .. }) = key.entry(fp.item)
                && let Some(c) = credits.get(master)
                && *c > Points::zero()
            {
                s += it.max_points * c;
            }
        }
        s.to_f64().unwrap_or(0.0)
    };
    for (n, row) in rm.rows.iter().enumerate() {
        if row.absent.is_some() {
            continue;
        }
        let Some(thr) = thresholds[row.form] else { continue };
        let own_score = own[n] as f64 / d;
        if own_score > thr {
            continue;
        }
        let near = (0..forms.forms.len()).all(|f| match thresholds[f] {
            Some(t) => f == row.form || alt_score(f, n, row.form) <= t,
            None => true,
        });
        if near {
            ex.push(
                Exception::must("SCO-005", Severity::Error, Scope::Booklet)
                    .with_sub_reason("near_chance")
                    .with_target(Target {
                        booklet: Some(row.booklet.0),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "best-form score {own_score:.2} ≤ E₀ + 2·SD₀ = {thr:.2}: wrong exam, \
                         wrong key or random responding"
                    )),
            );
        }
    }
    ex
}
