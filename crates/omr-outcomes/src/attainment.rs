//! The CLO formulas of spec §11.3: per-student P_{n,c} under the student methods, class
//! attainment (average, threshold-proportion, Taif per-question, levels, FCAR/EAMU), CIs,
//! verdicts and badges, direct/indirect blends, report groups and pooling.

use crate::evidence::{EvidenceStatus, IndirectEvidence, ItemEvidence, Offering, Track};
use crate::framework::{CloSet, Domain};
use crate::mapping::{MappingSet, evidence_share};
use crate::policy::{
    AttainmentPolicy, BonusEvidence, ClassMethod, EamuPopulation, FcarFlags, Scale,
    SectionAggregation, StudentMethod, VerdictBasis,
};
use num_traits::{ToPrimitive, Zero};
use omr_model::{Exception, Scope, Severity, StudentKey};
use omr_scoring::num::{cmp_exact, reduce};
use omr_scoring::stats::{SplitMix64, Z95, percentile_sorted, t_interval, wilson_ci};
use omr_scoring::{Exact, RosterStatus, exact, widen};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering::Less;
use std::collections::{BTreeMap, BTreeSet};

/// One student's performance on one CLO (§11.3 "Student performance").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StudentClo {
    /// Student.
    pub student: StudentKey,
    /// Section of record.
    pub section: String,
    /// CLO code.
    pub clo: String,
    /// P_{n,c} ∈ [0, 1], exact; `None` without evidence.
    pub p: Option<Exact>,
    /// Σ ω(n) / planned Σ ω (per-student coverage).
    pub coverage: Exact,
    /// Counts in N_c.
    pub included: bool,
    /// Why the student is out of N_c.
    pub excluded_reason: Option<String>,
    /// A pending response or later sitting: provisional (SCO-033).
    pub provisional: bool,
    /// Passing letter (EAMU population).
    pub passing: Option<bool>,
}

/// FCAR flag (§11.3 EAMU row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FcarFlag {
    /// avg₅ < 3.3 and U share > 10 %.
    Red,
    /// Exactly one of the red conditions.
    Yellow,
    /// avg₅ > 4.6 and U = 0.
    Green,
    /// None of the above.
    None,
}

/// EAMU vector and FCAR flag.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Eamu {
    /// Excellent: P ≥ 0.90.
    pub e: usize,
    /// Adequate: [0.75, 0.90).
    pub a: usize,
    /// Minimal: [0.60, 0.75).
    pub m: usize,
    /// Unsatisfactory: < 0.60.
    pub u: usize,
    /// Population size.
    pub n: usize,
    /// avg₃ = (3E + 2A + M)/N.
    pub avg3: Exact,
    /// avg₅ = avg₃·5/3.
    pub avg5: Exact,
    /// Flag.
    pub flag: FcarFlag,
}

/// EAMU counts over `values` with cut-points (E, A, M) and FCAR thresholds (exact).
pub fn eamu(values: &[Exact], bands: &[Exact; 3], flags: &FcarFlags) -> Eamu {
    let (mut e, mut a, mut m, mut u) = (0, 0, 0, 0);
    for v in values {
        if cmp_exact(v, &bands[0]) != Less {
            e += 1;
        } else if cmp_exact(v, &bands[1]) != Less {
            a += 1;
        } else if cmp_exact(v, &bands[2]) != Less {
            m += 1;
        } else {
            u += 1;
        }
    }
    let n = values.len();
    let (avg3, avg5, flag) = if n == 0 {
        (Exact::zero(), Exact::zero(), FcarFlag::None)
    } else {
        let nn = Exact::from_integer(n as i128);
        let avg3 = Exact::from_integer((3 * e + 2 * a + m) as i128) / nn;
        let avg5 = avg3 * exact(5, 3);
        let low = avg5 < flags.red_avg5;
        let many_u = Exact::from_integer(u as i128) / nn > flags.red_u_share;
        let flag = if low && many_u {
            FcarFlag::Red
        } else if low || many_u {
            FcarFlag::Yellow
        } else if avg5 > flags.green_avg5 && u == 0 {
            FcarFlag::Green
        } else {
            FcarFlag::None
        };
        (avg3, avg5, flag)
    };
    Eamu {
        e,
        a,
        m,
        u,
        n,
        avg3,
        avg5,
        flag,
    }
}

/// Class statistics for one CLO over one population (§11.3 "Class attainment").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassStats {
    /// N_c.
    pub n: usize,
    /// μ_c (exact).
    pub mean: Option<Exact>,
    /// Sample SD (divisor N − 1), f64.
    pub sample_sd: Option<f64>,
    /// t-interval for μ_c.
    pub mean_ci: Option<(f64, f64)>,
    /// Seeded percentile bootstrap CI for μ_c when N < 30.
    pub mean_bootstrap_ci: Option<(f64, f64)>,
    /// x_c = #{P ≥ τ}.
    pub met: usize,
    /// π_c = x_c/N_c (exact).
    pub proportion: Option<Exact>,
    /// Wilson 95 % CI for π_c.
    pub proportion_ci: Option<(f64, f64)>,
    /// Taif per-question ρ_c (exact).
    pub per_question: Option<Exact>,
    /// Bootstrap CI for ρ_c (students resampled).
    pub per_question_ci: Option<(f64, f64)>,
    /// L_c = max{ℓ : π_c ≥ T_ℓ}.
    pub level: Option<u8>,
    /// EAMU over the configured population.
    pub eamu: Option<Eamu>,
}

/// Per-question input (§11.3 Taif row): per item of I_c its weight w_{i,c}, and per included
/// student whether s⁺ ≥ τ_c·M_i (decided once, exactly).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PerQuestionData {
    /// w_{i,c} per item of I_c.
    pub weights: Vec<Exact>,
    /// Row-major cells, `weights.len()` per student: 0 = not presented, 1 = below τ·M_i,
    /// 2 = at or above τ·M_i.
    pub cells: Vec<u8>,
}

impl PerQuestionData {
    /// Builds the flags from exact scores: `items[k]` = (w, M_i); `rows[n][k]` = s⁺ (`None`
    /// when not presented).
    pub fn from_scores(items: &[(Exact, Exact)], rows: &[Vec<Option<Exact>>], tau: &Exact) -> Self {
        let mut cells = Vec::with_capacity(rows.len() * items.len());
        for r in rows {
            for (k, (_, m)) in items.iter().enumerate() {
                cells.push(match r.get(k).copied().flatten() {
                    None => 0,
                    Some(s) if cmp_exact(&s, &(tau * m)) != Less => 2,
                    Some(_) => 1,
                });
            }
        }
        Self {
            weights: items.iter().map(|(w, _)| *w).collect(),
            cells,
        }
    }

    /// ρ_c = Σ w·q_i / Σ w with q_i = #{n : s⁺ ≥ τ·M_i}/N_i over the rows in `subset`
    /// presented item i.
    pub fn rho(&self, subset: &[usize]) -> Option<Exact> {
        let k = self.weights.len();
        if k == 0 {
            return None;
        }
        let mut hit = vec![0i128; k];
        let mut presented = vec![0i128; k];
        for &n in subset {
            if let Some(row) = self.cells.get(n * k..(n + 1) * k) {
                for (j, &c) in row.iter().enumerate() {
                    presented[j] += i128::from(c != 0);
                    hit[j] += i128::from(c == 2);
                }
            }
        }
        let mut num = Exact::zero();
        let mut den = Exact::zero();
        for j in 0..k {
            if presented[j] > 0 {
                num += self.weights[j] * reduce(hit[j], presented[j]);
                den += self.weights[j];
            }
        }
        (!den.is_zero()).then(|| num / den)
    }
}

/// Cell-operation budget of the synchronous per-question bootstrap (students × items × B).
/// Larger classes get no CI on the synchronous path; the app computes it asynchronously.
pub const BOOTSTRAP_BUDGET: usize = 2_000_000;

/// Exact sum that adds numerators of equal denominators in integers (P values of one CLO
/// share a handful of denominators), falling back to rational addition.
pub fn sum_exact<'a>(values: impl IntoIterator<Item = &'a Exact>) -> Exact {
    let mut groups: Vec<(i128, i128)> = Vec::new();
    let mut spill = Exact::zero();
    for v in values {
        let (n, d) = (*v.numer(), *v.denom());
        if let Some(g) = groups.iter_mut().find(|g| g.0 == d) {
            match g.1.checked_add(n) {
                Some(x) => g.1 = x,
                None => spill += v,
            }
        } else if groups.len() < 64 {
            groups.push((d, n));
        } else {
            spill += v;
        }
    }
    groups
        .into_iter()
        .fold(spill, |acc, (d, n)| acc + reduce(n, d))
}

fn mean(v: &[Exact]) -> Option<Exact> {
    (!v.is_empty()).then(|| sum_exact(v) / Exact::from_integer(v.len() as i128))
}

/// Display/CI conversion; exact operands below 2⁵³ divide in one correctly rounded step.
fn f(x: &Exact) -> f64 {
    const EXACT_F64: i128 = 1 << 53;
    let (n, d) = (*x.numer(), *x.denom());
    if n.abs() < EXACT_F64 && d < EXACT_F64 {
        n as f64 / d as f64
    } else {
        x.to_f64().unwrap_or(f64::NAN)
    }
}

/// Computes the class statistics for one CLO over a population of P values (§11.3).
/// `passing[k]` marks students with a passing letter (EAMU `passing` population).
pub fn class_stats(
    values: &[Exact],
    passing: &[Option<bool>],
    tau: &Exact,
    policy: &AttainmentPolicy,
    per_question: Option<(&PerQuestionData, &[usize])>,
    rng: &mut SplitMix64,
) -> ClassStats {
    let n = values.len();
    let mu = mean(values);
    let sd = (n >= 2).then(|| {
        let m = mu.map(|m| f(&m)).unwrap_or(0.0);
        (values.iter().map(|v| (f(v) - m).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
    });
    let mean_ci = match (mu, sd) {
        (Some(m), Some(s)) => t_interval(f(&m), s, n as u64),
        _ => None,
    };
    let mean_bootstrap_ci = (2..30).contains(&n).then(|| {
        let fv: Vec<f64> = values.iter().map(f).collect();
        let mut means: Vec<f64> = (0..policy.bootstrap_b)
            .map(|_| {
                (0..n)
                    .map(|_| fv[rng.below(n as u64) as usize])
                    .sum::<f64>()
                    / n as f64
            })
            .collect();
        means.sort_by(f64::total_cmp);
        (
            percentile_sorted(&means, 0.025),
            percentile_sorted(&means, 0.975),
        )
    });
    let met = values.iter().filter(|v| cmp_exact(v, tau) != Less).count();
    let proportion = (n > 0).then(|| Exact::new(met as i128, n as i128));
    let proportion_ci = (n > 0).then(|| wilson_ci(met as u64, n as u64, Z95));
    let level = proportion.map(|p| {
        policy
            .levels
            .iter()
            .enumerate()
            .filter(|(_, t)| p >= **t)
            .map(|(l, _)| l as u8 + 1)
            .max()
            .unwrap_or(0)
    });
    let pop: Vec<Exact> = match policy.eamu_population {
        EamuPopulation::All => values.to_vec(),
        EamuPopulation::Passing => values
            .iter()
            .zip(passing)
            .filter(|(_, p)| **p == Some(true))
            .map(|(v, _)| *v)
            .collect(),
    };
    let eamu_v = (!pop.is_empty()).then(|| eamu(&pop, &policy.eamu_bands, &policy.fcar_flags));
    let (pq, pq_ci) = match per_question {
        Some((data, subset)) => {
            let rho = data.rho(subset);
            let budget = subset.len() * data.weights.len().max(1) * policy.bootstrap_b;
            let ci =
                (rho.is_some() && subset.len() >= 2 && budget <= BOOTSTRAP_BUDGET).then(|| {
                    let mut v: Vec<f64> = (0..policy.bootstrap_b)
                        .filter_map(|_| {
                            let s: Vec<usize> = (0..subset.len())
                                .map(|_| subset[rng.below(subset.len() as u64) as usize])
                                .collect();
                            data.rho(&s).map(|r| f(&r))
                        })
                        .collect();
                    v.sort_by(f64::total_cmp);
                    (percentile_sorted(&v, 0.025), percentile_sorted(&v, 0.975))
                });
            (rho, ci)
        }
        None => (None, None),
    };
    ClassStats {
        n,
        mean: mu,
        sample_sd: sd,
        mean_ci,
        mean_bootstrap_ci,
        met,
        proportion,
        proportion_ci,
        per_question: pq,
        per_question_ci: pq_ci,
        level,
        eamu: eamu_v,
    }
}

/// The deciding statistic, its CI and the verdict for a method.
fn decide(
    stats: &ClassStats,
    method: ClassMethod,
    target: &Exact,
    policy: &AttainmentPolicy,
) -> (Option<Exact>, Option<(f64, f64)>, Option<bool>) {
    let (value, ci) = match method {
        ClassMethod::Average => (stats.mean, stats.mean_ci),
        ClassMethod::ThresholdProportion => (stats.proportion, stats.proportion_ci),
        ClassMethod::PerQuestion => (stats.per_question, stats.per_question_ci),
        ClassMethod::Levels => (stats.level.map(|l| Exact::from_integer(l.into())), None),
        ClassMethod::Eamu => (stats.eamu.as_ref().map(|e| e.avg5), None),
    };
    let attained = match method {
        ClassMethod::Levels => stats.level.map(|l| l >= policy.target_level),
        ClassMethod::Eamu => stats.eamu.as_ref().map(|e| e.flag != FcarFlag::Red),
        _ => value.map(|v| match (policy.verdict_basis, ci) {
            (VerdictBasis::CiLower, Some((lo, _))) => lo >= f(target),
            _ => v >= *target,
        }),
    };
    (value, ci, attained)
}

/// Report group key (§11.3 "Across sections").
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GroupKey {
    /// All sections pooled (each student once, in the section of record).
    Pooled,
    /// One section.
    Section(String),
    /// One campus or branch.
    Campus(String),
    /// One track.
    Track(Track),
}

/// One CLO's result for one report group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupResult {
    /// Group.
    pub key: GroupKey,
    /// CLO code.
    pub clo: String,
    /// Statistics.
    pub stats: ClassStats,
    /// Deciding value.
    pub value: Option<Exact>,
    /// Verdict.
    pub attained: Option<bool>,
    /// n < `suppress_n`: shown as "< 5" in outputs shared beyond the instructor.
    pub suppressed: bool,
}

/// One CLO's offering-level attainment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloAttainment {
    /// CLO code.
    pub clo: String,
    /// NQF domain.
    pub domain: Domain,
    /// Deciding method.
    pub method: ClassMethod,
    /// τ_c.
    pub tau: Exact,
    /// T_c.
    pub target: Exact,
    /// Pooled statistics (or mean of sections, per policy). Diagnostic only while
    /// [`AttainmentResult::pooled_blocked`]: report the per-section groups instead.
    pub stats: ClassStats,
    /// Direct value of the deciding method (`None` while pooling is blocked by SCO-030).
    pub direct: Option<Exact>,
    /// Indirect share, if any survey maps to the CLO.
    pub indirect: Option<Exact>,
    /// A_c = β·Direct + (1 − β)·Indirect when blended; else Direct (`None` while pooling is
    /// blocked by SCO-030).
    pub value: Option<Exact>,
    /// Verdict on A_c (`None` while pooling is blocked by SCO-030).
    pub attained: Option<bool>,
    /// The CI of the deciding statistic contains T_c.
    pub indistinguishable: bool,
    /// No (non-voided) evidence: "not assessed", never 0 % (SCO-012).
    pub not_assessed: bool,
    /// Provisional (SCO-033).
    pub provisional: bool,
    /// Thin evidence (SCO-014).
    pub thin: bool,
    /// N_c < small_n (SCO-023 `clo_small_n`).
    pub small_n: bool,
    /// Per report group.
    pub groups: Vec<GroupResult>,
}

/// Provenance of an [`AttainmentResult`] (§11.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Provenance {
    /// Score-run IDs of every contributing assessment.
    pub run_ids: Vec<String>,
    /// Mapping-set version.
    pub mapping_version: u32,
    /// Attainment-policy hash.
    pub policy_hash: String,
    /// CLO-set version.
    pub clo_set_version: u32,
}

/// `AttainmentResult(scope, CLO, values, CI, hashes)` for one offering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttainmentResult {
    /// Provenance.
    pub provenance: Provenance,
    /// Per student and CLO.
    pub students: Vec<StudentClo>,
    /// Per CLO.
    pub clos: Vec<CloAttainment>,
    /// Findings (SCO-012, SCO-014, SCO-023, SCO-030, SCO-033, SCO-035 …).
    pub exceptions: Vec<Exception>,
    /// Pooled results blocked by version skew (SCO-030).
    pub pooled_blocked: bool,
}

impl AttainmentResult {
    /// The CLO result by code.
    pub fn clo(&self, code: &str) -> Option<&CloAttainment> {
        self.clos.iter().find(|c| c.clo == code)
    }

    /// P_{n,c} of one student.
    pub fn p(&self, student: &str, clo: &str) -> Option<Exact> {
        self.students
            .iter()
            .find(|s| s.student.0 == student && s.clo == clo)
            .and_then(|s| s.p)
    }
}

/// (item, w·L, M_i·d, bonus) rows of the integer path.
type IntRows = Vec<(usize, i128, i128, bool)>;

/// Per (assessment, CLO): the mapped rows and, when integer accumulation is possible, the
/// rows scaled to integers plus the factor g_a (see [`student_performance`]).
struct Prep<'a> {
    a: &'a crate::evidence::AssessmentEvidence,
    a_idx: usize,
    /// (item, w, M_i, bonus).
    rows: Vec<(usize, Exact, Exact, bool)>,
    /// (item, w·L, M_i·d, bonus) and the common score denominator d.
    int_rows: Option<(IntRows, i128)>,
    /// f_a = ω_a / Σ_i w·M_i scaled: grade-consistent W_a/(M_a·L·d), points-pooled 1/(L·d).
    f: Option<Exact>,
}

/// s·d as an integer when d is a multiple of the score's denominator (no division for
/// integer scores, the common case; `d` ≤ i64::MAX by construction).
#[inline]
fn scale_score(score: &omr_scoring::Points, d: i128) -> Option<i128> {
    let (n, den) = (i128::from(*score.numer()), *score.denom());
    if den == 1 {
        return Some(n * d);
    }
    let d64 = i64::try_from(d).ok()?;
    (d64 % den == 0).then(|| n * i128::from(d64 / den))
}

fn to_int(x: &Exact, scale: i128) -> Option<i128> {
    let (n, d) = (*x.numer(), *x.denom());
    if scale % d != 0 {
        return None;
    }
    n.checked_mul(scale / d)
}

/// Per-student P_{n,c} for every CLO of `clos` (§11.3 formulas and missing-data table).
///
/// For the grade-consistent and points-pooled methods, P_{n,c} = Σ_a f_a·num_a / Σ_a f_a·den_a
/// with num_a = Σ (w·L)·(s⁺·d) and den_a = Σ (w·L)·(M_i·d) accumulated in `i128` over a
/// common denominator, so a 10k × 100 recompute stays in milliseconds; any overflow, and the
/// assessment-equal and custom methods, use exact rational arithmetic instead. Both paths
/// give the same exact value.
pub fn student_performance(
    offering: &Offering,
    clos: &CloSet,
    mapping: &MappingSet,
    policy: &AttainmentPolicy,
) -> Vec<StudentClo> {
    let roster = offering.roster_map();
    let mut students: Vec<StudentKey> = offering.roster.iter().map(|r| r.student.clone()).collect();
    if students.is_empty() {
        let set: BTreeSet<StudentKey> = offering
            .assessments
            .iter()
            .flat_map(|a| a.students.iter().map(|s| s.student.clone()))
            .collect();
        students = set.into_iter().collect();
    }
    let index: BTreeMap<&StudentKey, usize> =
        students.iter().enumerate().map(|(k, s)| (s, k)).collect();
    let assessments: Vec<_> = offering
        .assessments
        .iter()
        .filter(|a| a.direct && !a.programme_level)
        .collect();
    let row_index: Vec<Vec<Option<usize>>> = assessments
        .iter()
        .map(|a| {
            a.students
                .iter()
                .map(|s| index.get(&s.student).copied())
                .collect()
        })
        .collect();
    let denoms: Vec<Option<i128>> = assessments.iter().map(|a| a.common_denom()).collect();
    let n_students = students.len();
    let meta: Vec<(String, Option<bool>, RosterStatus)> = students
        .iter()
        .map(|st| {
            let r = roster.get(st);
            (
                r.map(|r| r.section.clone()).unwrap_or_default(),
                r.and_then(|r| r.passing),
                r.map(|r| r.status).unwrap_or_default(),
            )
        })
        .collect();
    let fast_method = matches!(
        policy.student_method,
        StudentMethod::GradeConsistent | StudentMethod::PointsPooled
    );
    let mut out = Vec::with_capacity(n_students * clos.clos.len());
    for clo in clos.clos.iter().filter(|c| !c.retired) {
        // Planned ω per distinct assessment ID.
        let mut planned_total = Exact::zero();
        let mut seen = BTreeSet::new();
        for a in &assessments {
            if !seen.insert(a.id.as_str()) {
                continue;
            }
            let den: Exact = mapping
                .rows_for(&a.id, &clo.code)
                .filter_map(|r| {
                    let it = a.items.get(r.item)?;
                    let bonus_ok =
                        !it.bonus || policy.bonus_items_as_evidence == BonusEvidence::All;
                    (!it.voided && bonus_ok).then(|| widen(&r.weight) * widen(&it.max))
                })
                .sum();
            if !den.is_zero() {
                planned_total +=
                    omega(policy, a.weight, &den, &a.max_non_bonus(), &a.id, &clo.code);
            }
        }
        // Prepare each assessment's rows.
        let mut preps: Vec<Prep> = Vec::new();
        for (a_idx, a) in assessments.iter().enumerate() {
            let rows: Vec<(usize, Exact, Exact, bool)> = mapping
                .rows_for(&a.id, &clo.code)
                .filter_map(|r| {
                    let it = a.items.get(r.item)?;
                    let ok = !it.voided
                        && (!it.bonus || policy.bonus_items_as_evidence != BonusEvidence::Never);
                    ok.then_some((r.item, widen(&r.weight), widen(&it.max), it.bonus))
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            let int_rows = denoms[a_idx].and_then(|d| {
                let l = rows.iter().try_fold(1i128, |acc, (_, w, _, _)| {
                    omr_scoring::num::lcm_i128(acc, *w.denom())
                })?;
                let v = rows
                    .iter()
                    .map(|(i, w, m, b)| Some((*i, to_int(w, l)?, to_int(m, d)?, *b)))
                    .collect::<Option<Vec<_>>>()?;
                let ld = Exact::from_integer(l.checked_mul(d)?);
                Some((v, d, ld))
            });
            let (int_rows, f) = match int_rows {
                Some((v, d, ld)) if fast_method => {
                    let f = match policy.student_method {
                        StudentMethod::GradeConsistent => {
                            let ma = a.max_non_bonus();
                            (!ma.is_zero()).then(|| a.weight / (ma * ld))
                        }
                        _ => Some(Exact::from_integer(1) / ld),
                    };
                    (Some((v, d)), f)
                }
                _ => (None, None),
            };
            preps.push(Prep {
                a,
                a_idx,
                rows,
                int_rows,
                f,
            });
        }
        // Integer factors g_a = f_a·F with F the LCM of the f_a denominators.
        let big_f = preps.iter().try_fold(1i128, |acc, p| {
            p.f.and_then(|f| omr_scoring::num::lcm_i128(acc, *f.denom()))
        });
        let g: Vec<Option<i128>> = preps
            .iter()
            .map(|p| {
                let (f, bf) = (p.f?, big_f?);
                f.numer().checked_mul(bf / f.denom())
            })
            .collect();
        let mut num_int = vec![0i128; n_students];
        let mut den_int = vec![0i128; n_students];
        let mut fast_ok = vec![true; n_students];
        let mut slow: Vec<Vec<(Exact, Exact)>> = vec![Vec::new(); n_students];
        let mut any = vec![false; n_students];
        let mut provisional = vec![false; n_students];
        for (p, g_a) in preps.iter().zip(&g) {
            let ma = p.a.max_non_bonus();
            for (s, idx) in p.a.students.iter().zip(&row_index[p.a_idx]) {
                let Some(n) = *idx else { continue };
                if s.status == EvidenceStatus::Excused {
                    continue;
                }
                if s.provisional {
                    provisional[n] = true;
                }
                let attempted_ok = |bonus: bool, attempted: bool| {
                    !bonus
                        || policy.bonus_items_as_evidence != BonusEvidence::AttemptedOnly
                        || attempted
                };
                // Integer path.
                if let (Some((rows, d)), Some(g_a)) = (&p.int_rows, g_a)
                    && fast_ok[n]
                {
                    let mut ok = true;
                    let (mut na, mut da) = (0i128, 0i128);
                    for (i, w, m, bonus) in rows {
                        if let Some(ItemEvidence::Counted { score, attempted }) = s.items.get(*i) {
                            if !attempted_ok(*bonus, *attempted) {
                                continue;
                            }
                            let sc = scale_score(score, *d);
                            match sc.and_then(|sc| w.checked_mul(sc)) {
                                Some(v) => na += v,
                                None => ok = false,
                            }
                            da += w * m;
                        }
                    }
                    if !ok {
                        fast_ok[n] = false;
                    } else if da != 0 {
                        match (g_a.checked_mul(na), g_a.checked_mul(da)) {
                            (Some(x), Some(y)) => {
                                match (num_int[n].checked_add(x), den_int[n].checked_add(y)) {
                                    (Some(a1), Some(b1)) => {
                                        num_int[n] = a1;
                                        den_int[n] = b1;
                                        any[n] = true;
                                    }
                                    _ => fast_ok[n] = false,
                                }
                            }
                            _ => fast_ok[n] = false,
                        }
                    }
                }
                // Exact rational path (always kept for the slow methods and as fallback).
                if p.int_rows.is_none() || g_a.is_none() || !fast_ok[n] {
                    let (mut num, mut den) = (Exact::zero(), Exact::zero());
                    for (i, w, m, bonus) in &p.rows {
                        if let Some(ItemEvidence::Counted { score, attempted }) = s.items.get(*i) {
                            if !attempted_ok(*bonus, *attempted) {
                                continue;
                            }
                            num += w * widen(score);
                            den += w * m;
                        }
                    }
                    if !den.is_zero() {
                        let om = omega(policy, p.a.weight, &den, &ma, &p.a.id, &clo.code);
                        slow[n].push((om, om * num / den));
                        any[n] = true;
                    }
                }
            }
        }
        for (n, st) in students.iter().enumerate() {
            let (section, passing, status) = &meta[n];
            let (p, coverage) = if !any[n] {
                (None, Exact::zero())
            } else if fast_ok[n] && slow[n].is_empty() {
                // Σω = den_int/F, so coverage = den_int·q_planned / (F·p_planned).
                let bf = big_f.unwrap_or(1);
                let cov = den_int[n]
                    .checked_mul(*planned_total.denom())
                    .zip(bf.checked_mul(*planned_total.numer()))
                    .filter(|(_, b)| *b != 0)
                    .map_or_else(
                        || {
                            if planned_total.is_zero() {
                                Exact::zero()
                            } else {
                                reduce(den_int[n], bf) / planned_total
                            }
                        },
                        |(a, b)| reduce(a, b),
                    );
                (
                    (den_int[n] != 0).then(|| reduce(num_int[n], den_int[n])),
                    cov,
                )
            } else {
                // Fall back to exact rationals for the whole student (recompute fully).
                let (sw, swp) = exact_student(&preps, &row_index, n, policy, &clo.code);
                let cov = if planned_total.is_zero() {
                    Exact::zero()
                } else {
                    sw / planned_total
                };
                ((!sw.is_zero()).then(|| swp / sw), cov)
            };
            let excluded_reason = match status {
                RosterStatus::Withdrawn => Some("withdrawn".to_string()),
                RosterStatus::Denied => Some("denied entry (DN)".to_string()),
                RosterStatus::Exempted => Some("exempted".to_string()),
                _ if p.is_none() => Some("no evidence".to_string()),
                _ if cmp_exact(&coverage, &policy.min_evidence_share) == Less => Some(format!(
                    "coverage {:.2} below min_evidence_share",
                    f(&coverage)
                )),
                _ => None,
            };
            out.push(StudentClo {
                student: st.clone(),
                section: section.clone(),
                clo: clo.code.clone(),
                p,
                coverage,
                included: excluded_reason.is_none(),
                excluded_reason,
                provisional: provisional[n],
                passing: *passing,
            });
        }
    }
    out
}

/// Exact rational Σ ω and Σ ω·P for one student over all prepared assessments.
fn exact_student(
    preps: &[Prep],
    row_index: &[Vec<Option<usize>>],
    n: usize,
    policy: &AttainmentPolicy,
    clo: &str,
) -> (Exact, Exact) {
    let (mut sw, mut swp) = (Exact::zero(), Exact::zero());
    for p in preps {
        let ma = p.a.max_non_bonus();
        for (s, idx) in p.a.students.iter().zip(&row_index[p.a_idx]) {
            if *idx != Some(n) || s.status == EvidenceStatus::Excused {
                continue;
            }
            let (mut num, mut den) = (Exact::zero(), Exact::zero());
            for (i, w, m, bonus) in &p.rows {
                if let Some(ItemEvidence::Counted { score, attempted }) = s.items.get(*i) {
                    if *bonus
                        && policy.bonus_items_as_evidence == BonusEvidence::AttemptedOnly
                        && !attempted
                    {
                        continue;
                    }
                    num += w * widen(score);
                    den += w * m;
                }
            }
            if !den.is_zero() {
                let om = omega(policy, p.a.weight, &den, &ma, &p.a.id, clo);
                sw += om;
                swp += om * num / den;
            }
        }
    }
    (sw, swp)
}

/// ω_{a,c} under the policy's student method (§11.3 table).
fn omega(
    policy: &AttainmentPolicy,
    w_a: Exact,
    den: &Exact,
    m_a: &Exact,
    assessment: &str,
    clo: &str,
) -> Exact {
    match &policy.student_method {
        StudentMethod::GradeConsistent => {
            if m_a.is_zero() {
                Exact::zero()
            } else {
                w_a * den / m_a
            }
        }
        StudentMethod::PointsPooled => *den,
        StudentMethod::AssessmentEqual => Exact::from_integer(1),
        StudentMethod::Custom(v) => v
            .get(&(assessment.to_string(), clo.to_string()))
            .copied()
            .unwrap_or_default(),
    }
}

/// Per-question flags for the included students of one CLO (rows follow `students`).
fn per_question_data(
    offering: &Offering,
    mapping: &MappingSet,
    clo: &str,
    tau: &Exact,
    n_rows: usize,
    evidence_rows: &[Vec<Option<usize>>],
    row_of_global: &[Option<usize>],
) -> PerQuestionData {
    let ids: BTreeSet<&str> = offering
        .assessments
        .iter()
        .filter(|a| a.direct && !a.programme_level)
        .map(|a| a.id.as_str())
        .collect();
    // Columns: (assessment ID, item, w, M_i).
    let mut cols: Vec<(&str, usize, Exact, Exact)> = Vec::new();
    for id in &ids {
        let Some(first) = offering.assessments.iter().find(|a| a.id == *id) else {
            continue;
        };
        for r in mapping.rows_for(id, clo) {
            if let Some(it) = first.items.get(r.item).filter(|it| !it.voided) {
                cols.push((id, r.item, widen(&r.weight), widen(&it.max)));
            }
        }
    }
    let k = cols.len();
    let mut cells = vec![0u8; n_rows * k];
    for (a, ev_rows) in offering.assessments.iter().zip(evidence_rows) {
        if !ids.contains(a.id.as_str()) {
            continue;
        }
        let my_cols: Vec<(usize, usize, Exact)> = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| c.0 == a.id)
            .map(|(j, c)| (j, c.1, tau * c.3))
            .collect();
        if my_cols.is_empty() {
            continue;
        }
        for (s, g) in a.students.iter().zip(ev_rows) {
            let Some(n) = g.and_then(|g| row_of_global.get(g).copied().flatten()) else {
                continue;
            };
            if s.status == EvidenceStatus::Excused {
                continue;
            }
            for (j, item, thr) in &my_cols {
                if let Some(ItemEvidence::Counted { score, .. }) = s.items.get(*item) {
                    cells[n * k + j] = if cmp_exact(&widen(score), thr) != Less {
                        2
                    } else {
                        1
                    };
                }
            }
        }
    }
    PerQuestionData {
        weights: cols.iter().map(|c| c.2).collect(),
        cells,
    }
}

/// Computes the offering's attainment result (the §11 pure function).
pub fn compute_attainment(
    offering: &Offering,
    clos: &CloSet,
    mapping: &MappingSet,
    policy: &AttainmentPolicy,
    indirect: &[IndirectEvidence],
) -> AttainmentResult {
    let mut exceptions = policy.validate(!indirect.is_empty());
    let students = student_performance(offering, clos, mapping, policy);
    // SCO-030: one assessment scored under different key/policy versions across sections.
    let mut versions: BTreeMap<&str, BTreeSet<(u32, u32)>> = BTreeMap::new();
    for a in &offering.assessments {
        versions
            .entry(&a.id)
            .or_default()
            .insert((a.key_version, a.policy_version));
    }
    let mut pooled_blocked = false;
    for (id, v) in &versions {
        if v.len() > 1 {
            pooled_blocked = true;
            exceptions.push(
                Exception::must("SCO-030", Severity::Warn, Scope::OutcomeReport).with_detail(
                    format!("assessment {id} scored under {} key/policy versions across sections: pooled result blocked", v.len()),
                ),
            );
        }
    }
    let run_ids: Vec<String> = offering
        .assessments
        .iter()
        .map(|a| a.run_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let seed_base = run_ids.join(",");
    // Index arrays resolved once: evidence row → global student index (the per-CLO order of
    // `students`), and section → report-group indices.
    let live: Vec<_> = clos.clos.iter().filter(|c| !c.retired).collect();
    let n_global = if live.is_empty() {
        0
    } else {
        students.len() / live.len()
    };
    let global: BTreeMap<&StudentKey, usize> = students[..n_global]
        .iter()
        .enumerate()
        .map(|(n, s)| (&s.student, n))
        .collect();
    let evidence_rows: Vec<Vec<Option<usize>>> = offering
        .assessments
        .iter()
        .map(|a| {
            a.students
                .iter()
                .map(|s| global.get(&s.student).copied())
                .collect()
        })
        .collect();
    let mut group_index: BTreeMap<GroupKey, usize> = BTreeMap::new();
    group_index.insert(GroupKey::Pooled, 0);
    let mut section_groups: BTreeMap<&str, Vec<GroupKey>> = BTreeMap::new();
    for s in &students[..n_global] {
        section_groups.entry(s.section.as_str()).or_insert_with(|| {
            let mut keys = Vec::new();
            if policy.report_groups.section {
                keys.push(GroupKey::Section(s.section.clone()));
            }
            if let Some(sec) = offering.section(&s.section) {
                if policy.report_groups.campus {
                    keys.push(GroupKey::Campus(sec.campus.clone()));
                }
                if policy.report_groups.track {
                    keys.push(GroupKey::Track(sec.track));
                }
            }
            keys
        });
    }
    for keys in section_groups.values() {
        for k in keys {
            let next = group_index.len();
            group_index.entry(k.clone()).or_insert(next);
        }
    }
    let section_group_idx: BTreeMap<&str, Vec<usize>> = section_groups
        .iter()
        .map(|(s, keys)| (*s, keys.iter().map(|k| group_index[k]).collect()))
        .collect();
    let mut out_clos = Vec::new();
    for (ci, clo) in live.iter().enumerate() {
        let tau = policy.tau_for(&clo.code);
        let target = policy.target_for(&clo.code);
        let slice = &students[ci * n_global..(ci + 1) * n_global];
        let mut row_of_global: Vec<Option<usize>> = vec![None; n_global];
        let mut rows: Vec<&StudentClo> = Vec::new();
        for (g, s) in slice.iter().enumerate() {
            if s.included {
                row_of_global[g] = Some(rows.len());
                rows.push(s);
            }
        }
        let assessed = mapping.rows.iter().any(|r| {
            r.clo == clo.code
                && r.weight > omr_scoring::Points::zero()
                && r.status != crate::mapping::MappingStatus::Retired
                && offering
                    .assessments
                    .iter()
                    .find(|a| a.id == r.assessment)
                    .and_then(|a| a.items.get(r.item))
                    .is_some_and(|it| !it.voided)
        });
        let pq = per_question_data(
            offering,
            mapping,
            &clo.code,
            &tau,
            rows.len(),
            &evidence_rows,
            &row_of_global,
        );
        let mut rng = SplitMix64::from_run(&seed_base, &format!("clo:{}", clo.code));
        let stats_for = |subset: &[usize], rng: &mut SplitMix64| {
            let vals: Vec<Exact> = subset.iter().filter_map(|&k| rows[k].p).collect();
            let pass: Vec<Option<bool>> = subset.iter().map(|&k| rows[k].passing).collect();
            class_stats(&vals, &pass, &tau, policy, Some((&pq, subset)), rng)
        };
        let all: Vec<usize> = (0..rows.len()).collect();
        let mut stats = stats_for(&all, &mut rng);
        // Groups (pooled first, then sections, campuses, tracks in key order).
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); group_index.len()];
        for (k, s) in rows.iter().enumerate() {
            for &gi in section_group_idx
                .get(s.section.as_str())
                .into_iter()
                .flatten()
            {
                members[gi].push(k);
            }
        }
        let mut groups = Vec::new();
        for (key, &gi) in &group_index {
            if gi != 0 && members[gi].is_empty() {
                continue;
            }
            let st = if gi == 0 {
                stats.clone()
            } else {
                stats_for(&members[gi], &mut rng)
            };
            let (value, _, attained) = decide(&st, policy.class_method, &target, policy);
            groups.push(GroupResult {
                key: key.clone(),
                clo: clo.code.clone(),
                suppressed: st.n < policy.suppress_n,
                stats: st,
                value,
                attained,
            });
        }
        if policy.section_aggregation == SectionAggregation::MeanOfSections {
            // Opt-in: mean of section rates (Simpson's-paradox warning in reports).
            let rates: Vec<Exact> = groups
                .iter()
                .filter(|g| matches!(g.key, GroupKey::Section(_)))
                .filter_map(|g| g.value)
                .collect();
            if let Some(m) = mean(&rates) {
                match policy.class_method {
                    ClassMethod::Average => stats.mean = Some(m),
                    ClassMethod::ThresholdProportion => stats.proportion = Some(m),
                    ClassMethod::PerQuestion => stats.per_question = Some(m),
                    _ => {}
                }
            }
        }
        let (direct, ci, _) = decide(&stats, policy.class_method, &target, policy);
        let ind = indirect
            .iter()
            .find(|i| i.clo == clo.code && i.respondents > 0);
        let indirect_v = ind.map(|i| Exact::new(i.at_or_above.into(), i.respondents.into()));
        let one = Exact::from_integer(1);
        let value = match (direct, indirect_v) {
            (Some(d), Some(i))
                if policy.blend_allowed
                    && policy.beta < one
                    && policy.class_method.scale() == Scale::Proportion =>
            {
                Some(policy.beta * d + (one - policy.beta) * i)
            }
            (d, _) => d,
        };
        let attained = match policy.class_method {
            ClassMethod::Levels | ClassMethod::Eamu => {
                decide(&stats, policy.class_method, &target, policy).2
            }
            _ => value.map(|v| match (policy.verdict_basis, ci) {
                (VerdictBasis::CiLower, Some((lo, _))) if indirect_v.is_none() => lo >= f(&target),
                _ => v >= target,
            }),
        };
        let indistinguishable = ci.is_some_and(|(lo, hi)| lo <= f(&target) && f(&target) <= hi);
        let provisional = rows.iter().any(|s| s.provisional);
        let small_n = stats.n < policy.small_n;
        // Thin evidence: < 3 items, < 10 % of the course, or a single assessment.
        // Live (non-retired) rows on non-voided items, each item once.
        let n_items = mapping
            .rows
            .iter()
            .filter(|r| {
                r.clo == clo.code
                    && r.weight > omr_scoring::Points::zero()
                    && r.status != crate::mapping::MappingStatus::Retired
                    && offering
                        .assessments
                        .iter()
                        .find(|a| a.id == r.assessment)
                        .and_then(|a| a.items.get(r.item))
                        .is_some_and(|it| !it.voided)
            })
            .map(|r| (r.assessment.as_str(), r.item))
            .collect::<BTreeSet<_>>()
            .len();
        let n_assess = mapping.assessments_for(&clo.code).len();
        let share = evidence_share(mapping, offering, &clo.code);
        let thin = assessed
            && (n_items < policy.thin_evidence.items
                || share < policy.thin_evidence.course_share
                || n_assess < policy.thin_evidence.assessments);
        let rep = |code: &str, sev: Severity| Exception::must(code, sev, Scope::OutcomeReport);
        if !assessed {
            exceptions.push(rep("SCO-012", Severity::Error).with_detail(format!(
                "{}: not assessed (no evidence or all voided); never reported as 0 %",
                clo.code
            )));
        } else {
            if thin {
                exceptions.push(rep("SCO-014", Severity::Warn).with_detail(format!(
                    "{}: thin evidence ({n_items} items, {n_assess} assessments, {:.1} % of the course)",
                    clo.code,
                    f(&share) * 100.0
                )));
            }
            if small_n {
                exceptions.push(
                    rep("SCO-023", Severity::Info)
                        .with_sub_reason("clo_small_n")
                        .with_detail(format!("{}: N_c = {}", clo.code, stats.n)),
                );
            }
            if provisional {
                exceptions.push(rep("SCO-033", Severity::Info).with_detail(format!(
                    "{}: provisional (pending responses, incomplete or make-up pending)",
                    clo.code
                )));
            }
        }
        // The offering-level value is the pooled result: withheld while SCO-030 blocks
        // pooling (§11.3, §11.8); per-section groups stay available.
        let publish = assessed && !pooled_blocked;
        out_clos.push(CloAttainment {
            clo: clo.code.clone(),
            domain: clo.domain,
            method: policy.class_method,
            tau,
            target,
            stats,
            direct: if publish { direct } else { None },
            indirect: indirect_v,
            value: if publish { value } else { None },
            attained: if publish { attained } else { None },
            indistinguishable: publish && indistinguishable,
            not_assessed: !assessed,
            provisional,
            thin,
            small_n,
            groups: if pooled_blocked {
                groups
                    .into_iter()
                    .filter(|g| g.key != GroupKey::Pooled)
                    .collect()
            } else {
                groups
            },
        });
    }
    AttainmentResult {
        provenance: Provenance {
            run_ids,
            mapping_version: mapping.version,
            policy_hash: policy.hash(),
            clo_set_version: clos.version,
        },
        students,
        clos: out_clos,
        exceptions,
        pooled_blocked,
    }
}

/// Pools sections that were computed separately (e.g. `.omrres` snapshots from two campus
/// PCs): π = Σ x_s / Σ N_s; each student counts once (§11.3 "Across sections").
pub fn pool_proportions(sections: &[(usize, usize)]) -> Option<(Exact, (f64, f64))> {
    let x: usize = sections.iter().map(|s| s.0).sum();
    let n: usize = sections.iter().map(|s| s.1).sum();
    (n > 0).then(|| {
        (
            Exact::new(x as i128, n as i128),
            wilson_ci(x as u64, n as u64, Z95),
        )
    })
}

/// The opt-in mean of section rates (with a Simpson's-paradox warning).
pub fn mean_of_section_rates(sections: &[(usize, usize)]) -> Option<Exact> {
    let rates: Vec<Exact> = sections
        .iter()
        .filter(|s| s.1 > 0)
        .map(|s| Exact::new(s.0 as i128, s.1 as i128))
        .collect();
    mean(&rates)
}

/// Shared-output cell: the count, or "< n" when suppressed (§11.3, §12.3).
pub fn suppress_count(n: usize, suppress_n: usize) -> String {
    if n < suppress_n {
        format!("< {suppress_n}")
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sum_and_eamu_edges() {
        let v = [exact(1, 3), exact(1, 3), exact(1, 6), exact(1, 6)];
        assert_eq!(sum_exact(&v), exact(1, 1));
        // P exactly at a band edge falls in the upper band (≥).
        let p = AttainmentPolicy::default();
        let m = eamu(
            &[exact(9, 10), exact(3, 4), exact(3, 5), exact(59, 100)],
            &p.eamu_bands,
            &p.fcar_flags,
        );
        assert_eq!((m.e, m.a, m.m, m.u), (1, 1, 1, 1));
        assert_eq!(suppress_count(4, 5), "< 5");
        assert_eq!(suppress_count(5, 5), "5");
        assert_eq!(pool_proportions(&[]), None);
    }
}
