//! Points per question from published totals (spec §3.5 item 3, PAR-09).
//!
//! An incumbent's graded export (Remark Office OMR's workbooks) carries each student's answers
//! and total, and sometimes the points per learning-objective group, but not the points per
//! question. Those points are the exact solution of the linear system
//!
//! Σ_q x_{r,q} · p_q = t_r   for every equation r,
//!
//! where x_{r,q} = 1 when equation r counts question q (the student earned it, or the question
//! belongs to the group) and t_r is the published total. [`solve_points`] eliminates the
//! system modulo a 61-bit prime (exact arithmetic in a finite field: no rounding, and no growth
//! of the numbers, which overflow any fixed-width fraction on a 100-question exam), rebuilds
//! each question's points as the fraction they reduce from, and then checks that solution
//! against **every** equation in exact rationals ([`Exact`], checked). A unique solution mod p
//! means a unique solution over the rationals, and the check proves it reproduces every total,
//! so an answer is never approximate. It never fits: a system with no exact solution is
//! [`PointsError::Inconsistent`], and points the equations cannot tell apart are
//! [`PointsError::Underdetermined`], naming the questions, so that the caller asks a person for
//! them instead of guessing. A refusal is confirmed modulo a second prime (a prime that divides
//! a minor of the system could only make it look less determined, never wrong).

use crate::num::Exact;
use num_traits::{CheckedAdd, CheckedSub, Zero};
use thiserror::Error;

/// One equation: the questions it counts (0-based) and its exact total.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Equation {
    /// Questions counted (0-based, any order; duplicates count once).
    pub questions: Vec<usize>,
    /// The published total.
    pub total: Exact,
}

/// Why the points could not be derived.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PointsError {
    /// An equation names a question outside `0..n`, or a known value list has the wrong
    /// length.
    #[error("points system: {0}")]
    Shape(String),
    /// No fixed points per question reproduce every total exactly (bonus or partial credit,
    /// a total edited by hand, or answers that are not the ones graded). `equations` lists
    /// (0-based) the equations a solution of the others contradicts, when one was found.
    #[error("no points per question reproduce every total exactly")]
    Inconsistent {
        /// Equations not reproduced by the points that fit the others (may be empty when the
        /// remaining system is itself underdetermined).
        equations: Vec<usize>,
    },
    /// The equations do not determine these questions' points (0-based): e.g. questions
    /// earned by exactly the same students, or one no student earned.
    #[error("the totals do not determine the points of {} question(s)", .questions.len())]
    Underdetermined {
        /// Questions whose points are not determined.
        questions: Vec<usize>,
        /// The points the equations do determine (`None` for `questions`).
        determined: Vec<Option<Exact>>,
    },
    /// Exact arithmetic would overflow `i128` (an extremely large system).
    #[error("exact arithmetic overflow while solving for the points")]
    Overflow,
}

fn ck<T>(v: Option<T>) -> Result<T, PointsError> {
    v.ok_or(PointsError::Overflow)
}

/// Work budget (multiply-reduce steps) of the search for the one equation that contradicts the
/// others among the pivot equations (one elimination per pivot equation).
const LEAVE_ONE_OUT_WORK: u128 = 2_000_000_000;

/// The primes the system is eliminated modulo, as Mersenne exponents: 2⁶¹ − 1, then 2³¹ − 1 to
/// confirm a refusal. Their fractions are rebuilt up to |numerator|, denominator ≤ ⌊√(p/2)⌋
/// (about 10⁹ and 32,767): far beyond any points per question.
const MERSENNE: [u32; 2] = [61, 31];

/// The points of `n` questions from `equations`, with the questions whose points are
/// already known (`known[q] = Some(p)`, e.g. given by the instructor) fixed. Every equation is
/// verified against the result; see the module documentation for the refusals. When no
/// solution exists and leaving out one equation gives one, that equation is the one named.
pub fn solve_points(
    n: usize,
    equations: &[Equation],
    known: &[Option<Exact>],
) -> Result<Vec<Exact>, PointsError> {
    let e = solve_core(n, equations, known)?;
    match e.result {
        // A solution of the pivot equations reproduces every total but one: that one.
        Err(PointsError::Inconsistent { equations: named }) if named.len() != 1 => {
            // Otherwise the contradicting equation, if it is one, is a pivot equation.
            let rows = equations.len() as u128;
            let m = e.unknowns.max(1) as u128;
            if e.pivot_rows.len() as u128 * rows * m * m <= LEAVE_ONE_OUT_WORK {
                for &r in &e.pivot_rows {
                    let mut rest = equations.to_vec();
                    rest.remove(r);
                    if let Ok(Ok(p)) = solve_core(n, &rest, known).map(|x| x.result) {
                        return Err(PointsError::Inconsistent {
                            equations: unreproduced(equations, &p)?,
                        });
                    }
                }
            }
            Err(PointsError::Inconsistent { equations: named })
        }
        other => other,
    }
}

/// One solve: its result, the original indices of the equations it pivoted on, and the
/// number of unknown questions.
struct Solved {
    result: Result<Vec<Exact>, PointsError>,
    pivot_rows: Vec<usize>,
    unknowns: usize,
}

fn solve_core(
    n: usize,
    equations: &[Equation],
    known: &[Option<Exact>],
) -> Result<Solved, PointsError> {
    if known.len() != n {
        return Err(PointsError::Shape(format!(
            "{} known values for {n} questions",
            known.len()
        )));
    }
    for (r, e) in equations.iter().enumerate() {
        if let Some(q) = e.questions.iter().find(|q| **q >= n) {
            return Err(PointsError::Shape(format!(
                "equation {r} names question {} of {n}",
                q + 1
            )));
        }
    }
    // Each equation's unknown questions, and its total with the known points moved over.
    let unknown: Vec<usize> = (0..n).filter(|q| known[*q].is_none()).collect();
    let mut col_of = vec![None; n];
    for (c, q) in unknown.iter().enumerate() {
        col_of[*q] = Some(c);
    }
    let mut cols: Vec<Vec<usize>> = Vec::with_capacity(equations.len());
    let mut rhs: Vec<Exact> = Vec::with_capacity(equations.len());
    for e in equations {
        let mut row = Vec::new();
        let mut t = e.total;
        let mut seen = vec![false; n];
        for &q in &e.questions {
            if std::mem::replace(&mut seen[q], true) {
                continue;
            }
            match (col_of[q], &known[q]) {
                (Some(c), _) => row.push(c),
                (None, Some(p)) => t = ck(t.checked_sub(p))?,
                (None, None) => unreachable!("a question is known or unknown"),
            }
        }
        cols.push(row);
        rhs.push(t);
    }
    let mut best: Option<Solved> = None;
    for k in MERSENNE {
        let Some(s) = eliminate(k, equations, known, &unknown, &cols, &rhs)? else {
            continue;
        };
        if s.result.is_ok() {
            return Ok(s);
        }
        // Keep the refusal that says more: an inconsistency, else fewer undetermined points.
        let rank = |s: &Solved| match &s.result {
            Err(PointsError::Underdetermined { questions, .. }) => questions.len(),
            _ => 0,
        };
        best = match best {
            Some(b) if rank(&b) <= rank(&s) => Some(b),
            _ => Some(s),
        };
    }
    best.ok_or(PointsError::Overflow)
}

/// Arithmetic modulo the Mersenne prime 2^k − 1.
#[derive(Clone, Copy)]
struct Field {
    k: u32,
    p: u64,
}

impl Field {
    fn new(k: u32) -> Self {
        Self {
            k,
            p: (1u64 << k) - 1,
        }
    }

    fn reduce(self, x: u128) -> u64 {
        let p = u128::from(self.p);
        let mut r = (x & p) + (x >> self.k);
        r = (r & p) + (r >> self.k);
        if r >= p {
            r -= p;
        }
        r as u64
    }

    fn mul(self, a: u64, b: u64) -> u64 {
        self.reduce(u128::from(a) * u128::from(b))
    }

    fn sub(self, a: u64, b: u64) -> u64 {
        if a >= b { a - b } else { a + self.p - b }
    }

    fn pow(self, mut a: u64, mut e: u64) -> u64 {
        let mut r = 1u64;
        while e > 0 {
            if e & 1 == 1 {
                r = self.mul(r, a);
            }
            a = self.mul(a, a);
            e >>= 1;
        }
        r
    }

    fn inv(self, a: u64) -> u64 {
        self.pow(a, self.p - 2)
    }

    /// `x` mod p; `None` when its denominator is a multiple of p.
    fn of(self, x: &Exact) -> Option<u64> {
        let p = i128::from(self.p);
        let d = x.denom().rem_euclid(p) as u64;
        if d == 0 {
            return None;
        }
        let n = x.numer().rem_euclid(p) as u64;
        Some(self.mul(n, self.inv(d)))
    }

    /// The fraction a/b with |a|, b ≤ ⌊√(p/2)⌋ that reduces to `u` (Wang's rational
    /// reconstruction; unique when it exists), else `None`.
    fn fraction(self, u: u64) -> Option<Exact> {
        let half = self.p / 2;
        let mut bound = (half as f64).sqrt() as u64;
        while bound * bound > half {
            bound -= 1;
        }
        while (bound + 1) * (bound + 1) <= half {
            bound += 1;
        }
        let bound = i128::from(bound);
        let (mut r0, mut r1) = (i128::from(self.p), i128::from(u));
        let (mut s0, mut s1) = (0i128, 1i128);
        while r1 > bound {
            let q = r0 / r1;
            (r0, r1) = (r1, r0 - q * r1);
            (s0, s1) = (s1, s0 - q * s1);
        }
        if s1 == 0 || s1.abs() > bound {
            return None;
        }
        let (a, b) = if s1 < 0 { (-r1, -s1) } else { (r1, s1) };
        let x = Exact::new(a, b);
        (*x.denom() == b).then_some(x)
    }
}

/// Gauss–Jordan elimination modulo 2^k − 1; `None` when a total's denominator is a multiple of
/// the prime (another prime is tried).
fn eliminate(
    k: u32,
    equations: &[Equation],
    known: &[Option<Exact>],
    unknown: &[usize],
    cols: &[Vec<usize>],
    rhs: &[Exact],
) -> Result<Option<Solved>, PointsError> {
    let f = Field::new(k);
    let m = unknown.len();
    let rows = cols.len();
    let mut a: Vec<Vec<u64>> = Vec::with_capacity(rows);
    let mut b: Vec<u64> = Vec::with_capacity(rows);
    for (c, t) in cols.iter().zip(rhs) {
        let mut row = vec![0u64; m];
        for &j in c {
            row[j] = 1;
        }
        let Some(t) = f.of(t) else {
            return Ok(None);
        };
        a.push(row);
        b.push(t);
    }
    let mut orig: Vec<usize> = (0..rows).collect();
    let mut pivots: Vec<(usize, usize)> = Vec::new(); // (row, column)
    let mut r = 0usize;
    for c in 0..m {
        if r == rows {
            break;
        }
        let Some(pi) = (r..rows).find(|&i| a[i][c] != 0) else {
            continue;
        };
        a.swap(r, pi);
        b.swap(r, pi);
        orig.swap(r, pi);
        let piv = a[r][c];
        if piv != 1 {
            let inv = f.inv(piv);
            for x in a[r].iter_mut().skip(c) {
                *x = f.mul(*x, inv);
            }
            b[r] = f.mul(b[r], inv);
        }
        let pivot_row = a[r].clone();
        let nz: Vec<usize> = (c..m).filter(|&j| pivot_row[j] != 0).collect();
        let br = b[r];
        for i in 0..rows {
            if i == r || a[i][c] == 0 {
                continue;
            }
            let g = a[i][c];
            for &j in &nz {
                a[i][j] = f.sub(a[i][j], f.mul(g, pivot_row[j]));
            }
            b[i] = f.sub(b[i], f.mul(g, br));
        }
        pivots.push((r, c));
        r += 1;
    }
    let pivot_rows: Vec<usize> = pivots.iter().map(|&(row, _)| orig[row]).collect();
    // Columns without a pivot are free; a pivot column whose row also holds a free column is
    // tied to it. Either way its value is not determined.
    let mut is_pivot = vec![false; m];
    for &(_, c) in &pivots {
        is_pivot[c] = true;
    }
    let free: Vec<usize> = (0..m).filter(|c| !is_pivot[*c]).collect();
    let mut undetermined: Vec<usize> = free.iter().map(|&c| unknown[c]).collect();
    let mut values: Vec<Option<Exact>> = known.to_vec();
    for &(row, c) in &pivots {
        if free.iter().any(|&j| a[row][j] != 0) {
            undetermined.push(unknown[c]);
        } else {
            match f.fraction(b[row]) {
                Some(x) => values[unknown[c]] = Some(x),
                // No fraction of a size points could have: as good as undetermined.
                None => undetermined.push(unknown[c]),
            }
        }
    }
    undetermined.sort_unstable();
    undetermined.dedup();
    let solved = |result| {
        Ok(Some(Solved {
            result,
            pivot_rows: pivot_rows.clone(),
            unknowns: m,
        }))
    };
    // Rows below the rank must read 0 = 0.
    if !(pivots.len()..rows).all(|i| b[i] == 0) {
        let mut bad = Vec::new();
        if undetermined.is_empty() {
            // The points of the pivot equations; name the equations they contradict.
            let p: Vec<Exact> = values
                .iter()
                .map(|v| v.unwrap_or_else(Exact::zero))
                .collect();
            bad = unreproduced(equations, &p).unwrap_or_default();
        }
        return solved(Err(PointsError::Inconsistent { equations: bad }));
    }
    if !undetermined.is_empty() {
        for &q in &undetermined {
            values[q] = None;
        }
        return solved(Err(PointsError::Underdetermined {
            questions: undetermined,
            determined: values,
        }));
    }
    // A unique solution: verified exactly against every equation.
    let p: Vec<Exact> = values
        .iter()
        .map(|v| v.unwrap_or_else(Exact::zero))
        .collect();
    let bad = unreproduced(equations, &p)?;
    if !bad.is_empty() {
        return solved(Err(PointsError::Inconsistent { equations: bad }));
    }
    solved(Ok(p))
}

/// The equations (0-based) that `points` do not reproduce exactly.
pub fn unreproduced(equations: &[Equation], points: &[Exact]) -> Result<Vec<usize>, PointsError> {
    let mut bad = Vec::new();
    for (r, e) in equations.iter().enumerate() {
        let mut seen = vec![false; points.len()];
        let mut s = Exact::zero();
        for &q in &e.questions {
            let Some(p) = points.get(q) else {
                return Err(PointsError::Shape(format!(
                    "equation {r} names question {} of {}",
                    q + 1,
                    points.len()
                )));
            };
            if !std::mem::replace(&mut seen[q], true) {
                s = ck(s.checked_add(p))?;
            }
        }
        if s != e.total {
            bad.push(r);
        }
    }
    Ok(bad)
}

/// An exact value from a spreadsheet number: a decimal with at most 9 places as written by
/// the shortest round-trip form of the `f64` (`17.75`, `0.5`), else the simplest fraction with
/// a denominator ≤ 1,000 within 10⁻⁹ (`0.6666666666666666` → 2/3). `None` for NaN, infinities
/// and values no such fraction matches.
pub fn exact_from_f64(v: f64) -> Option<Exact> {
    if !v.is_finite() || v.abs() > 1e15 {
        return None;
    }
    let s = format!("{v}");
    if !s.contains(['e', 'E'])
        && s.split_once('.').is_none_or(|(_, f)| f.len() <= 9)
        && let Ok(p) = crate::num::parse_points(&s)
    {
        return Some(crate::num::widen(&p));
    }
    // The smallest denominator that matches.
    for d in 1..=1000i128 {
        let n = (v * d as f64).round();
        if ((n / d as f64) - v).abs() <= 1e-9 {
            return Some(Exact::new(n as i128, d));
        }
    }
    None
}

/// Exact decimal text of `x` when its denominator has only the factors 2 and 5 (`0.75`,
/// `1.5`, `2`), else the fraction (`1/3`): the forms [`crate::parse_points`] reads back.
pub fn exact_text(x: &Exact) -> String {
    let (n, d) = (*x.numer(), *x.denom());
    let mut dd = d;
    let (mut twos, mut fives) = (0u32, 0u32);
    while dd % 2 == 0 {
        dd /= 2;
        twos += 1;
    }
    while dd % 5 == 0 {
        dd /= 5;
        fives += 1;
    }
    if dd != 1 {
        return format!("{n}/{d}");
    }
    let places = twos.max(fives);
    let Some(scale) = 10i128.checked_pow(places) else {
        return format!("{n}/{d}");
    };
    let Some(v) = n.checked_mul(scale / d) else {
        return format!("{n}/{d}");
    };
    if places == 0 {
        return v.to_string();
    }
    let neg = v < 0;
    let digits = v.unsigned_abs().to_string();
    let p = places as usize;
    let digits = format!("{digits:0>width$}", width = p + 1);
    let (int, frac) = digits.split_at(digits.len() - p);
    let frac = frac.trim_end_matches('0');
    let body = if frac.is_empty() {
        int.to_string()
    } else {
        format!("{int}.{frac}")
    };
    if neg { format!("-{body}") } else { body }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::num::exact;
    use proptest::prelude::*;

    fn eq(q: &[usize], t: Exact) -> Equation {
        Equation {
            questions: q.to_vec(),
            total: t,
        }
    }

    #[test]
    fn solves_quarter_and_half_points_exactly() {
        // Three questions worth 3/4, 1/2 and 1.
        let p = [exact(3, 4), exact(1, 2), exact(1, 1)];
        let rows: Vec<Vec<usize>> = vec![vec![0], vec![1, 2], vec![0, 2], vec![0, 1, 2]];
        let eqs: Vec<Equation> = rows
            .iter()
            .map(|r| eq(r, r.iter().map(|q| p[*q]).sum()))
            .collect();
        assert_eq!(solve_points(3, &eqs, &[None, None, None]).unwrap(), p);
        // A known value is fixed, the rest solved.
        let k = [None, Some(exact(1, 2)), None];
        assert_eq!(solve_points(3, &eqs, &k).unwrap(), p);
    }

    #[test]
    fn questions_earned_by_the_same_students_are_underdetermined() {
        // Questions 1 and 2 (0-based 0, 1) always go together; question 3 never earned.
        let eqs = vec![
            eq(&[0, 1], exact(2, 1)),
            eq(&[0, 1, 3], exact(3, 1)),
            eq(&[3], exact(1, 1)),
            eq(&[], exact(0, 1)),
        ];
        let e = solve_points(4, &eqs, &[None, None, None, None]).unwrap_err();
        assert_eq!(
            e,
            PointsError::Underdetermined {
                questions: vec![0, 1, 2],
                determined: vec![None, None, None, Some(exact(1, 1))],
            }
        );
        // Giving one of the pair and the unearned question settles the rest.
        let p = solve_points(4, &eqs, &[Some(exact(1, 1)), None, Some(exact(0, 1)), None]).unwrap();
        assert_eq!(p, vec![exact(1, 1), exact(1, 1), exact(0, 1), exact(1, 1)]);
    }

    #[test]
    fn totals_no_points_reproduce_are_refused_naming_them() {
        let eqs = vec![
            eq(&[0], exact(1, 1)),
            eq(&[1], exact(2, 1)),
            eq(&[0, 1], exact(3, 1)),
            eq(&[0, 1], exact(7, 2)), // half a point of bonus
        ];
        match solve_points(2, &eqs, &[None, None]).unwrap_err() {
            PointsError::Inconsistent { equations } => assert_eq!(equations, vec![3]),
            e => panic!("{e:?}"),
        }
        // A known value that contradicts the totals.
        let eqs = vec![eq(&[0], exact(1, 1))];
        assert!(matches!(
            solve_points(1, &eqs, &[Some(exact(2, 1))]),
            Err(PointsError::Inconsistent { .. })
        ));
    }

    #[test]
    fn shape_errors_are_reported() {
        assert!(matches!(
            solve_points(2, &[eq(&[2], exact(1, 1))], &[None, None]),
            Err(PointsError::Shape(_))
        ));
        assert!(matches!(
            solve_points(2, &[], &[None]),
            Err(PointsError::Shape(_))
        ));
    }

    #[test]
    fn spreadsheet_numbers_become_exact_values() {
        assert_eq!(exact_from_f64(17.75), Some(exact(71, 4)));
        assert_eq!(exact_from_f64(20.0), Some(exact(20, 1)));
        assert_eq!(exact_from_f64(2.0 / 3.0), Some(exact(2, 3)));
        assert_eq!(exact_from_f64(f64::NAN), None);
        assert_eq!(exact_text(&exact(3, 4)), "0.75");
        assert_eq!(exact_text(&exact(5, 2)), "2.5");
        assert_eq!(exact_text(&exact(2, 1)), "2");
        assert_eq!(exact_text(&exact(1, 3)), "1/3");
        assert_eq!(exact_text(&exact(-1, 8)), "-0.125");
        assert_eq!(exact_text(&exact(1, 40)), "0.025");
        for s in ["0.75", "2.5", "2", "1/3", "-0.125", "0.025"] {
            let p = crate::num::parse_points(s).unwrap();
            assert_eq!(exact_text(&crate::num::widen(&p)), s);
        }
    }

    /// A class's answer patterns: `students` rows earning each question with probability 3/5
    /// (deterministic xorshift), totals from `p`, and the total-possible equation.
    fn class(p: &[Exact], students: usize, seed: u64) -> Vec<Equation> {
        let mut s = seed;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let n = p.len();
        let mut eqs: Vec<Equation> = (0..students)
            .map(|_| {
                let qs: Vec<usize> = (0..n).filter(|_| rnd() % 5 < 3).collect();
                let total = qs.iter().map(|q| p[*q]).sum();
                Equation {
                    questions: qs,
                    total,
                }
            })
            .collect();
        eqs.push(Equation {
            questions: (0..n).collect(),
            total: p.iter().sum(),
        });
        eqs
    }

    #[test]
    fn a_hundred_question_exam_is_solved_exactly_without_overflow() {
        // Regression: exact elimination over i128 fractions overflowed from about 60 questions
        // (the numbers grow like the minors of a 0/1 matrix), so a 100-question IAU exam could
        // not be imported. 3/4, 1 and 1/2 points; 300 students.
        let p: Vec<Exact> = (0..100)
            .map(|q| match q % 3 {
                0 => exact(3, 4),
                1 => exact(1, 1),
                _ => exact(1, 2),
            })
            .collect();
        let eqs = class(&p, 300, 0x9e37_79b9_7f4a_7c15);
        assert_eq!(solve_points(100, &eqs, &vec![None; 100]).unwrap(), p);
        // One total edited by hand among them is named.
        let mut bad = eqs.clone();
        bad[17].total += exact(1, 4);
        let t = std::time::Instant::now();
        match solve_points(100, &bad, &vec![None; 100]).unwrap_err() {
            PointsError::Inconsistent { equations } => assert_eq!(equations, vec![17]),
            e => panic!("{e:?}"),
        }
        assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
        // The edited total on an equation the elimination pivots on is found too.
        let mut first = eqs.clone();
        first[0].total += exact(1, 2);
        match solve_points(100, &first, &vec![None; 100]).unwrap_err() {
            PointsError::Inconsistent { equations } => assert_eq!(equations, vec![0]),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn fractions_are_rebuilt_from_their_residues() {
        for k in MERSENNE {
            let f = Field::new(k);
            // 2^k − 1 is prime: Fermat's little theorem holds for a few bases.
            for a in [2u64, 3, 5, 7, 11, 13] {
                assert_eq!(f.pow(a, f.p - 1), 1, "2^{k} - 1");
            }
            for x in [
                exact(3, 4),
                exact(-1, 8),
                exact(0, 1),
                exact(7, 3),
                exact(250, 1),
            ] {
                assert_eq!(f.fraction(f.of(&x).unwrap()), Some(x));
                assert_eq!(
                    f.mul(f.of(&x).unwrap(), f.of(&exact(1, 1)).unwrap()),
                    f.of(&x).unwrap()
                );
            }
        }
        // A residue whose fraction would be larger than the bound is not a value of points.
        let f = Field::new(31);
        assert_eq!(f.fraction(f.of(&exact(1, 40_000)).unwrap()), None);
    }

    proptest! {
        /// Random 0/1 answer patterns with quarter points: whenever the solver answers, the
        /// answer reproduces every total; with enough distinct patterns it is the truth.
        #[test]
        fn solutions_always_reproduce_every_total(
            quarters in proptest::collection::vec(1i128..=8, 2..10),
            seed in proptest::collection::vec(any::<u32>(), 1..60),
        ) {
            let n = quarters.len();
            let p: Vec<Exact> = quarters.iter().map(|q| exact(*q, 4)).collect();
            let mut eqs: Vec<Equation> = seed
                .iter()
                .map(|s| {
                    let qs: Vec<usize> = (0..n).filter(|q| s >> (q % 32) & 1 == 1).collect();
                    let t = qs.iter().map(|q| p[*q]).sum();
                    Equation { questions: qs, total: t }
                })
                .collect();
            eqs.push(Equation { questions: (0..n).collect(), total: p.iter().sum() });
            match solve_points(n, &eqs, &vec![None; n]) {
                Ok(got) => {
                    prop_assert!(unreproduced(&eqs, &got).unwrap().is_empty());
                    prop_assert_eq!(got, p);
                }
                Err(PointsError::Underdetermined { questions, determined }) => {
                    prop_assert!(!questions.is_empty());
                    // What the equations do determine is the truth.
                    for (q, d) in determined.iter().enumerate() {
                        if let Some(d) = d {
                            prop_assert_eq!(*d, p[q]);
                        }
                    }
                }
                Err(e) => prop_assert!(false, "{e:?}"),
            }
        }
    }
}
