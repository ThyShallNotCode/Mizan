//! L3: exact item scores s_{n,i} from the master-space response matrix (spec §10.1, §10.4).
//!
//! The key and policy compile into per-item lookup tables of integers scaled to one run-wide
//! common denominator D (the LCM of every possible cell value's denominator). The hot loop
//! is then integer table lookups and popcounts over a contiguous column-major `u32` matrix:
//! TP = popcnt(r ∧ K), FP = popcnt(r ∧ ¬K ∧ O_i). Values stay exact: s = scaled / D.

use crate::error::ScoringError;
use crate::form::FormMap;
use crate::item::{ItemKind, MAX_OPTIONS};
use crate::key::{KeyEntry, KeyVersion, formula_wrong_credit};
use crate::num::{Exact, Points, lcm_i128, widen};
use crate::policy::{MultiOnSba, NegativeMarking, PolicyVersion};
use crate::response::{ResponseMatrix, cell};
use num_rational::Ratio;
use num_traits::{Signed, Zero};
use omr_model::exceptions::Target;
use omr_model::{Exception, Scope, Severity};
use serde::{Deserialize, Serialize};

/// State of one L3 cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ItemState {
    /// Scored from a response (including a confirmed multi or a score-box value).
    Scored = 0,
    /// Blank: 0 for grading.
    Blank = 1,
    /// Not presented on the booklet's form, or the student is absent.
    NotPresented = 2,
    /// Skip-logic condition false.
    NotApplicable = 3,
    /// Unreadable or unresolved: the booklet total is provisional.
    Pending = 4,
    /// Trailing blank run in a speeded section: 0 for grading, missing for statistics.
    NotReached = 5,
}

impl ItemState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => ItemState::Scored,
            1 => ItemState::Blank,
            3 => ItemState::NotApplicable,
            4 => ItemState::Pending,
            5 => ItemState::NotReached,
            _ => ItemState::NotPresented,
        }
    }

    /// True when the item was on the student's form.
    pub fn presented(self) -> bool {
        !matches!(self, ItemState::NotPresented | ItemState::NotApplicable)
    }
}

const STATE_OF_CELL: [u8; 8] = [
    ItemState::Scored as u8,
    ItemState::Blank as u8,
    ItemState::NotPresented as u8,
    ItemState::NotApplicable as u8,
    ItemState::Pending as u8,
    ItemState::Scored as u8,
    ItemState::NotPresented as u8,
    ItemState::NotPresented as u8,
];

#[derive(Clone, Debug)]
enum ChoiceMulti {
    Value(i64),
    MaxOverCount,
}

#[derive(Clone, Debug)]
struct ChoiceLut {
    single: [i64; MAX_OPTIONS as usize],
    raw: [i64; MAX_OPTIONS as usize],
    multi: ChoiceMulti,
}

#[derive(Clone, Debug)]
enum Lut {
    Choice(Box<ChoiceLut>),
    Select {
        key: u32,
        opts: u32,
        stride: usize,
        table: Vec<i64>,
    },
    Open,
}

/// Key + policy compiled for one response matrix: integer lookup tables at denominator D.
#[derive(Clone, Debug)]
pub struct CompiledKey {
    denom: i64,
    luts: Vec<Lut>,
    max_scaled: Vec<i64>,
    points_scaled: Vec<i64>,
}

struct Collect {
    d: i128,
}

impl Collect {
    fn add(&mut self, v: &Exact) -> Result<(), ScoringError> {
        self.d = lcm_i128(self.d, *v.denom())
            .ok_or_else(|| ScoringError::Overflow("common denominator D".into()))?;
        Ok(())
    }
}

fn scale(v: &Exact, d: i128) -> Result<i64, ScoringError> {
    let den = *v.denom();
    if d % den != 0 {
        return Err(ScoringError::Overflow(format!(
            "{v} not representable at D = {d}"
        )));
    }
    v.numer()
        .checked_mul(d / den)
        .and_then(|x| i64::try_from(x).ok())
        .ok_or_else(|| ScoringError::Overflow(format!("{v} × D = {d} exceeds i64")))
}

impl CompiledKey {
    /// Compiles `key` and `policy` for the items of `forms` and the score-box side table.
    pub fn compile(
        forms: &FormMap,
        key: &KeyVersion,
        policy: &PolicyVersion,
        points: &[Points],
    ) -> Result<Self, ScoringError> {
        enum Exacts {
            Choice {
                single: Vec<Exact>,
                raw: Vec<Exact>,
                multi: Option<Exact>,
            },
            Select {
                key: u32,
                opts: u32,
                stride: usize,
                table: Vec<Exact>,
            },
            Open,
        }
        let mut col = Collect { d: 1 };
        let mut exacts = Vec::with_capacity(forms.items.len());
        for (i, it) in forms.items.iter().enumerate() {
            let m_i = widen(&it.max_points);
            col.add(&m_i)?;
            let entry = key.entry(i).ok_or_else(|| {
                ScoringError::InvalidReference(format!("item {} has no key entry", it.label))
            })?;
            let e = match (it.kind, entry) {
                (
                    ItemKind::SingleBestAnswer | ItemKind::TrueFalse,
                    KeyEntry::Credits { credits, .. },
                ) => {
                    let a = entry.full_credit_count();
                    let m = u32::from(it.options);
                    let wrong = match &policy.negative {
                        NegativeMarking::Off => Points::zero(),
                        NegativeMarking::Formula => formula_wrong_credit(a, m),
                        NegativeMarking::Penalty { .. } => {
                            -policy.penalty_for(i).unwrap_or_default().abs()
                        }
                    };
                    let single: Vec<Exact> = credits
                        .iter()
                        .map(|c| {
                            let c = if c.is_zero() { wrong } else { *c };
                            m_i * widen(&c)
                        })
                        .collect();
                    let raw: Vec<Exact> = credits.iter().map(|c| m_i * widen(c)).collect();
                    for v in single.iter().chain(&raw) {
                        col.add(v)?;
                    }
                    let multi = match policy.multi_on_sba {
                        MultiOnSba::InvalidZero => Some(Exact::zero()),
                        MultiOnSba::Wrong => Some(m_i * widen(&wrong)),
                        MultiOnSba::PartialMaxOverCount => {
                            for v in &raw {
                                for k in 2..=i128::from(it.options) {
                                    col.add(&(v / Ratio::from_integer(k)))?;
                                }
                            }
                            None
                        }
                    };
                    if let Some(v) = &multi {
                        col.add(v)?;
                    }
                    Exacts::Choice { single, raw, multi }
                }
                (ItemKind::MultiSelect(mode), KeyEntry::Set(k)) => {
                    let rule = policy.rule_for(i, mode);
                    let m = u32::from(it.options);
                    let t = k.count_ones();
                    let stride = (m - t + 1) as usize;
                    let mut table = Vec::with_capacity((t as usize + 1) * stride);
                    for tp in 0..=t {
                        for fp in 0..=(m - t) {
                            let v = m_i * widen(&rule.credit(tp, fp, t, m));
                            col.add(&v)?;
                            table.push(v);
                        }
                    }
                    Exacts::Select {
                        key: *k,
                        opts: it.option_mask(),
                        stride,
                        table,
                    }
                }
                (ItemKind::ScoreBox, KeyEntry::Open) => Exacts::Open,
                _ => {
                    return Err(ScoringError::InvalidReference(format!(
                        "item {}: key entry does not match item type",
                        it.label
                    )));
                }
            };
            exacts.push(e);
        }
        for p in points {
            col.add(&widen(p))?;
        }
        let d = col.d;
        if d > i128::from(i64::MAX) {
            return Err(ScoringError::Overflow(format!(
                "common denominator D = {d}"
            )));
        }
        let mut luts = Vec::with_capacity(exacts.len());
        let mut max_scaled = Vec::with_capacity(exacts.len());
        let mut row_bound: i128 = 0;
        let max_points_scaled = points
            .iter()
            .map(|p| scale(&widen(p), d))
            .collect::<Result<Vec<_>, _>>()?;
        for (it, e) in forms.items.iter().zip(exacts) {
            let ms = scale(&widen(&it.max_points), d)?;
            max_scaled.push(ms);
            let mut worst = i128::from(ms).abs();
            let lut = match e {
                Exacts::Choice { single, raw, multi } => {
                    let mut s = [0i64; MAX_OPTIONS as usize];
                    let mut r = [0i64; MAX_OPTIONS as usize];
                    for (o, v) in single.iter().enumerate() {
                        s[o] = scale(v, d)?;
                        worst = worst.max(i128::from(s[o]).abs());
                    }
                    for (o, v) in raw.iter().enumerate() {
                        r[o] = scale(v, d)?;
                        worst = worst.max(i128::from(r[o]).abs());
                    }
                    let multi = match multi {
                        Some(v) => {
                            let x = scale(&v, d)?;
                            worst = worst.max(i128::from(x).abs());
                            ChoiceMulti::Value(x)
                        }
                        None => ChoiceMulti::MaxOverCount,
                    };
                    Lut::Choice(Box::new(ChoiceLut {
                        single: s,
                        raw: r,
                        multi,
                    }))
                }
                Exacts::Select {
                    key,
                    opts,
                    stride,
                    table,
                } => {
                    let table = table
                        .iter()
                        .map(|v| scale(v, d))
                        .collect::<Result<Vec<_>, _>>()?;
                    for v in &table {
                        worst = worst.max(i128::from(*v).abs());
                    }
                    Lut::Select {
                        key,
                        opts,
                        stride,
                        table,
                    }
                }
                Exacts::Open => Lut::Open,
            };
            row_bound += worst;
            luts.push(lut);
        }
        if row_bound > i128::from(i64::MAX / 4) {
            return Err(ScoringError::Overflow(format!(
                "per-booklet sum bound {row_bound} at D = {d}"
            )));
        }
        Ok(Self {
            denom: d as i64,
            luts,
            max_scaled,
            points_scaled: max_points_scaled,
        })
    }

    /// The common denominator D.
    pub fn denom(&self) -> i64 {
        self.denom
    }

    /// M_i·D.
    pub fn max_scaled(&self, i: usize) -> i64 {
        self.max_scaled[i]
    }
}

/// L3 item scores: N × I exact scores as integers over D, plus item states, column-major.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemScoreMatrix {
    /// Booklets (rows).
    pub n_rows: usize,
    /// Master items (columns).
    pub n_items: usize,
    /// Common denominator D.
    pub denom: i64,
    scaled: Vec<i64>,
    states: Vec<u8>,
}

impl ItemScoreMatrix {
    /// s_{n,i}·D.
    #[inline]
    pub fn scaled(&self, n: usize, i: usize) -> i64 {
        self.scaled[i * self.n_rows + n]
    }

    /// Exact s_{n,i}.
    pub fn score(&self, n: usize, i: usize) -> Points {
        Ratio::new(self.scaled(n, i), self.denom)
    }

    /// State of (n, i).
    #[inline]
    pub fn state(&self, n: usize, i: usize) -> ItemState {
        ItemState::from_u8(self.states[i * self.n_rows + n])
    }

    /// Scaled column of item `i`.
    pub fn column(&self, i: usize) -> &[i64] {
        &self.scaled[i * self.n_rows..(i + 1) * self.n_rows]
    }

    fn set_state(&mut self, n: usize, i: usize, s: ItemState) {
        self.states[i * self.n_rows + n] = s as u8;
    }
}

/// The L3 hot loop: every cell scored by table lookup (§10.1 "Hot loop"). The item kind is
/// dispatched once per column, so the inner loop is a lookup or two popcounts per cell.
pub fn score_matrix(rm: &ResponseMatrix, ck: &CompiledKey) -> ItemScoreMatrix {
    let n = rm.n_rows;
    let mut scaled = vec![0i64; n * rm.n_items];
    let mut states = vec![0u8; n * rm.n_items];
    for (i, lut) in ck.luts.iter().enumerate() {
        let col = rm.column(i);
        let out = &mut scaled[i * n..(i + 1) * n];
        let st = &mut states[i * n..(i + 1) * n];
        for (c, s) in col.iter().zip(st.iter_mut()) {
            *s = STATE_OF_CELL[(cell::state(*c) & 7) as usize];
        }
        match lut {
            Lut::Choice(cl) => {
                let ChoiceLut { single, raw, multi } = &**cl;
                for (c, o) in col.iter().zip(out.iter_mut()) {
                    if cell::state(*c) != cell::ANSWERED {
                        continue;
                    }
                    let mask = cell::payload(*c);
                    *o = if mask & mask.wrapping_sub(1) == 0 {
                        single[mask.trailing_zeros() as usize & 31]
                    } else {
                        match multi {
                            ChoiceMulti::Value(v) => *v,
                            ChoiceMulti::MaxOverCount => {
                                let mut best = i64::MIN;
                                let mut m = mask;
                                while m != 0 {
                                    best = best.max(raw[m.trailing_zeros() as usize & 31]);
                                    m &= m - 1;
                                }
                                best / i64::from(mask.count_ones())
                            }
                        }
                    };
                }
            }
            Lut::Select {
                key,
                opts,
                stride,
                table,
            } => {
                for (c, o) in col.iter().zip(out.iter_mut()) {
                    if cell::state(*c) != cell::ANSWERED {
                        continue;
                    }
                    let mask = cell::payload(*c);
                    let tp = (mask & key).count_ones() as usize;
                    let fp = (mask & !key & opts).count_ones() as usize;
                    *o = table[tp * stride + fp];
                }
            }
            Lut::Open => {
                for (c, o) in col.iter().zip(out.iter_mut()) {
                    if cell::state(*c) == cell::POINTS {
                        *o = ck.points_scaled[cell::payload(*c) as usize];
                    }
                }
            }
        }
    }
    ItemScoreMatrix {
        n_rows: n,
        n_items: rm.n_items,
        denom: ck.denom,
        scaled,
        states,
    }
}

/// Marks the trailing blank run of each speeded section, in the student's printed order, as
/// `NotReached` (0 for grading, missing for statistics) and raises SCO-024 (INFO) (§10.5).
pub fn mark_not_reached(
    l3: &mut ItemScoreMatrix,
    rm: &ResponseMatrix,
    forms: &FormMap,
    policy: &PolicyVersion,
) -> Vec<Exception> {
    let mut ex = Vec::new();
    let speeded: Vec<_> = policy.sections.iter().filter(|s| s.speeded).collect();
    if speeded.is_empty() {
        return ex;
    }
    // Per form and speeded section: the section's items in printed order.
    let orders: Vec<Vec<Vec<usize>>> = forms
        .forms
        .iter()
        .map(|f| {
            let pos = f.positions_of_items(forms.items.len());
            speeded
                .iter()
                .map(|s| {
                    let mut v: Vec<(usize, usize)> = s
                        .items
                        .iter()
                        .filter_map(|&i| pos.get(i).copied().flatten().map(|j| (j, i)))
                        .collect();
                    v.sort_unstable();
                    v.into_iter().map(|(_, i)| i).collect()
                })
                .collect()
        })
        .collect();
    for (n, row) in rm.rows.iter().enumerate() {
        if row.absent.is_some() {
            continue;
        }
        let mut count = 0usize;
        for order in &orders[row.form] {
            for &i in order.iter().rev() {
                if l3.state(n, i) == ItemState::Blank {
                    l3.set_state(n, i, ItemState::NotReached);
                    count += 1;
                } else {
                    break;
                }
            }
        }
        if count > 0 {
            ex.push(
                Exception::must("SCO-024", Severity::Info, Scope::Booklet)
                    .with_target(Target {
                        booklet: Some(row.booklet.0),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "{count} trailing unanswered item(s) in a speeded section: 0 for \
                         grading, missing for item statistics"
                    )),
            );
        }
    }
    ex
}

/// Exact `scaled / D` helper for aggregates.
pub fn unscale(v: i128, d: i64) -> Exact {
    Ratio::new(v, i128::from(d))
}
