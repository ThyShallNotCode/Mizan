//! ABET Criterion 3 summary (spec §11.7 "ABET Criterion 3/4 summary"): per student outcome
//! (EAC 1–7, CAC 1–6, custom), the contributing performance indicators (CLO → PLO → SO
//! through the one stored crosswalk, §11.1), their methods, N, targets, results with 95 %
//! intervals and verdicts, the SO roll-up and the actions.
//!
//! Aggregate-only by construction: no student appears, so the export is pseudonymous as §17.6
//! requires for ABET's foreign platform. Groups with N below `suppress_n` print `< n` and no
//! result, interval, verdict or flag (they would disclose individual results, SCO-023); an SO
//! roll-up resting only on such groups is suppressed too.

use crate::error::ReportError;
use crate::meta::ReportMeta;
use crate::table::{Cell, Row, Sheet, write_csv, write_xlsx};
use crate::text::{exact_str, float_str};
use omr_outcomes::{
    AttainmentResult, B1Table, ClassMethod, CloAttainment, CrosswalkRow, FcarFlag, OutcomeFramework,
};
use omr_scoring::Exact;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;

/// One performance indicator (a CLO crosswalked to the SO through a PLO).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbetIndicator {
    /// Course code.
    pub course: String,
    /// Term.
    pub term: String,
    /// CLO code.
    pub clo: String,
    /// PLO code.
    pub plo: String,
    /// CLO → PLO strength (I/P/M) and PLO → SO strength.
    pub strength: String,
    /// Assessment methods ("Midterm Q3–5; Final Q12–20") and the class method.
    pub methods: String,
    /// N_c.
    pub n: usize,
    /// Target T_c.
    pub target: Exact,
    /// Deciding value as printed (percentage, level or avg₅).
    pub result: Option<String>,
    /// 95 % interval of the deciding statistic, in percent.
    pub ci: Option<(f64, f64)>,
    /// Verdict.
    pub attained: Option<bool>,
    /// The CI contains the target ("statistically indistinguishable", §11.3).
    pub indistinguishable: bool,
    /// FCAR flag (EAMU).
    pub fcar: Option<FcarFlag>,
}

/// One student outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbetSoRow {
    /// SO code.
    pub code: String,
    /// SO label (official wording from the versioned framework package).
    pub text: String,
    /// Indicators.
    pub indicators: Vec<AbetIndicator>,
    /// SO roll-up (strength-weighted, §11.5), when available.
    pub result: Option<Exact>,
    /// Institutional target.
    pub target: Exact,
    /// Verdict on the roll-up.
    pub attained: Option<bool>,
    /// Actions (closing the loop, §11.6).
    pub actions: String,
}

/// The Criterion 3 summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbetSummary {
    /// Header and provenance.
    pub meta: ReportMeta,
    /// Framework name and edition.
    pub framework: String,
    /// Small-N suppression threshold.
    pub suppress_n: usize,
    /// Rows in framework order.
    pub rows: Vec<AbetSoRow>,
}

/// One course's inputs.
#[derive(Clone, Copy, Debug)]
pub struct CourseInput<'a> {
    /// Course code.
    pub course: &'a str,
    /// Term.
    pub term: &'a str,
    /// Crosswalk rows of the course (CLO → PLO → SO).
    pub crosswalk: &'a [CrosswalkRow],
    /// CLO attainment of the offering.
    pub attainment: &'a AttainmentResult,
    /// TP-154 B.1 (for the generated assessment-method text).
    pub b1: Option<&'a B1Table>,
}

fn deciding(r: &CloAttainment) -> (Option<String>, Option<(f64, f64)>) {
    let pct = |ci: Option<(f64, f64)>| {
        ci.map(|(a, b)| ((a * 100.0).clamp(0.0, 100.0), (b * 100.0).clamp(0.0, 100.0)))
    };
    match (r.method, r.value) {
        (_, None) => (None, None),
        (ClassMethod::Levels, Some(v)) => (Some(format!("level {}", v.to_integer())), None),
        (ClassMethod::Eamu, Some(v)) => (Some(format!("avg5 {}", exact_str(&v, 2))), None),
        (m, Some(v)) => {
            let ci = match m {
                ClassMethod::Average => r.stats.mean_bootstrap_ci.or(r.stats.mean_ci),
                ClassMethod::ThresholdProportion => r.stats.proportion_ci,
                _ => r.stats.per_question_ci,
            };
            (Some(exact_str(&(v * Exact::from_integer(100)), 1)), pct(ci))
        }
    }
}

impl AbetSummary {
    /// Builds the summary: indicators per SO from every course's crosswalk, the SO roll-ups
    /// (`omr_outcomes::framework_rollup` values) and the institutional target.
    pub fn build(
        meta: ReportMeta,
        framework: &OutcomeFramework,
        courses: &[CourseInput<'_>],
        so_values: &BTreeMap<String, Exact>,
        target: Exact,
        actions: &BTreeMap<String, String>,
        suppress_n: usize,
    ) -> Self {
        let rows = framework
            .outcomes
            .iter()
            .map(|so| {
                let mut indicators = Vec::new();
                for c in courses {
                    let mut seen = Vec::new();
                    for x in c
                        .crosswalk
                        .iter()
                        .filter(|x| x.outcome.as_deref() == Some(so.code.as_str()))
                    {
                        if seen.contains(&(&x.clo, &x.plo)) {
                            continue;
                        }
                        seen.push((&x.clo, &x.plo));
                        let Some(r) = c.attainment.clo(&x.clo) else {
                            continue;
                        };
                        let assessed =
                            c.b1.and_then(|b| {
                                b.groups
                                    .iter()
                                    .flat_map(|g| g.rows.iter())
                                    .find(|row| row.clo == x.clo)
                            })
                            .map(|row| row.methods.clone())
                            .filter(|m| !m.is_empty());
                        let methods = match assessed {
                            Some(m) => format!("{m} ({})", r.method.name()),
                            None => r.method.name().to_owned(),
                        };
                        let (result, ci) = deciding(r);
                        indicators.push(AbetIndicator {
                            course: c.course.into(),
                            term: c.term.into(),
                            clo: x.clo.clone(),
                            plo: x.plo.clone(),
                            strength: format!(
                                "{}{}",
                                x.clo_plo,
                                x.plo_outcome.map(|s| format!("/{s}")).unwrap_or_default()
                            ),
                            methods,
                            n: r.stats.n,
                            target: r.target,
                            result,
                            ci,
                            attained: r.attained,
                            indistinguishable: r.indistinguishable,
                            fcar: r.stats.eamu.as_ref().map(|e| e.flag),
                        });
                    }
                }
                let result = so_values.get(&so.code).copied();
                AbetSoRow {
                    code: so.code.clone(),
                    text: so.text.clone(),
                    indicators,
                    result,
                    target,
                    attained: result.map(|v| v >= target),
                    actions: actions.get(&so.code).cloned().unwrap_or_default(),
                }
            })
            .collect();
        Self {
            meta,
            framework: format!("{} ({})", framework.name, framework.edition),
            suppress_n,
            rows,
        }
    }

    /// The summary as one flat sheet: one row per indicator (an SO without indicators gets a
    /// row of its own), SO columns repeated.
    pub fn sheet(&self) -> Sheet {
        let header: Vec<String> = [
            "SO",
            "Student outcome",
            "Course",
            "Term",
            "CLO",
            "PLO",
            "Strength",
            "Assessment methods",
            "N",
            "Target %",
            "Result",
            "95% CI low",
            "95% CI high",
            "Verdict",
            "FCAR flag",
            "SO result %",
            "SO target %",
            "SO verdict",
            "Actions",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let verdict = |a: Option<bool>, ind: bool| -> Cell {
            Cell::opt_text(a.map(|a| {
                let v = if a { "attained" } else { "not attained" };
                if ind {
                    format!("{v} (≈ target, 95% CI)")
                } else {
                    v.to_owned()
                }
            }))
        };
        let pct = |x: &Exact| Cell::num(Some(exact_str(&(x * Exact::from_integer(100)), 1)), 1);
        let suppressed = || Cell::text(format!("suppressed (N < {})", self.suppress_n));
        let mut rows = Vec::new();
        for so in &self.rows {
            // A small group's statistics would disclose individual results (SCO-023: cells
            // with n below the threshold are suppressed in shared outputs), and so would a
            // roll-up that rests only on small groups.
            let so_small =
                !so.indicators.is_empty() && so.indicators.iter().all(|i| i.n < self.suppress_n);
            let so_cells = |c: &mut Vec<Cell>| {
                if so_small {
                    c.push(Cell::Empty);
                    c.push(pct(&so.target));
                    c.push(suppressed());
                } else {
                    c.push(so.result.as_ref().map_or(Cell::Empty, pct));
                    c.push(pct(&so.target));
                    c.push(verdict(so.attained, false));
                }
                c.push(Cell::text(so.actions.clone()));
            };
            if so.indicators.is_empty() {
                let mut c = vec![Cell::text(so.code.clone()), Cell::text(so.text.clone())];
                c.extend(std::iter::repeat_n(Cell::Empty, 13));
                c[13] = Cell::text("not assessed");
                so_cells(&mut c);
                rows.push(Row::new(c));
                continue;
            }
            for i in &so.indicators {
                let small = i.n < self.suppress_n;
                let mut c = vec![
                    Cell::text(so.code.clone()),
                    Cell::text(so.text.clone()),
                    Cell::text(i.course.clone()),
                    Cell::text(i.term.clone()),
                    Cell::text(i.clo.clone()),
                    Cell::text(i.plo.clone()),
                    Cell::text(i.strength.clone()),
                    Cell::text(i.methods.clone()),
                    if small {
                        Cell::text(format!("< {}", self.suppress_n))
                    } else {
                        Cell::Int(i.n as i64)
                    },
                    pct(&i.target),
                ];
                if small {
                    c.extend([
                        Cell::Empty,
                        Cell::Empty,
                        Cell::Empty,
                        suppressed(),
                        Cell::Empty,
                    ]);
                } else {
                    c.extend([
                        Cell::num(i.result.clone(), 1),
                        Cell::num(i.ci.and_then(|c| float_str(c.0, 1)), 1),
                        Cell::num(i.ci.and_then(|c| float_str(c.1, 1)), 1),
                        verdict(i.attained, i.indistinguishable),
                        Cell::opt_text(i.fcar.map(|f| format!("{f:?}").to_lowercase())),
                    ]);
                }
                so_cells(&mut c);
                rows.push(Row::new(c));
            }
        }
        Sheet {
            name: "ABET Criterion 3".into(),
            title: format!("{} — {}", self.meta.title, self.framework),
            headers: vec![header],
            rows,
            notes: vec![
                "Aggregate results only: no student-level data (pseudonymous export, §17.6)."
                    .into(),
                "Result = the deciding statistic of each CLO's attainment policy, on uncurved \
                 item scores (§11.3); SO result = strength-weighted roll-up (§11.5)."
                    .into(),
                format!(
                    "Counts below {} are suppressed in shared outputs (§12.3).",
                    self.suppress_n
                ),
            ],
            rtl: false,
            widths: vec![
                6.0, 40.0, 9.0, 9.0, 7.0, 7.0, 8.0, 34.0, 6.0, 8.0, 9.0, 9.0, 9.0, 16.0, 9.0, 10.0,
                10.0, 12.0, 30.0,
            ],
        }
    }

    /// CSV.
    pub fn write_csv<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        write_csv(w, &self.sheet())
    }

    /// XLSX.
    pub fn write_xlsx<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        write_xlsx(w, &self.meta, &[self.sheet()])
    }
}
