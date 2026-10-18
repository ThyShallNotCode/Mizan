//! Batch assignment (§9.6): one sheet per student across the exam instance, with a private
//! "not on roster" column per booklet and a second-copy column per student, solved exactly on
//! integer costs (1/1024 nat) per connected component. Duplicates raise IDN-005; a booklet whose
//! assignment differs from its own best candidate raises IDN-004 `assignment_changed`; a booklet
//! assigned to a student marked absent, DN or W raises IDN-004 `assigned_to_absent`.
//!
//! Inputs are sorted by booklet ID (UUIDv7 text) and students by key, so the outcome does not
//! depend on batch or booklet order (§23.6).

use crate::decode::{
    IdentityDecision, IdentityPolicy, IdentityStatus, PriorContext, SubReason, nats, q,
};
use crate::normalize::normalize_id;
use crate::roster::{RosterSnapshot, SisStatus};
use omr_model::{Exception, Scope, Severity, StudentKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Big-M for missing edges (2³⁰ quanta, §9.6); the solver never uses one.
const BIG: i64 = 1 << 30;

/// One booklet entering the assignment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookletIdentity {
    /// Booklet identifier (UUIDv7 text sorts by creation).
    pub booklet: String,
    /// Its identity decision.
    pub decision: IdentityDecision,
    /// A confirmed pair (human-confirmed, or auto-accepted in an earlier solve): fixed.
    pub confirmed: Option<StudentKey>,
}

/// What the booklet was assigned to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Assigned {
    /// The student.
    Student(StudentKey),
    /// A second copy of a student who already has a booklet (IDN-005).
    Duplicate(StudentKey),
    /// Not on the roster (∅).
    NotOnRoster,
}

/// One booklet's assignment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookletAssignment {
    /// Booklet identifier.
    pub booklet: String,
    /// Assignment.
    pub assigned: Assigned,
    /// Final status after the batch rules.
    pub status: IdentityStatus,
    /// Batch margin Δ_i (quantised), for booklets assigned to a student.
    pub margin_q: Option<i64>,
    /// The decision with batch findings added and exceptions re-rendered.
    pub decision: IdentityDecision,
}

/// The batch outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchOutcome {
    /// Per booklet, sorted by booklet ID.
    pub booklets: Vec<BookletAssignment>,
    /// IDN-005 (Student scope) per student claimed by more than one booklet.
    pub duplicates: Vec<(StudentKey, Exception)>,
    /// Expected students without a booklet (absent/unreturned list).
    pub absent: Vec<StudentKey>,
    /// DN students: never expected, never absent (§9.6).
    pub denied_entry: Vec<StudentKey>,
    /// W students without a booklet.
    pub withdrawn: Vec<StudentKey>,
    /// Rows of the largest connected component solved (diagnostics; alarm above 500).
    pub largest_component: usize,
}

/// Minimum-cost assignment of every row to a distinct column (`rows ≤ cols`), Hungarian
/// algorithm with potentials, O(n²·m), exact on integers. Ties resolve to the lowest column.
pub(crate) fn hungarian(cost: &[Vec<i64>]) -> Vec<usize> {
    let n = cost.len();
    if n == 0 {
        return Vec::new();
    }
    let m = cost[0].len();
    debug_assert!(n <= m);
    const INF: i64 = i64::MAX / 4;
    let mut u = vec![0i64; n + 1];
    let mut v = vec![0i64; m + 1];
    let mut p = vec![0usize; m + 1];
    let mut way = vec![0usize; m + 1];
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![INF; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = INF;
            let mut j1 = 0usize;
            for j in 1..=m {
                if !used[j] {
                    let cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut ans = vec![usize::MAX; n];
    for j in 1..=m {
        if p[j] != 0 {
            ans[p[j] - 1] = j - 1;
        }
    }
    ans
}

fn total(cost: &[Vec<i64>], a: &[usize]) -> i64 {
    a.iter().enumerate().map(|(i, &j)| cost[i][j]).sum()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Col {
    Student(usize),
    Dup(usize),
    Null(usize),
}

/// Solves the batch against a roster (`identity_mode = roster`).
pub fn assign_batch(
    items: &[BookletIdentity],
    roster: &RosterSnapshot,
    prior: &PriorContext,
    policy: &IdentityPolicy,
) -> BatchOutcome {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[a].booklet.cmp(&items[b].booklet).then(a.cmp(&b)));
    let lambda_new = q(policy.lambda_new);
    let lambda_dup = q(policy.lambda_dup);
    let h_q = q(policy.hysteresis);

    // Students are indexed by roster position.
    let student_idx = |k: &StudentKey| roster.position(&normalize_id(&k.0));
    let mut fixed: BTreeMap<usize, Vec<usize>> = BTreeMap::new(); // student → fixed rows
    let mut unfixed: Vec<usize> = Vec::new();
    for &r in &order {
        match &items[r].confirmed {
            Some(k) => {
                // A confirmed pair is never re-solved; one whose student is missing from this
                // snapshot stays out of the solve and is flagged `roster_diff` below.
                if let Some(s) = student_idx(k) {
                    fixed.entry(s).or_default().push(r);
                }
            }
            None => unfixed.push(r),
        }
    }

    // Edges of unfixed rows: (student, cost); ∅ cost per row.
    let mut edges: BTreeMap<usize, Vec<(usize, i64)>> = BTreeMap::new();
    let mut null_cost: BTreeMap<usize, i64> = BTreeMap::new();
    for &r in &unfixed {
        let dec = &items[r].decision;
        let nc = dec.null_log_posterior_q.map_or(0, |l| -l) + lambda_new;
        null_cost.insert(r, nc);
        let mut es = Vec::new();
        if !dec.has(SubReason::NotOnRoster) {
            for c in &dec.candidates {
                if let Some(s) = student_idx(&c.student) {
                    let cost = -c.log_posterior_q;
                    // Exact pruning: an edge as expensive as the private ∅ column is never needed.
                    if cost < nc {
                        es.push((s, cost));
                    }
                }
            }
        }
        edges.insert(r, es);
    }

    // Connected components over shared students (union-find on rows).
    let mut parent: BTreeMap<usize, usize> = unfixed.iter().map(|&r| (r, r)).collect();
    fn find(p: &mut BTreeMap<usize, usize>, x: usize) -> usize {
        let mut r = x;
        while p[&r] != r {
            r = p[&r];
        }
        let mut y = x;
        while p[&y] != r {
            let n = p[&y];
            p.insert(y, r);
            y = n;
        }
        r
    }
    let mut owner: BTreeMap<usize, usize> = BTreeMap::new();
    for &r in &unfixed {
        for &(s, _) in &edges[&r] {
            match owner.get(&s).copied() {
                Some(o) => {
                    let (a, b) = (find(&mut parent, o), find(&mut parent, r));
                    if a != b {
                        parent.insert(a.max(b), a.min(b));
                    }
                }
                None => {
                    owner.insert(s, r);
                }
            }
        }
    }
    let mut comps: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &r in &unfixed {
        let root = find(&mut parent, r);
        comps.entry(root).or_default().push(r);
    }

    let mut assigned: BTreeMap<usize, (Assigned, Option<i64>)> = BTreeMap::new();
    let mut largest = 0;
    for rows in comps.values() {
        largest = largest.max(rows.len());
        let students: BTreeSet<usize> = rows
            .iter()
            .flat_map(|r| edges[r].iter().map(|e| e.0))
            .collect();
        let mut cols: Vec<Col> = Vec::new();
        for &s in &students {
            if !fixed.contains_key(&s) {
                cols.push(Col::Student(s));
            }
            cols.push(Col::Dup(s));
        }
        for (k, _) in rows.iter().enumerate() {
            cols.push(Col::Null(k));
        }
        let cost: Vec<Vec<i64>> = rows
            .iter()
            .enumerate()
            .map(|(k, r)| {
                cols.iter()
                    .map(|c| match *c {
                        Col::Student(s) => edges[r].iter().find(|e| e.0 == s).map_or(BIG, |e| e.1),
                        Col::Dup(s) => edges[r]
                            .iter()
                            .find(|e| e.0 == s)
                            .map_or(BIG, |e| e.1 + lambda_dup),
                        Col::Null(j) => {
                            if j == k {
                                null_cost[r]
                            } else {
                                BIG
                            }
                        }
                    })
                    .collect()
            })
            .collect();
        let sol = hungarian(&cost);
        let opt = total(&cost, &sol);
        for (k, &r) in rows.iter().enumerate() {
            let col = cols[sol[k]];
            debug_assert!(cost[k][sol[k]] < BIG, "big-M edge used");
            let (a, s_star) = match col {
                Col::Student(s) => (Assigned::Student(roster.rows()[s].key.clone()), Some(s)),
                Col::Dup(s) => (Assigned::Duplicate(roster.rows()[s].key.clone()), Some(s)),
                Col::Null(_) => (Assigned::NotOnRoster, None),
            };
            // Margin: re-solve with row k forbidden from s* and dup(s*).
            let margin = s_star.map(|s| {
                let mut c2 = cost.clone();
                for (j, c) in cols.iter().enumerate() {
                    if matches!(c, Col::Student(x) | Col::Dup(x) if *x == s) {
                        c2[k][j] = BIG;
                    }
                }
                let sol2 = hungarian(&c2);
                (total(&c2, &sol2) - opt).min(BIG)
            });
            assigned.insert(r, (a, margin));
        }
    }

    // ---- Post-processing.
    let mut claims: BTreeMap<usize, Vec<String>> = BTreeMap::new(); // student → booklets
    for (&s, rows) in &fixed {
        for &r in rows {
            claims.entry(s).or_default().push(items[r].booklet.clone());
        }
    }
    for (&r, (a, _)) in &assigned {
        if let Assigned::Student(k) | Assigned::Duplicate(k) = a
            && let Some(s) = student_idx(k)
        {
            claims.entry(s).or_default().push(items[r].booklet.clone());
        }
    }
    // Strong competing claims that ended on ∅ (two strong matches).
    for (&r, (a, _)) in &assigned {
        if *a == Assigned::NotOnRoster {
            let dec = &items[r].decision;
            if let Some(best) = &dec.student
                && let Some(s) = student_idx(best)
                && dec
                    .candidates
                    .first()
                    .is_some_and(|c| c.log_posterior_q >= q(0.5f64.ln()))
                && claims.get(&s).is_some_and(|v| !v.is_empty())
            {
                claims.entry(s).or_default().push(items[r].booklet.clone());
            }
        }
    }
    let dup_students: BTreeSet<usize> = claims
        .iter()
        .filter(|(_, b)| b.len() > 1)
        .map(|(s, _)| *s)
        .collect();
    let duplicates: Vec<(StudentKey, Exception)> = dup_students
        .iter()
        .map(|&s| {
            let key = roster.rows()[s].key.clone();
            let mut bs = claims[&s].clone();
            bs.sort();
            let e = Exception::must("IDN-005", Severity::Error, Scope::Student).with_detail(
                format!("{} booklets resolve to {key}: {}", bs.len(), bs.join(", ")),
            );
            (key, e)
        })
        .collect();

    let absent_keys = prior.absent_keys();
    let absent_like = |s: usize| {
        let r = &roster.rows()[s];
        absent_keys.contains(&r.key.0)
            || matches!(
                r.record.sis_status,
                Some(SisStatus::DeniedEntry | SisStatus::Withdrawn)
            )
    };

    let mut out = Vec::with_capacity(items.len());
    for &r in &order {
        let item = &items[r];
        let mut dec = item.decision.clone();
        if let Some(k) = &item.confirmed {
            // Fixed pair: never moved, never re-flagged as assignment_changed. When its student
            // is missing from this snapshot it keeps the student and goes to review with
            // `roster_diff` (§9.2 "confirmed pairs never move silently", §9.9 row 19).
            let (assigned, status) = match student_idx(k) {
                Some(s) => (Assigned::Student(roster.rows()[s].key.clone()), dec.status),
                None => {
                    if !dec.has(SubReason::RosterDiff) {
                        dec.findings.push(crate::decode::Finding {
                            sub_reason: SubReason::RosterDiff,
                            detail: format!(
                                "confirmed student {k} is not on the current roster snapshot; kept, not re-assigned"
                            ),
                        });
                    }
                    dec.status = IdentityStatus::Review;
                    dec.render_exceptions();
                    (
                        Assigned::Student(StudentKey(normalize_id(&k.0))),
                        IdentityStatus::Review,
                    )
                }
            };
            out.push(BookletAssignment {
                booklet: item.booklet.clone(),
                assigned,
                status,
                margin_q: None,
                decision: dec,
            });
            continue;
        }
        let (a, margin) = assigned
            .get(&r)
            .cloned()
            .unwrap_or((Assigned::NotOnRoster, None));
        let own = dec.student.clone();
        let mut involved_in_dup = false;
        match &a {
            Assigned::Student(k) | Assigned::Duplicate(k) => {
                if own.as_ref().is_some_and(|o| o != k) {
                    dec.findings.push(crate::decode::Finding {
                        sub_reason: SubReason::AssignmentChanged,
                        detail: format!(
                            "batch assignment {k} differs from the booklet's best candidate {}",
                            own.as_ref().map_or_else(String::new, |o| o.0.clone())
                        ),
                    });
                }
                if let Some(s) = student_idx(k) {
                    involved_in_dup = dup_students.contains(&s);
                    if absent_like(s) {
                        dec.findings.push(crate::decode::Finding {
                            sub_reason: SubReason::AssignedToAbsent,
                            detail: format!("{k} is marked absent, DN or W"),
                        });
                    }
                }
            }
            Assigned::NotOnRoster => {
                if let Some(o) = &own
                    && let Some(s) = student_idx(o)
                {
                    involved_in_dup = dup_students.contains(&s);
                    if !involved_in_dup && !dec.has(SubReason::NotOnRoster) {
                        dec.findings.push(crate::decode::Finding {
                            sub_reason: SubReason::AssignmentChanged,
                            detail: format!(
                                "best candidate {o} went to another booklet; left unassigned"
                            ),
                        });
                    }
                }
            }
        }
        if dec.status == IdentityStatus::AutoAccepted
            && let Some(m) = margin
            && m < q(policy.tau_m) + h_q
        {
            dec.findings.push(crate::decode::Finding {
                sub_reason: SubReason::LowMargin,
                detail: format!(
                    "batch margin {:.2} nat < τ_m {:.1} (+h)",
                    nats(m),
                    policy.tau_m
                ),
            });
        }
        let blocking = dec.findings.iter().any(|f| {
            !(dec.status == IdentityStatus::AutoAccepted && f.sub_reason.info_if_auto_accepted())
        });
        let status = if blocking || involved_in_dup || dec.binding_conflict.is_some() {
            IdentityStatus::Review
        } else {
            dec.status
        };
        dec.status = status;
        dec.render_exceptions();
        out.push(BookletAssignment {
            booklet: item.booklet.clone(),
            assigned: a,
            status,
            margin_q: margin,
            decision: dec,
        });
    }

    // ---- Batch close lists.
    let taken: BTreeSet<usize> = out
        .iter()
        .filter_map(|b| match &b.assigned {
            Assigned::Student(k) => student_idx(k),
            _ => None,
        })
        .collect();
    let (mut absent, mut dn, mut w) = (Vec::new(), Vec::new(), Vec::new());
    for (s, row) in roster.rows().iter().enumerate() {
        // Students of sections not sitting this session are not expected here: never absent.
        if !taken.contains(&s) && !prior.expects(row.record.section.as_ref()) {
            continue;
        }
        match row.record.sis_status {
            Some(SisStatus::DeniedEntry) => dn.push(row.key.clone()),
            Some(SisStatus::Withdrawn) if !taken.contains(&s) => w.push(row.key.clone()),
            _ if !taken.contains(&s) => absent.push(row.key.clone()),
            _ => {}
        }
    }
    BatchOutcome {
        booklets: out,
        duplicates,
        absent,
        denied_entry: dn,
        withdrawn: w,
        largest_component: largest,
    }
}

/// Rosterless batch (`candidate_number` mode): one booklet per key; later or unconfirmed
/// copies of a key are duplicates (IDN-005).
pub fn assign_rosterless(items: &[BookletIdentity]) -> BatchOutcome {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        let ka = items[a].confirmed.is_none();
        let kb = items[b].confirmed.is_none();
        ka.cmp(&kb)
            .then(items[a].booklet.cmp(&items[b].booklet))
            .then(a.cmp(&b))
    });
    let key_of = |it: &BookletIdentity| {
        it.confirmed
            .clone()
            .or_else(|| it.decision.student.clone())
            .map(|k| StudentKey(normalize_id(&k.0)))
    };
    let mut claims: BTreeMap<StudentKey, Vec<String>> = BTreeMap::new();
    for &r in &order {
        if let Some(k) = key_of(&items[r]) {
            claims.entry(k).or_default().push(items[r].booklet.clone());
        }
    }
    let mut seen: BTreeSet<StudentKey> = BTreeSet::new();
    let mut out: Vec<BookletAssignment> = Vec::new();
    for &r in &order {
        let it = &items[r];
        let mut dec = it.decision.clone();
        let assigned = match key_of(it) {
            Some(k) if seen.insert(k.clone()) => Assigned::Student(k),
            Some(k) => Assigned::Duplicate(k),
            None => Assigned::NotOnRoster,
        };
        let dup = match &assigned {
            Assigned::Student(k) | Assigned::Duplicate(k) => claims[k].len() > 1,
            Assigned::NotOnRoster => false,
        };
        if dup && it.confirmed.is_none() {
            dec.status = IdentityStatus::Review;
            dec.render_exceptions();
        }
        out.push(BookletAssignment {
            booklet: it.booklet.clone(),
            assigned,
            status: dec.status,
            margin_q: None,
            decision: dec,
        });
    }
    out.sort_by(|a, b| a.booklet.cmp(&b.booklet));
    let duplicates = claims
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, mut v)| {
            v.sort();
            let e = Exception::must("IDN-005", Severity::Error, Scope::Student)
                .with_detail(format!("{} booklets carry {k}: {}", v.len(), v.join(", ")));
            (k, e)
        })
        .collect();
    BatchOutcome {
        booklets: out,
        duplicates,
        absent: Vec::new(),
        denied_entry: Vec::new(),
        withdrawn: Vec::new(),
        largest_component: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn brute(cost: &[Vec<i64>]) -> i64 {
        fn rec(cost: &[Vec<i64>], i: usize, used: &mut Vec<bool>) -> i64 {
            if i == cost.len() {
                return 0;
            }
            let mut best = i64::MAX;
            for j in 0..cost[0].len() {
                if !used[j] {
                    used[j] = true;
                    best = best.min(cost[i][j] + rec(cost, i + 1, used));
                    used[j] = false;
                }
            }
            best
        }
        rec(cost, 0, &mut vec![false; cost[0].len()])
    }

    proptest! {
        #[test]
        fn hungarian_is_optimal(n in 1usize..5, extra in 0usize..3, seed in any::<u64>()) {
            let m = n + extra;
            let mut x = seed | 1;
            let cost: Vec<Vec<i64>> = (0..n).map(|_| (0..m).map(|_| {
                x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                (x % 1000) as i64
            }).collect()).collect();
            let a = hungarian(&cost);
            let distinct: BTreeSet<usize> = a.iter().copied().collect();
            prop_assert_eq!(distinct.len(), n);
            prop_assert_eq!(total(&cost, &a), brute(&cost));
        }
    }
}
