//! TP-154 Course Report tables (spec §11.7): B.1 CLO rows grouped by NQF domain with the
//! targeted and actual levels, and A.1 grade distribution with the status columns.
//! The DOCX filling of the official blank is deferred (docs/IMPLEMENTATION.md §2); these are
//! the data rows `omr-report` renders to XLSX/CSV/HTML.

use crate::attainment::AttainmentResult;
use crate::evidence::Offering;
use crate::framework::{CloPloMap, CloSet, Domain};
use crate::mapping::{MappingSet, MappingStatus};
use crate::policy::{ActualLevelDisplay, AttainmentPolicy, ClassMethod};
use num_traits::Zero;
use omr_scoring::{A1Column, Exact, GradeOutcome, GradeScale, StatusCode, decimal_string};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Formats a proportion as a percentage with one decimal, e.g. `72.0%`.
pub fn pct(x: &Exact) -> String {
    format!(
        "{}%",
        decimal_string(&(x * Exact::from_integer(100)), 1, Default::default())
    )
}

/// One TP-154 B.1 row.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct B1Row {
    /// Row number, e.g. `1.2`.
    pub number: String,
    /// CLO code.
    pub clo: String,
    /// CLO text (programme language).
    pub clo_text: String,
    /// Related PLO codes.
    pub plo_codes: Vec<String>,
    /// Generated assessment methods, e.g. `Midterm Q3–5; Final Q12–20`.
    pub methods: String,
    /// Targeted level (T_c), e.g. `70.0%`.
    pub targeted: String,
    /// Average μ_c, when shown.
    pub actual_average: Option<String>,
    /// Threshold-proportion π_c, when shown.
    pub actual_proportion: Option<String>,
    /// The printed "Actual level" cell.
    pub actual: String,
    /// Verdict on the deciding method.
    pub attained: Option<bool>,
    /// Comment (faculty narrative; prefilled skeleton).
    pub comment: String,
    /// "Not attained" makes the comment (and a B.2 recommendation) required.
    pub comment_required: bool,
}

/// One NQF domain block of B.1 (1.0 Knowledge…, 2.0 Skills…, 3.0 Values…).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct B1Group {
    /// Domain.
    pub domain: Domain,
    /// Group number, e.g. `1.0`.
    pub number: String,
    /// English heading.
    pub heading_en: String,
    /// Arabic heading.
    pub heading_ar: String,
    /// Rows.
    pub rows: Vec<B1Row>,
}

/// The TP-154 B.1 table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct B1Table {
    /// Domain groups in NQF order (empty domains included, as on the blank).
    pub groups: Vec<B1Group>,
    /// Footnote naming the deciding method, τ and T.
    pub footnote: String,
    /// Draft-mappings watermark (SCO-032 open).
    pub draft_watermark: bool,
}

/// Compresses item labels into ranges: `Q3, Q4, Q5, Q9` → `Q3–5, Q9`.
pub fn compress_labels(labels: &[String]) -> String {
    let split = |l: &str| {
        let digits = l.trim_start_matches(|c: char| !c.is_ascii_digit());
        let prefix = &l[..l.len() - digits.len()];
        digits.parse::<u32>().ok().map(|n| (prefix.to_string(), n))
    };
    let mut parts: Vec<String> = Vec::new();
    let mut run: Option<(String, u32, u32)> = None;
    let flush = |run: &mut Option<(String, u32, u32)>, parts: &mut Vec<String>| {
        if let Some((p, a, b)) = run.take() {
            parts.push(if a == b {
                format!("{p}{a}")
            } else {
                format!("{p}{a}–{b}")
            });
        }
    };
    for l in labels {
        match split(l) {
            Some((p, n)) => match &mut run {
                Some((rp, _, b)) if *rp == p && *b + 1 == n => *b = n,
                _ => {
                    flush(&mut run, &mut parts);
                    run = Some((p, n, n));
                }
            },
            None => {
                flush(&mut run, &mut parts);
                parts.push(l.clone());
            }
        }
    }
    flush(&mut run, &mut parts);
    parts.join(", ")
}

/// Generated "assessment methods" text for a CLO: `Midterm Q3–5; Final Q12–20`.
pub fn assessment_methods(offering: &Offering, mapping: &MappingSet, clo: &str) -> String {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for a in &offering.assessments {
        if seen.contains(&a.id) {
            continue;
        }
        seen.push(a.id.clone());
        let mut items: Vec<usize> = mapping.rows_for(&a.id, clo).map(|r| r.item).collect();
        items.sort_unstable();
        items.dedup();
        let labels: Vec<String> = items
            .iter()
            .filter_map(|&i| {
                a.items
                    .get(i)
                    .filter(|it| !it.voided)
                    .map(|it| it.label.clone())
            })
            .collect();
        if !labels.is_empty() {
            out.push(format!("{} {}", a.name, compress_labels(&labels)));
        }
    }
    out.join("; ")
}

/// Builds TP-154 B.1 (§11.7): per CLO, grouped by NQF domain: # · CLO · related PLO code ·
/// assessment methods · targeted level · actual level (average **and** threshold-proportion
/// with a footnote, per `actual_level_display`) · comment.
pub fn tp154_b1(
    offering: &Offering,
    clos: &CloSet,
    mapping: &MappingSet,
    clo_plo: &[CloPloMap],
    result: &AttainmentResult,
    policy: &AttainmentPolicy,
    comments: &BTreeMap<String, String>,
) -> B1Table {
    let mut groups: Vec<B1Group> = Domain::ALL
        .iter()
        .map(|d| B1Group {
            domain: *d,
            number: format!("{}.0", d.number()),
            heading_en: d.name_en().into(),
            heading_ar: d.name_ar().into(),
            rows: Vec::new(),
        })
        .collect();
    for c in clos.clos.iter().filter(|c| !c.retired) {
        let Some(r) = result.clo(&c.code) else {
            continue;
        };
        let g = groups
            .iter_mut()
            .find(|g| g.domain == c.domain)
            .expect("every domain has a group");
        let mut plo_codes: Vec<String> = clo_plo
            .iter()
            .filter(|l| l.clo == c.code)
            .map(|l| l.plo.clone())
            .collect();
        plo_codes.sort();
        plo_codes.dedup();
        let show_both = policy.actual_level_display == ActualLevelDisplay::Both;
        // Version skew (SCO-030) blocks the pooled result: no pooled number is printed.
        let blocked = result.pooled_blocked && !r.not_assessed;
        let avg = r.stats.mean.as_ref().map(pct).filter(|_| !blocked);
        let prop = r.stats.proportion.as_ref().map(pct).filter(|_| !blocked);
        let deciding = match (r.method, r.value) {
            (ClassMethod::Levels, Some(v)) => format!("level {}", v.to_integer()),
            (ClassMethod::Eamu, Some(v)) => {
                format!("avg₅ {}", decimal_string(&v, 2, Default::default()))
            }
            (_, Some(v)) => pct(&v),
            (_, None) => String::new(),
        };
        let mut actual = if r.not_assessed {
            "Not assessed".to_string()
        } else if blocked {
            "Pooled result blocked: sections on different key or policy versions (SCO-030); \
             see the per-section results"
                .to_string()
        } else if show_both {
            let mut parts = vec![
                format!("Average {}", avg.clone().unwrap_or_else(|| "—".into())),
                format!(
                    "{} of students ≥ {}",
                    prop.clone().unwrap_or_else(|| "—".into()),
                    pct(&r.tau)
                ),
            ];
            if !matches!(
                r.method,
                ClassMethod::Average | ClassMethod::ThresholdProportion
            ) {
                parts.push(format!("{}: {deciding}", r.method.name()));
            }
            format!("{} *", parts.join("; "))
        } else {
            format!("{deciding} *")
        };
        if r.provisional {
            actual.push_str(" (provisional)");
        }
        let comment_required = r.attained == Some(false);
        let comment = comments.get(&c.code).cloned().unwrap_or_else(|| {
            if comment_required {
                "Not attained: comment and B.2 recommendation required".into()
            } else {
                String::new()
            }
        });
        let k = g.rows.len() + 1;
        g.rows.push(B1Row {
            number: format!("{}.{k}", c.domain.number()),
            clo: c.code.clone(),
            clo_text: c.text_en.clone(),
            plo_codes,
            methods: assessment_methods(offering, mapping, &c.code),
            targeted: pct(&r.target),
            actual_average: if show_both || r.method == ClassMethod::Average {
                avg
            } else {
                None
            },
            actual_proportion: if show_both || r.method == ClassMethod::ThresholdProportion {
                prop
            } else {
                None
            },
            actual,
            attained: r.attained,
            comment,
            comment_required,
        });
    }
    let footnote = format!(
        "* Deciding method: {} (student method {:?}); student threshold τ = {}, target T = {}; \
         policy {} v{}.",
        policy.class_method.name(),
        policy.student_method,
        pct(&policy.tau),
        pct(&policy.target),
        policy.name,
        policy.version
    );
    B1Table {
        groups,
        footnote,
        draft_watermark: mapping
            .rows
            .iter()
            .any(|r| r.status != MappingStatus::Approved && r.status != MappingStatus::Retired),
    }
}

/// One A.1 cell: count and exact percentage of all listed students.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct A1Cell {
    /// English label (letter or status column).
    pub label: String,
    /// Arabic label.
    pub arabic: String,
    /// Count.
    pub count: usize,
    /// 100·count/total, exact.
    pub percent: Exact,
}

/// TP-154 A.1 grade distribution (§11.7): A+ … F plus Denied Entry, In Progress,
/// Incomplete, Pass, Fail, Withdrawn, from L6 (§10.6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct A1Table {
    /// Letter columns, A+ first.
    pub letters: Vec<A1Cell>,
    /// Status columns.
    pub statuses: Vec<A1Cell>,
    /// Students listed (the percentage denominator).
    pub total: usize,
}

/// Builds A.1 from L6 outcomes. DN goes to "Denied Entry" (it counts as F only for GPA);
/// E maps to no column (pending the package mapping) but counts in the total.
pub fn tp154_a1(grades: &[GradeOutcome], scale: &GradeScale) -> A1Table {
    let total = grades.len();
    let percent = |c: usize| {
        if total == 0 {
            Exact::zero()
        } else {
            Exact::new(100 * c as i128, total as i128)
        }
    };
    let letters = scale
        .bands
        .iter()
        .rev()
        .map(|b| {
            let count = grades
                .iter()
                .filter(|g| matches!(g, GradeOutcome::Letter { letter, .. } if *letter == b.letter))
                .count();
            A1Cell {
                label: b.letter.clone(),
                arabic: b.arabic.clone(),
                count,
                percent: percent(count),
            }
        })
        .collect();
    let cols = [
        (A1Column::DeniedEntry, "Denied Entry", StatusCode::DN),
        (A1Column::InProgress, "In Progress", StatusCode::IP),
        (A1Column::Incomplete, "Incomplete", StatusCode::IC),
        (A1Column::Pass, "Pass", StatusCode::NP),
        (A1Column::Fail, "Fail", StatusCode::NF),
        (A1Column::Withdrawn, "Withdrawn", StatusCode::W),
    ];
    let statuses = cols
        .iter()
        .map(|(col, label, code)| {
            let count = grades
                .iter()
                .filter(|g| matches!(g, GradeOutcome::Status(s) if s.a1_column() == Some(*col)))
                .count();
            A1Cell {
                label: (*label).into(),
                arabic: code.arabic().into(),
                count,
                percent: percent(count),
            }
        })
        .collect();
    A1Table {
        letters,
        statuses,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_percentages() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(compress_labels(&l(&["Q1", "Q2", "Q3"])), "Q1–3");
        assert_eq!(
            compress_labels(&l(&["Q1", "Q3", "P4", "P5"])),
            "Q1, Q3, P4–5"
        );
        assert_eq!(pct(&omr_scoring::exact(87, 125)), "69.6%");
        assert_eq!(pct(&omr_scoring::exact(2, 3)), "66.7%");
    }
}
