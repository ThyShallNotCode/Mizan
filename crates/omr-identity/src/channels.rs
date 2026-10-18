//! Channel likelihoods (§9.5): the bubbled ID grid (channel A), a typed or OCR-read ID string
//! (channel B, the IAU sheet's handwritten "Student ID" line), and the writer-error
//! neighbourhood that links them through the latent intended ID.
//!
//! All likelihoods are in natural-log space. Column likelihoods are divided by the
//! candidate-independent factor `Π_j (1 − p_j)`, so a column's "intended blank" likelihood is 1.

use omr_model::{ColumnState, IdGridRead};
use std::collections::BTreeSet;

/// Numerically safe `ln Σ exp(xᵢ)`.
pub(crate) fn logsumexp(xs: impl IntoIterator<Item = f64>) -> f64 {
    let v: Vec<f64> = xs.into_iter().collect();
    let m = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if m == f64::NEG_INFINITY {
        return m;
    }
    m + v.iter().map(|x| (x - m).exp()).sum::<f64>().ln()
}

/// Channel A: per-column log-likelihoods `log L_c(k)` (§9.5 formula, normalised).
#[derive(Clone, Debug)]
pub(crate) struct GridModel {
    /// `ll[c][k]` = log L'_c(k); unreadable columns are all 0.
    ll: Vec<[f64; 10]>,
    /// Column states as read.
    pub(crate) states: Vec<ColumnState>,
}

impl GridModel {
    /// Builds the model. `beta` = blank probability, `mu` = double-mark probability,
    /// `default_logit` = log-odds used for columns that carry a state but no logits.
    pub(crate) fn new(read: &IdGridRead, beta: f64, mu: f64, default_logit: f64) -> Self {
        let mut cols = read.columns.clone();
        cols.sort_by_key(|c| c.column);
        let mut ll = Vec::with_capacity(cols.len());
        for c in &cols {
            if c.state == ColumnState::Unreadable {
                ll.push([0.0; 10]);
                continue;
            }
            let logits: [f64; 10] = if c.logits.len() == 10 {
                std::array::from_fn(|k| c.logits[k].nats())
            } else {
                std::array::from_fn(|k| {
                    if c.marked.contains(&(k as u8)) {
                        default_logit
                    } else {
                        -default_logit
                    }
                })
            };
            // Odds r_k = p_k / (1 − p_k) = exp(logit_k), clamped.
            let r: [f64; 10] = std::array::from_fn(|k| logits[k].clamp(-30.0, 30.0).exp());
            let total: f64 = r.iter().sum();
            let col: [f64; 10] = std::array::from_fn(|k| {
                let v = (1.0 - beta - mu) * r[k] + beta + mu * r[k] * (total - r[k]) / 9.0;
                v.ln()
            });
            ll.push(col);
        }
        Self {
            ll,
            states: cols.iter().map(|c| c.state).collect(),
        }
    }

    /// Number of columns.
    pub(crate) fn n(&self) -> usize {
        self.ll.len()
    }

    /// True when no column carries evidence (all unreadable).
    pub(crate) fn uninformative(&self) -> bool {
        self.states.iter().all(|s| *s == ColumnState::Unreadable)
    }

    /// `log ℓ_A(u)` for a digit string of length n, n−1 or n−2 (blank columns placed anywhere,
    /// uniform prior over placements, §9.5); −∞ for other lengths.
    pub(crate) fn log_l(&self, u: &[u8]) -> f64 {
        let n = self.n();
        if u.len() == n {
            return u
                .iter()
                .enumerate()
                .map(|(c, &d)| self.ll[c][usize::from(d)])
                .sum();
        }
        if u.len() + 1 == n {
            let terms = (0..n).map(|p| {
                (0..n)
                    .filter(|&c| c != p)
                    .map(|c| {
                        let i = if c < p { c } else { c - 1 };
                        self.ll[c][usize::from(u[i])]
                    })
                    .sum::<f64>()
            });
            return logsumexp(terms) - (n as f64).ln();
        }
        if u.len() + 2 == n && n >= 2 {
            let mut terms = Vec::new();
            for p in 0..n {
                for q in p + 1..n {
                    let mut i = 0;
                    let mut s = 0.0;
                    for c in 0..n {
                        if c == p || c == q {
                            continue;
                        }
                        s += self.ll[c][usize::from(u[i])];
                        i += 1;
                    }
                    terms.push(s);
                }
            }
            let k = terms.len() as f64;
            return logsumexp(terms) - k.ln();
        }
        f64::NEG_INFINITY
    }

    /// The most likely digit per column (ties → smaller digit).
    pub(crate) fn top(&self) -> Vec<u8> {
        self.ll
            .iter()
            .map(|col| {
                let mut best = 0;
                for k in 1..10 {
                    if col[k] > col[best] {
                        best = k;
                    }
                }
                best as u8
            })
            .collect()
    }

    /// Per-column log score of digit `d` (for combining with channel B positionally).
    pub(crate) fn col(&self, c: usize, d: u8) -> f64 {
        self.ll[c][usize::from(d)]
    }

    /// `log ℓ̃_A(u)`: mean of ℓ_A over the strings differing from `u` in exactly one column
    /// (another digit or a blank). Equal to `log ℓ_A(u)` for strings not of length n.
    pub(crate) fn log_tilde(&self, u: &[u8]) -> f64 {
        let n = self.n();
        let base = self.log_l(u);
        if u.len() != n || n == 0 {
            return base;
        }
        let terms = (0..n).map(|c| {
            let own = self.ll[c][usize::from(u[c])];
            let others = logsumexp(
                (0..10)
                    .filter(|&k| k != usize::from(u[c]))
                    .map(|k| self.ll[c][k])
                    .chain(std::iter::once(0.0)),
            );
            others - own
        });
        base + logsumexp(terms) - ((10 * n) as f64).ln()
    }
}

/// Channel A pieces of one candidate ID, all exact: `log ℓ_A(x)`, `log ℓ̃_A(x)` and
/// `log Σ_{u ∈ N(x)} ℓ_A(u)` over the writer-error neighbourhood, with `|N(x)|`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AParts {
    pub(crate) direct: f64,
    pub(crate) tilde: f64,
    pub(crate) nb_lse: f64,
    pub(crate) n_nb: usize,
}

impl GridModel {
    /// [`AParts`] for `x`. IDs as long as the grid use an allocation-light O(n²) path (the
    /// neighbourhood is enumerated implicitly); other lengths enumerate it explicitly.
    pub(crate) fn parts(&self, x: &[u8], with_tilde: bool) -> AParts {
        let n = self.n();
        if x.len() != n || n == 0 {
            let nb = writer_neighbours(x);
            return AParts {
                direct: self.log_l(x),
                tilde: if with_tilde {
                    self.log_tilde(x)
                } else {
                    f64::NAN
                },
                nb_lse: logsumexp(nb.iter().map(|u| self.log_l(u))),
                n_nb: nb.len(),
            };
        }
        let own: Vec<f64> = (0..n).map(|c| self.ll[c][usize::from(x[c])]).collect();
        let total: f64 = own.iter().sum();
        let mut terms: Vec<f64> = Vec::with_capacity(12 * n);
        let mut tilde_terms: Vec<f64> = Vec::with_capacity(n);
        for c in 0..n {
            for d in 0..10 {
                if d != usize::from(x[c]) {
                    terms.push(total - own[c] + self.ll[c][d]);
                }
            }
            if with_tilde {
                let alts = (0..10)
                    .filter(|&d| d != usize::from(x[c]))
                    .map(|d| self.ll[c][d])
                    .chain(std::iter::once(0.0)); // blank
                tilde_terms.push(logsumexp(alts) - own[c]);
            }
        }
        for c in 0..n.saturating_sub(1) {
            if x[c] != x[c + 1] {
                terms.push(
                    total - own[c] - own[c + 1]
                        + self.ll[c][usize::from(x[c + 1])]
                        + self.ll[c + 1][usize::from(x[c])],
                );
            }
        }
        // Deletions: u = x without x_i, placed with a blank at column p. Column c shows x_c
        // (aligned) before min(p, i) and after max(p, i); x_{c+1} for i ≤ c < p; x_{c−1} for
        // p < c ≤ i. Prefix sums make each (i, p) term O(1).
        let mut s0 = vec![0.0f64; n + 1]; // aligned
        let mut s1 = vec![0.0f64; n + 1]; // column c shows x_{c+1}
        let mut sm = vec![0.0f64; n + 1]; // column c shows x_{c−1}
        for c in 0..n {
            s0[c + 1] = s0[c] + own[c];
            s1[c + 1] = s1[c]
                + if c + 1 < n {
                    self.ll[c][usize::from(x[c + 1])]
                } else {
                    0.0
                };
            sm[c + 1] = sm[c]
                + if c > 0 {
                    self.ll[c][usize::from(x[c - 1])]
                } else {
                    0.0
                };
        }
        let mut i = 0;
        let mut placements = Vec::with_capacity(n);
        while i < n {
            placements.clear();
            for p in 0..n {
                let (lo, hi) = (p.min(i), p.max(i));
                let mut s = s0[lo] + (s0[n] - s0[hi + 1]);
                if i < p {
                    s += s1[p] - s1[i];
                } else if p < i {
                    s += sm[i + 1] - sm[p + 1];
                }
                placements.push(s);
            }
            terms.push(logsumexp(placements.iter().copied()) - (n as f64).ln());
            let v = x[i];
            while i < n && x[i] == v {
                i += 1;
            }
        }
        AParts {
            direct: total,
            tilde: if with_tilde {
                total + logsumexp(tilde_terms) - ((10 * n) as f64).ln()
            } else {
                f64::NAN
            },
            nb_lse: logsumexp(terms.iter().copied()),
            n_nb: terms.len(),
        }
    }
}

/// `|N(x)|` without enumerating it.
pub(crate) fn neighbour_count(x: &[u8]) -> usize {
    if x.is_empty() {
        return 0;
    }
    let transpositions = x.windows(2).filter(|w| w[0] != w[1]).count();
    let runs = 1 + transpositions;
    transpositions + 9 * x.len() + runs
}

/// Channel B: a typed or OCR-read ID string scored with a character edit model.
#[derive(Clone, Debug)]
pub(crate) struct TypedModel {
    /// Characters: `Some(digit)` or `None` (unreadable mark such as `?`).
    pub(crate) chars: Vec<Option<u8>>,
    p_match: f64,
    p_sub: f64,
    p_del: f64,
    p_ins: f64,
    p_q: f64,
}

impl TypedModel {
    /// `char_error` is the per-character substitution rate (typed ≈ 0.002, OCR ≈ 0.02).
    pub(crate) fn new(chars: Vec<Option<u8>>, char_error: f64) -> Self {
        let e = char_error.clamp(1e-6, 0.3);
        let (p_sub, p_del, p_ins, p_q) = (e, e / 4.0, e / 4.0, e);
        Self {
            chars,
            p_match: 1.0 - p_sub - p_del - p_q,
            p_sub,
            p_del,
            p_ins,
            p_q,
        }
    }

    /// The digit string when every character is a digit.
    pub(crate) fn complete(&self) -> Option<Vec<u8>> {
        self.chars.iter().copied().collect()
    }

    /// True when no character is readable.
    pub(crate) fn uninformative(&self) -> bool {
        self.chars.iter().all(Option::is_none)
    }

    fn emit(&self, d: u8, c: Option<u8>) -> f64 {
        match c {
            Some(c) if c == d => self.p_match,
            Some(_) => self.p_sub / 9.0,
            None => self.p_q,
        }
    }

    /// log of the emission probability of character `c` for intended digit `d`.
    pub(crate) fn emit_ln(&self, d: u8, c: Option<u8>) -> f64 {
        self.emit(d, c).ln()
    }

    /// `log P(b | u)` by the forward recursion over alignments (substitution, deletion of an
    /// intended digit, insertion of an extra character).
    pub(crate) fn log_l(&self, u: &[u8]) -> f64 {
        let m = self.chars.len();
        let mut prev = vec![0.0f64; m + 1];
        prev[0] = 1.0;
        for j in 1..=m {
            prev[j] = prev[j - 1] * self.p_ins / 10.0;
        }
        let mut cur = vec![0.0f64; m + 1];
        for &d in u {
            cur[0] = prev[0] * self.p_del;
            for j in 1..=m {
                cur[j] = prev[j - 1] * self.emit(d, self.chars[j - 1])
                    + prev[j] * self.p_del
                    + cur[j - 1] * self.p_ins / 10.0;
            }
            std::mem::swap(&mut prev, &mut cur);
        }
        prev[m].max(f64::MIN_POSITIVE).ln()
    }

    /// `log ℓ̃_B(u)`: mean over one-position substitutions and deletions of `u`.
    pub(crate) fn log_tilde(&self, u: &[u8]) -> f64 {
        if u.is_empty() {
            return self.log_l(u);
        }
        let mut terms = Vec::with_capacity(10 * u.len());
        let mut v = u.to_vec();
        for i in 0..u.len() {
            for d in 0..10u8 {
                if d == u[i] {
                    continue;
                }
                v[i] = d;
                terms.push(self.log_l(&v));
            }
            v[i] = u[i];
            let mut del = u.to_vec();
            del.remove(i);
            terms.push(self.log_l(&del));
        }
        let k = terms.len() as f64;
        logsumexp(terms) - k.ln()
    }

    /// `log ρ` bound for strings different from the best reading (complete strings only):
    /// any single edit costs at least this log-ratio.
    pub(crate) fn log_rho1(&self) -> f64 {
        if self.complete().is_none() {
            return 0.0;
        }
        let worst_ok = (self.p_sub / 9.0).max(self.p_del).max(self.p_ins / 10.0);
        (worst_ok / self.p_match).ln().min(0.0)
    }
}

/// Kinds of writer error linking an intended ID to what was written (§9.5, §9.9 rows 4–5).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum WriterErrorKind {
    /// Two adjacent digits swapped.
    Transposition,
    /// One digit replaced.
    Substitution,
    /// One digit left out.
    Deletion,
}

/// The writer-error neighbourhood `N(x)`: adjacent transpositions, single substitutions and
/// single deletions (deduplicated, `x` excluded), in a deterministic order.
pub(crate) fn writer_neighbours(x: &[u8]) -> Vec<Vec<u8>> {
    let mut set: BTreeSet<Vec<u8>> = BTreeSet::new();
    for i in 0..x.len() {
        if i + 1 < x.len() && x[i] != x[i + 1] {
            let mut v = x.to_vec();
            v.swap(i, i + 1);
            set.insert(v);
        }
        for d in 0..10u8 {
            if d != x[i] {
                let mut v = x.to_vec();
                v[i] = d;
                set.insert(v);
            }
        }
        let mut v = x.to_vec();
        v.remove(i);
        set.insert(v);
    }
    set.remove(x);
    set.into_iter().collect()
}

/// `N(x)` plus single insertions (for blocking lookups: an ID one digit longer than the read).
pub(crate) fn lookup_neighbours(x: &[u8]) -> Vec<Vec<u8>> {
    let mut v = writer_neighbours(x);
    for i in 0..=x.len() {
        for d in 0..10u8 {
            let mut s = x.to_vec();
            s.insert(i, d);
            v.push(s);
        }
    }
    v.sort();
    v.dedup();
    v
}

/// How `id` relates to the read `y` by one writer error, if it does.
pub(crate) fn writer_relation(id: &[u8], y: &[u8]) -> Option<WriterErrorKind> {
    if id.len() == y.len() {
        let diff: Vec<usize> = (0..id.len()).filter(|&i| id[i] != y[i]).collect();
        match diff.as_slice() {
            [_] => Some(WriterErrorKind::Substitution),
            [a, b] if *b == a + 1 && id[*a] == y[*b] && id[*b] == y[*a] => {
                Some(WriterErrorKind::Transposition)
            }
            _ => None,
        }
    } else if id.len() == y.len() + 1 {
        (0..id.len())
            .any(|i| {
                let mut v = id.to_vec();
                v.remove(i);
                v == y
            })
            .then_some(WriterErrorKind::Deletion)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omr_model::{IdColumnRead, LogitMilli};

    fn grid(s: &str) -> IdGridRead {
        IdGridRead {
            columns: s
                .chars()
                .enumerate()
                .map(|(i, ch)| {
                    let (state, marked) = match ch {
                        '_' => (ColumnState::Blank, vec![]),
                        '?' => (ColumnState::Unreadable, vec![]),
                        d => (ColumnState::Single, vec![d.to_digit(10).unwrap() as u8]),
                    };
                    IdColumnRead {
                        column: i as u8,
                        state,
                        marked,
                        logits: vec![],
                    }
                })
                .collect(),
        }
    }

    fn digits(s: &str) -> Vec<u8> {
        s.bytes().map(|b| b - b'0').collect()
    }

    #[test]
    fn grid_likelihood_prefers_the_read_and_treats_blank_as_wildcard() {
        let g = GridModel::new(&grid("2220001234"), 0.02, 0.01, 6.9);
        let exact = g.log_l(&digits("2220001234"));
        let one_off = g.log_l(&digits("2220001235"));
        assert!(exact - one_off > 6.0, "{exact} vs {one_off}");
        assert_eq!(g.top(), digits("2220001234"));
        let gb = GridModel::new(&grid("22200012_4"), 0.02, 0.01, 6.9);
        let a = gb.log_l(&digits("2220001234"));
        let b = gb.log_l(&digits("2220001284"));
        assert!((a - b).abs() < 1e-9, "a blank column is a wildcard");
        // Unreadable columns contribute nothing.
        let gu = GridModel::new(&grid("222000123?"), 0.02, 0.01, 6.9);
        assert!((gu.log_l(&digits("2220001230")) - gu.log_l(&digits("2220001239"))).abs() < 1e-12);
        // Logits, when present, are used.
        let mut r = grid("1");
        r.columns[0].logits = (0..10)
            .map(|k| LogitMilli(if k == 1 { 2000 } else { -2000 }))
            .collect();
        let gl = GridModel::new(&r, 0.02, 0.01, 6.9);
        assert!(
            gl.log_l(&[1]) - gl.log_l(&[2]) < 5.0,
            "softer evidence from ±2 nat logits"
        );
    }

    #[test]
    fn shorter_ids_sum_over_blank_placements() {
        let g = GridModel::new(&grid("43300000_"), 0.02, 0.01, 6.9);
        let nine = g.log_l(&digits("43300000"));
        assert!(nine.is_finite());
        assert!(nine > g.log_l(&digits("43300001")) + 5.0);
        assert_eq!(g.log_l(&digits("1")), f64::NEG_INFINITY);
    }

    #[test]
    fn typed_channel_edit_model() {
        let b = TypedModel::new(digits("2220001234").into_iter().map(Some).collect(), 0.02);
        let exact = b.log_l(&digits("2220001234"));
        let sub = b.log_l(&digits("2220001235"));
        let del = b.log_l(&digits("222001234"));
        assert!(exact > sub + 5.0 && exact > del + 3.0);
        let q = TypedModel::new(vec![Some(2), None, Some(2)], 0.02);
        assert!(
            (q.log_l(&[2, 0, 2]) - q.log_l(&[2, 9, 2])).abs() < 1e-12,
            "? is a wildcard"
        );
        assert!(b.log_tilde(&digits("2220001234")) < exact);
        assert!(b.log_rho1() < -4.0);
    }

    #[test]
    fn neighbourhood_and_relations() {
        let x = digits("1234");
        let n = writer_neighbours(&x);
        // 3 transpositions + 36 substitutions + 4 deletions.
        assert_eq!(n.len(), 3 + 36 + 4);
        assert!(!n.contains(&x));
        let rep = writer_neighbours(&digits("1123"));
        assert_eq!(
            rep.iter().filter(|v| v.len() == 3).count(),
            3,
            "duplicate deletions merged"
        );
        assert_eq!(
            writer_relation(&digits("1243"), &digits("1234")),
            Some(WriterErrorKind::Transposition)
        );
        assert_eq!(
            writer_relation(&digits("1235"), &digits("1234")),
            Some(WriterErrorKind::Substitution)
        );
        assert_eq!(
            writer_relation(&digits("12345"), &digits("1235")),
            Some(WriterErrorKind::Deletion)
        );
        assert_eq!(writer_relation(&digits("9999"), &digits("1234")), None);
        assert!(lookup_neighbours(&x).contains(&digits("12034")));
    }

    #[test]
    fn fast_parts_match_explicit_enumeration() {
        for (p, x) in [
            ("2220_01234", "2220001234"),
            ("2220001234", "2220011234"),
            ("22?000123_", "1122334455"),
            ("1111111111", "1111111111"),
        ] {
            let g = GridModel::new(&grid(p), 0.02, 0.01, 6.9);
            let x = digits(x);
            let fast = g.parts(&x, true);
            let nb = writer_neighbours(&x);
            assert_eq!(fast.n_nb, nb.len(), "{p}");
            assert_eq!(neighbour_count(&x), nb.len());
            let slow = logsumexp(nb.iter().map(|u| g.log_l(u)));
            assert!(
                (fast.nb_lse - slow).abs() < 1e-9,
                "{p}: {} vs {slow}",
                fast.nb_lse
            );
            assert!((fast.direct - g.log_l(&x)).abs() < 1e-9);
            assert!((fast.tilde - g.log_tilde(&x)).abs() < 1e-9);
        }
    }
}
