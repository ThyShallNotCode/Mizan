//! A9 (docs/IMPLEMENTATION.md §1): outcome-engine oracle tests. The numbers are the §11.3
//! worked example of docs/TECHNICAL_DESIGN.md, re-derived by hand in the comments.

use omr_model::{FormCode, StudentKey};
use omr_outcomes::attainment::{PerQuestionData, class_stats};
use omr_outcomes::mapping::evidence_share;
use omr_outcomes::policy::{ActualLevelDisplay, EamuPopulation, VerdictBasis, mid_term_change};
use omr_outcomes::report::compress_labels;
use omr_outcomes::rollup::check_like_scales;
use omr_outcomes::*;
use omr_scoring::stats::SplitMix64;
use omr_scoring::{
    Adjustments, BookletL1, Exact, FormMap, GradeOutcome, GradeScale, KeyEntry, KeyVersion,
    L1Response, MasterItem, PolicyVersion, ResponseSnapshot, RosterStatus, ScoreRun, StatusCode,
    exact, pts, score,
};
use std::collections::BTreeMap;

fn e(n: i128, d: i128) -> Exact {
    exact(n, d)
}

/// A score run of score-box items: `items` = (label, M_i); `rows` = (student, points per item).
fn box_run(items: &[(&str, i64)], rows: &[(&str, Vec<i64>)]) -> ScoreRun {
    let forms = FormMap::single_form(
        items
            .iter()
            .map(|(l, m)| MasterItem::score_box(*l, pts(*m, 1)))
            .collect(),
    );
    let key = KeyVersion::new(items.iter().map(|_| KeyEntry::Open).collect());
    let l1 = ResponseSnapshot {
        booklets: rows
            .iter()
            .enumerate()
            .map(|(k, (s, v))| {
                BookletL1::present(
                    k as u32 + 1,
                    Some(s),
                    FormCode(1),
                    v.iter().map(|x| L1Response::Points(pts(*x, 1))).collect(),
                )
            })
            .collect(),
    };
    score(
        &l1,
        &forms,
        &key,
        &PolicyVersion::default(),
        &Adjustments::default(),
    )
    .unwrap()
}

fn roster(ids: &[&str], section: &str) -> Vec<RosterEntry> {
    ids.iter()
        .map(|s| RosterEntry {
            student: StudentKey((*s).into()),
            section: section.into(),
            status: RosterStatus::Enrolled,
            passing: Some(true),
        })
        .collect()
}

fn section(id: &str, campus: &str, track: Track) -> SectionInfo {
    SectionInfo {
        id: id.into(),
        campus: campus.into(),
        track,
    }
}

fn clo_set(codes: &[(&str, Domain)]) -> CloSet {
    CloSet {
        course: "CS 101".into(),
        version: 1,
        clos: codes
            .iter()
            .map(|(c, d)| Clo::new(c, &format!("{c} text"), *d))
            .collect(),
    }
}

fn map(rows: &[(&str, usize, &str)]) -> MappingSet {
    MappingSet {
        version: 1,
        mode: MappingMode::Full,
        rows: rows
            .iter()
            .map(|(a, i, c)| ItemCloMap::approved(a, *i, c, pts(1, 1)))
            .collect(),
    }
}

/// The §11.3 worked example: Quiz (W 0.10, M 20), Midterm (0.30, 40), Final (0.60, 100);
/// every item maps to one CLO with w = 1. Student S1's points as in the spec.
fn worked_example() -> (Offering, CloSet, MappingSet) {
    let quiz = box_run(&[("Q1", 12), ("Q2", 8)], &[("S1", vec![9, 6])]);
    let mid = box_run(
        &[("Q1", 8), ("Q2", 22), ("Q3", 10)],
        &[("S1", vec![4, 16, 6])],
    );
    let fin = box_run(
        &[("Q1", 20), ("Q2", 20), ("Q3", 20), ("Q4", 40)],
        &[("S1", vec![15, 11, 12, 26])],
    );
    let offering = Offering {
        course: "CS 101".into(),
        term: "1447-1".into(),
        credit_hours: 3,
        sections: vec![section("101", "main", Track::Male)],
        roster: roster(&["S1"], "101"),
        assessments: vec![
            AssessmentEvidence::from_run("quiz", "Quiz", e(1, 10), &quiz),
            AssessmentEvidence::from_run("mid", "Midterm", e(3, 10), &mid),
            AssessmentEvidence::from_run("final", "Final", e(6, 10), &fin),
        ],
    };
    let clos = clo_set(&[
        ("CLO1", Domain::Knowledge),
        ("CLO2", Domain::Skills),
        ("CLO3", Domain::Skills),
        ("CLO4", Domain::Values),
    ]);
    let mapping = map(&[
        ("quiz", 0, "CLO1"),
        ("quiz", 1, "CLO2"),
        ("mid", 0, "CLO2"),
        ("mid", 1, "CLO3"),
        ("mid", 2, "CLO4"),
        ("final", 0, "CLO1"),
        ("final", 1, "CLO2"),
        ("final", 2, "CLO3"),
        ("final", 3, "CLO4"),
    ]);
    (offering, clos, mapping)
}

#[test]
fn req_a9_student_performance_worked_example() {
    let (offering, clos, mapping) = worked_example();
    let p = AttainmentPolicy::default();
    let r = compute_attainment(&offering, &clos, &mapping, &p, &[]);
    // ω_{a,c} = W_a·M_{a,c}/M_a; CLO1: 0.06 + 0.12 = 0.18 -> (0.06·0.75 + 0.12·0.75)/0.18 = 0.750
    assert_eq!(r.p("S1", "CLO1"), Some(e(3, 4)));
    // CLO2: (0.04·0.75 + 0.06·0.50 + 0.12·0.55)/0.22 = 0.126/0.22 = 63/110 = 0.573
    assert_eq!(r.p("S1", "CLO2"), Some(e(63, 110)));
    // CLO3: (0.165·16/22 + 0.12·0.60)/0.285 = 0.192/0.285 = 64/95 = 0.674
    assert_eq!(r.p("S1", "CLO3"), Some(e(64, 95)));
    // CLO4: (0.075·0.60 + 0.24·0.65)/0.315 = 0.201/0.315 = 67/105 = 0.638
    assert_eq!(r.p("S1", "CLO4"), Some(e(67, 105)));
    // §11.2 identity: Σ_c ω_c·P_{S1,c} = 0.135 + 0.126 + 0.192 + 0.201 = 0.654 =
    // 0.1·15/20 + 0.3·26/40 + 0.6·64/100, S1's uncurved course percentage.
    let total: Exact = ["CLO1", "CLO2", "CLO3", "CLO4"]
        .iter()
        .map(|c| evidence_share(&mapping, &offering, c) * r.p("S1", c).unwrap())
        .sum();
    assert_eq!(total, e(654, 1000));
    assert_eq!(evidence_share(&mapping, &offering, "CLO3"), e(285, 1000));
    // Other student methods on CLO2: points-pooled 21/36; assessment-equal
    // (0.75 + 0.50 + 0.55)/3 = 3/5.
    let pp = AttainmentPolicy {
        student_method: StudentMethod::PointsPooled,
        ..AttainmentPolicy::default()
    };
    let r_pp = compute_attainment(&offering, &clos, &mapping, &pp, &[]);
    assert_eq!(r_pp.p("S1", "CLO2"), Some(e(21, 36)));
    let ae = AttainmentPolicy {
        student_method: StudentMethod::AssessmentEqual,
        ..AttainmentPolicy::default()
    };
    let r_ae = compute_attainment(&offering, &clos, &mapping, &ae, &[]);
    assert_eq!(r_ae.p("S1", "CLO2"), Some(e(3, 5)));
    // τ = 0.60: S1 misses CLO2 under the default (63/110 < 3/5) and meets it exactly under
    // assessment-equal (3/5 ≥ 3/5): exact comparison.
    assert_eq!(r.clo("CLO2").unwrap().stats.met, 0);
    assert_eq!(r_ae.clo("CLO2").unwrap().stats.met, 1);
    // Custom V_{a,c}: final counts 60 % toward CLO2, midterm 40 %: 0.6·0.55 + 0.4·0.5 = 0.53.
    let custom = AttainmentPolicy {
        student_method: StudentMethod::Custom(BTreeMap::from([
            (("final".to_string(), "CLO2".to_string()), e(6, 10)),
            (("mid".to_string(), "CLO2".to_string()), e(4, 10)),
        ])),
        ..AttainmentPolicy::default()
    };
    let r_c = compute_attainment(&offering, &clos, &mapping, &custom, &[]);
    assert_eq!(r_c.p("S1", "CLO2"), Some(e(53, 100)));
}

/// 25 students' CLO2 values: 3 E, 7 A, 8 M, 7 U; 18 at or above τ = 0.60; Σ = 17.10.
fn class_values() -> Vec<Exact> {
    [
        95, 92, 90, 85, 80, 78, 76, 75, 88, 82, 70, 65, 62, 60, 72, 68, 74, 61, 45, 50, 45, 46, 40,
        52, 59,
    ]
    .iter()
    .map(|v| e(*v, 100))
    .collect()
}

#[test]
fn req_a9_average_threshold_levels_eamu() {
    let v = class_values();
    let pass = vec![Some(true); v.len()];
    let policy = AttainmentPolicy::default();
    let mut rng = SplitMix64::new(1);
    let s = class_stats(&v, &pass, &e(6, 10), &policy, None, &mut rng);
    // Average: μ = 17.10/25 = 0.684 < T = 0.70 -> not attained. Sample SD 0.16109 (scipy),
    // t_{24, .975} = 2.0639 -> CI [0.6175, 0.7505].
    assert_eq!(s.mean, Some(e(684, 1000)));
    assert!(s.mean.unwrap() < e(7, 10));
    let (lo, hi) = s.mean_ci.unwrap();
    assert!(
        (lo - 0.617_505).abs() < 1e-5 && (hi - 0.750_495).abs() < 1e-5,
        "{lo} {hi}"
    );
    assert!(
        s.mean_bootstrap_ci.is_some(),
        "N < 30 adds the seeded bootstrap"
    );
    // Threshold-proportion: 18/25 = 72.0 % ≥ 70 % -> attained; Wilson [52.4 %, 85.7 %]
    // contains 70 % -> "indistinguishable from target".
    assert_eq!(s.met, 18);
    assert_eq!(s.proportion, Some(e(18, 25)));
    let (lo, hi) = s.proportion_ci.unwrap();
    assert!((lo - 0.524_234).abs() < 1e-5 && (hi - 0.857_161).abs() < 1e-5);
    // Levels (0.6, 0.7, 0.8): π = 0.72 ≥ 0.70 but < 0.80 -> level 2.
    assert_eq!(s.level, Some(2));
    // EAMU: E/A/M/U = 3/7/8/7; avg₃ = (9 + 14 + 8)/25 = 31/25 = 1.24; avg₅ = 31/15 = 2.07.
    // U = 28 % > 10 % and avg₅ < 3.3 -> red flag.
    let m = s.eamu.unwrap();
    assert_eq!((m.e, m.a, m.m, m.u), (3, 7, 8, 7));
    assert_eq!(m.avg3, e(31, 25));
    assert_eq!(m.avg5, e(31, 15));
    assert_eq!(m.flag, FcarFlag::Red);
    // Passing-students-only population: mark the 7 U students as failing -> E/A/M/U =
    // 3/7/8/0, avg₃ = 31/18, avg₅ = 155/54 = 2.87 < 3.3 but U = 0 -> yellow.
    let pass2: Vec<Option<bool>> = v.iter().map(|x| Some(*x >= e(6, 10))).collect();
    let pol2 = AttainmentPolicy {
        eamu_population: EamuPopulation::Passing,
        ..AttainmentPolicy::default()
    };
    let s2 = class_stats(&v, &pass2, &e(6, 10), &pol2, None, &mut rng);
    let m2 = s2.eamu.unwrap();
    assert_eq!((m2.n, m2.u), (18, 0));
    assert_eq!(m2.avg5, e(155, 54));
    assert_eq!(m2.flag, FcarFlag::Yellow);
    // Green: avg₅ > 4.6 and U = 0 (all E: avg₅ = 5).
    let g = eamu(&[e(95, 100); 4], &policy.eamu_bands, &policy.fcar_flags);
    assert_eq!(g.flag, FcarFlag::Green);
}

/// 25 students × 5 dichotomous items (M = 1): row sums 1×5, 17×4, 7×2 (Σ = 87); column sums
/// 19, 16, 20, 14, 18 (q = 0.76, 0.64, 0.80, 0.56, 0.72).
fn per_question_matrix() -> Vec<Vec<bool>> {
    let row_sums: Vec<usize> = std::iter::once(5)
        .chain(std::iter::repeat_n(4, 17))
        .chain(std::iter::repeat_n(2, 7))
        .collect();
    let mut col_left = [19usize, 16, 20, 14, 18];
    let mut m = Vec::new();
    for r in row_sums {
        // Greedy (Gale–Ryser): give this row's answers to the columns with most demand left.
        let mut order: Vec<usize> = (0..5).collect();
        order.sort_by_key(|&c| (std::cmp::Reverse(col_left[c]), c));
        let mut row = vec![false; 5];
        for &c in order.iter().take(r) {
            row[c] = true;
            col_left[c] -= 1;
        }
        m.push(row);
    }
    assert_eq!(col_left, [0; 5]);
    m
}

/// Per-question offering: Midterm Q1–Q7; Q1–Q2 → CLO1 (20 right, 5 wrong on both);
/// Q3–Q7 → CLO2 (the matrix above). Key A everywhere.
fn per_question_offering() -> (Offering, CloSet, MappingSet) {
    let items: Vec<MasterItem> = (1..=7)
        .map(|i| MasterItem::sba(format!("Q{i}"), 4, pts(1, 1)))
        .collect();
    let forms = FormMap::single_form(items);
    let key = KeyVersion::new((0..7).map(|_| KeyEntry::single(0, 4)).collect());
    let m = per_question_matrix();
    let ids: Vec<String> = (1..=25).map(|k| format!("S{k:02}")).collect();
    let l1 = ResponseSnapshot {
        booklets: m
            .iter()
            .enumerate()
            .map(|(k, row)| {
                let clo1 = if k < 20 { 0 } else { 1 };
                let mut r = vec![L1Response::Single(clo1), L1Response::Single(clo1)];
                r.extend(
                    row.iter()
                        .map(|&ok| L1Response::Single(if ok { 0 } else { 2 })),
                );
                BookletL1::present(k as u32 + 1, Some(&ids[k]), FormCode(1), r)
            })
            .collect(),
    };
    let run = score(
        &l1,
        &forms,
        &key,
        &PolicyVersion::default(),
        &Adjustments::default(),
    )
    .unwrap();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let offering = Offering {
        course: "CS 101".into(),
        term: "1447-1".into(),
        credit_hours: 3,
        sections: vec![section("101", "main", Track::Female)],
        roster: roster(&id_refs, "101"),
        assessments: vec![AssessmentEvidence::from_run(
            "mid",
            "Midterm",
            e(1, 1),
            &run,
        )],
    };
    let clos = clo_set(&[("CLO1", Domain::Knowledge), ("CLO2", Domain::Skills)]);
    let mut rows = vec![("mid", 0, "CLO1"), ("mid", 1, "CLO1")];
    rows.extend((2..7).map(|i| ("mid", i, "CLO2")));
    (offering, clos, map(&rows))
}

#[test]
fn req_a9_taif_per_question() {
    let (offering, clos, mapping) = per_question_offering();
    let taif = AttainmentPolicy::taif();
    let r = compute_attainment(&offering, &clos, &mapping, &taif, &[]);
    let c2 = r.clo("CLO2").unwrap();
    // ρ = (0.76 + 0.64 + 0.80 + 0.56 + 0.72)/5 = 3.48/5 = 87/125 = 69.6 %.
    assert_eq!(c2.stats.per_question, Some(e(87, 125)));
    // Taif template: 60 % target -> attained. Under the default 70 % it would not be.
    assert_eq!(c2.attained, Some(true));
    assert!(c2.stats.per_question_ci.is_some(), "seeded bootstrap CI");
    let strict = AttainmentPolicy {
        class_method: ClassMethod::PerQuestion,
        ..AttainmentPolicy::default()
    };
    let r2 = compute_attainment(&offering, &clos, &mapping, &strict, &[]);
    assert_eq!(r2.clo("CLO2").unwrap().attained, Some(false));
    // Same data, other methods: π = 18/25 (rows with ≥ 3 of 5) = 72 %; μ = 87/125.
    assert_eq!(c2.stats.proportion, Some(e(18, 25)));
    assert_eq!(c2.stats.mean, Some(e(87, 125)));
    // Bootstrap is reproducible (seeded from the run IDs).
    let again = compute_attainment(&offering, &clos, &mapping, &taif, &[]);
    assert_eq!(r, again);
    // Direct PerQuestionData oracle with unequal weights, τ = 0.6: item 1 (w 2, M 1): s⁺ 1
    // meets 0.6, 0 does not -> q = ½; item 2 (w 1, M 2): threshold 1.2, s⁺ 2 meets, 1 does
    // not -> q = ½. ρ = (2·½ + 1·½)/3 = ½.
    let d = PerQuestionData::from_scores(
        &[(e(2, 1), e(1, 1)), (e(1, 1), e(2, 1))],
        &[
            vec![Some(e(1, 1)), Some(e(2, 1))],
            vec![Some(e(0, 1)), Some(e(1, 1))],
        ],
        &e(6, 10),
    );
    assert_eq!(d.rho(&[0, 1]), Some(e(1, 2)));
    // Item 2 not presented to student 2: q₂ = 1/1 -> ρ = (2·½ + 1·1)/3 = 2/3.
    let d2 = PerQuestionData::from_scores(
        &[(e(2, 1), e(1, 1)), (e(1, 1), e(2, 1))],
        &[
            vec![Some(e(1, 1)), Some(e(2, 1))],
            vec![Some(e(0, 1)), None],
        ],
        &e(6, 10),
    );
    assert_eq!(d2.rho(&[0, 1]), Some(e(2, 3)));
}

#[test]
fn req_a9_sections_pooling_blend_and_suppression() {
    // Section 101 (main, male): 25 students, 18 with P ≥ 0.6; section 102 (branch, female):
    // 10 students, 6 with P ≥ 0.6. One score-box item (M = 10) mapped to CLO1.
    let mut rows = Vec::new();
    let mut rost = Vec::new();
    for k in 0..35 {
        let id = format!("S{k:02}");
        let sec = if k < 25 { "101" } else { "102" };
        let met = if k < 25 { k < 18 } else { k < 31 };
        rows.push((id.clone(), vec![if met { 8 } else { 3 }]));
        rost.push(RosterEntry {
            student: StudentKey(id),
            section: sec.into(),
            status: RosterStatus::Enrolled,
            passing: Some(true),
        });
    }
    let row_refs: Vec<(&str, Vec<i64>)> =
        rows.iter().map(|(s, v)| (s.as_str(), v.clone())).collect();
    let run = box_run(&[("Q1", 10)], &row_refs);
    let offering = Offering {
        course: "CS 101".into(),
        term: "1447-1".into(),
        credit_hours: 3,
        sections: vec![
            section("101", "main", Track::Male),
            section("102", "B01", Track::Female),
        ],
        roster: rost,
        assessments: vec![AssessmentEvidence::from_run(
            "final",
            "Final",
            e(1, 1),
            &run,
        )],
    };
    let clos = clo_set(&[("CLO1", Domain::Knowledge)]);
    let mapping = map(&[("final", 0, "CLO1")]);
    let r = compute_attainment(
        &offering,
        &clos,
        &mapping,
        &AttainmentPolicy::default(),
        &[],
    );
    let c = r.clo("CLO1").unwrap();
    // Pooled students: 24/35 = 68.6 % (Wilson [52.0 %, 81.4 %]) -> not attained at 70 %.
    assert_eq!(c.stats.proportion, Some(e(24, 35)));
    assert_eq!(c.attained, Some(false));
    let (lo, hi) = c.stats.proportion_ci.unwrap();
    assert_eq!(
        ((lo * 1000.0).round(), (hi * 1000.0).round()),
        (520.0, 814.0)
    );
    let group = |k: GroupKey| c.groups.iter().find(|g| g.key == k).unwrap();
    assert_eq!(
        group(GroupKey::Section("101".into())).value,
        Some(e(18, 25))
    );
    assert_eq!(group(GroupKey::Section("102".into())).value, Some(e(6, 10)));
    assert_eq!(group(GroupKey::Campus("B01".into())).value, Some(e(6, 10)));
    assert_eq!(group(GroupKey::Track(Track::Male)).value, Some(e(18, 25)));
    assert_eq!(group(GroupKey::Pooled).value, Some(e(24, 35)));
    // Pooling helpers: 24/35 pooled; the opt-in mean of section rates is 66.0 %.
    assert_eq!(
        omr_outcomes::pool_proportions(&[(18, 25), (6, 10)])
            .unwrap()
            .0,
        e(24, 35)
    );
    assert_eq!(
        omr_outcomes::mean_of_section_rates(&[(18, 25), (6, 10)]),
        Some(e(66, 100))
    );
    let mos = AttainmentPolicy {
        section_aggregation: policy::SectionAggregation::MeanOfSections,
        ..AttainmentPolicy::default()
    };
    let r2 = compute_attainment(&offering, &clos, &mapping, &mos, &[]);
    assert_eq!(r2.clo("CLO1").unwrap().value, Some(e(66, 100)));
    // Indirect blend (SEU 70/30): section 101 alone: π = 0.72, 16 of 20 survey respondents
    // ≥ 4 -> Indirect 0.80 -> A = 0.7·0.72 + 0.3·0.80 = 0.744.
    let only101 = Offering {
        roster: offering.roster[..25].to_vec(),
        ..offering.clone()
    };
    let blend = AttainmentPolicy {
        beta: e(7, 10),
        ..AttainmentPolicy::default()
    };
    let ind = [IndirectEvidence {
        clo: "CLO1".into(),
        respondents: 20,
        at_or_above: 16,
    }];
    let r3 = compute_attainment(&only101, &clos, &mapping, &blend, &ind);
    let c3 = r3.clo("CLO1").unwrap();
    assert_eq!(c3.direct, Some(e(18, 25)));
    assert_eq!(c3.indirect, Some(e(4, 5)));
    assert_eq!(c3.value, Some(e(744, 1000)));
    // PSU practice: blending not allowed -> A_c = Direct, Indirect printed beside it.
    let psu = AttainmentPolicy {
        beta: e(7, 10),
        ..AttainmentPolicy::psu()
    };
    let r4 = compute_attainment(&only101, &clos, &mapping, &psu, &ind);
    assert_eq!(r4.clo("CLO1").unwrap().value, Some(e(18, 25)));
    // Suppression: a 3-student section is suppressed in shared outputs.
    let tiny = Offering {
        roster: offering
            .roster
            .iter()
            .cloned()
            .map(|mut x| {
                if x.student.0 == "S00" || x.student.0 == "S01" || x.student.0 == "S02" {
                    x.section = "103".into();
                }
                x
            })
            .collect(),
        sections: [
            offering.sections.clone(),
            vec![section("103", "main", Track::Male)],
        ]
        .concat(),
        ..offering.clone()
    };
    let r5 = compute_attainment(&tiny, &clos, &mapping, &AttainmentPolicy::default(), &[]);
    let g = r5
        .clo("CLO1")
        .unwrap()
        .groups
        .iter()
        .find(|g| g.key == GroupKey::Section("103".into()))
        .unwrap()
        .clone();
    assert!(g.suppressed);
    assert_eq!(omr_outcomes::suppress_count(g.stats.n, 5), "< 5");
}

#[test]
fn req_a9_plo_rollup_worked_example() {
    // A_c = π_c: CLO1 0.84, CLO2 0.72, CLO3 0.64, CLO4 0.68.
    let a: BTreeMap<String, Exact> = [("CLO1", 84), ("CLO2", 72), ("CLO3", 64), ("CLO4", 68)]
        .iter()
        .map(|(c, v)| (c.to_string(), e(*v, 100)))
        .collect();
    // PLO1 ← CLO1 (M = 3), CLO2 (P = 2), CLO4 (I = 1); PLO2 ← CLO3 (M), CLO4 (P).
    let links = vec![
        CloPloMap::new("CLO1", "K1", 3),
        CloPloMap::new("CLO2", "K1", 2),
        CloPloMap::new("CLO4", "K1", 1),
        CloPloMap::new("CLO3", "S1", 3),
        CloPloMap::new("CLO4", "S1", 2),
    ];
    let plo = plo_course_level(&a, &links, false);
    // PLO1 = (3·0.84 + 2·0.72 + 1·0.68)/6 = 4.64/6 = 58/75 = 0.773;
    // PLO2 = (3·0.64 + 2·0.68)/5 = 3.28/5 = 82/125 = 0.656.
    assert_eq!(plo["K1"], e(58, 75));
    assert_eq!(plo["S1"], e(82, 125));
    // M-only variant: 0.84 and 0.64.
    let m_only = plo_course_level(&a, &links, true);
    assert_eq!((m_only["K1"], m_only["S1"]), (e(84, 100), e(64, 100)));
    // Student level, S1 of the worked example:
    // P_{S1,PLO1} = (3·0.750 + 2·63/110 + 67/105)/6 = 3727/5544 = 0.672;
    // P_{S1,PLO2} = (3·64/95 + 2·67/105)/5 = 6578/9975 = 0.659.
    let (offering, clos, mapping) = worked_example();
    let r = compute_attainment(
        &offering,
        &clos,
        &mapping,
        &AttainmentPolicy::default(),
        &[],
    );
    let sp = plo_student_level(&r.students, &links, false);
    let k = |p: &str| (StudentKey("S1".into()), p.to_string());
    assert_eq!(sp[&k("K1")], e(3727, 5544));
    assert_eq!(sp[&k("S1")], e(6578, 9975));
    // M_{k,p} on the worked example (each item once, g_i = W_a·M_i/M_a):
    // PLO1 items Quiz Q1 0.06, Q2 0.04, Mid Q1 0.06, Q3 0.075, Final Q1 0.12, Q2 0.12,
    // Q4 0.24 -> 0.715; PLO2: Mid Q2 0.165, Q3 0.075, Final Q3 0.12, Q4 0.24 -> 0.600.
    assert_eq!(
        mapped_grade_share(&offering, &mapping, &links, "K1"),
        e(715, 1000)
    );
    assert_eq!(
        mapped_grade_share(&offering, &mapping, &links, "S1"),
        e(6, 10)
    );
    // Credit-hour AR: X (C 3, M 0.40, A 58/75), Y (4, 0.25, 0.700), Z (2, 0.60, 0.800):
    // (0.928 + 0.700 + 0.960)/3.4 = 2.588/3.4 = 647/850 = 0.761.
    let contribs = [
        CourseContribution {
            course: "X".into(),
            credit_hours: e(3, 1),
            attainment: e(58, 75),
            mapped_share: e(4, 10),
        },
        CourseContribution {
            course: "Y".into(),
            credit_hours: e(4, 1),
            attainment: e(7, 10),
            mapped_share: e(25, 100),
        },
        CourseContribution {
            course: "Z".into(),
            credit_hours: e(2, 1),
            attainment: e(8, 10),
            mapped_share: e(6, 10),
        },
    ];
    let ar = credit_hour_rollup(&contribs).unwrap();
    assert_eq!(ar, e(647, 850));
    // Exit exam X_PLO1 = 0.65 at γ = 0.3: 0.7·0.7612 + 0.3·0.65 = 12373/17000 = 0.728.
    let blended = exit_exam_blend(&ar, Some(&e(65, 100)), &e(3, 10));
    assert_eq!(blended, e(12373, 17000));
    assert_eq!(
        exit_exam_blend(&ar, None, &e(3, 10)),
        ar,
        "γ = 0 without an exit exam"
    );
    // Framework level: SO1 ← PLO1 (x 3), PLO2 (x 1): (3·58/75 + 82/125)/4 = 93/125.
    let xw = vec![
        Crosswalk::new("K1", "SO1", 3),
        Crosswalk::new("S1", "SO1", 1),
    ];
    assert_eq!(framework_rollup(&plo, &xw)["SO1"], e(93, 125));
}

#[test]
fn req_a9_exit_exam_item_outcome_map() {
    // An exit exam (programme level) with 4 score-box items mapped straight to PLOs:
    // Q1, Q2 → K1; Q3, Q4 → S1. Three students.
    let run = box_run(
        &[("Q1", 5), ("Q2", 5), ("Q3", 5), ("Q4", 5)],
        &[
            ("A", vec![5, 5, 1, 1]),
            ("B", vec![3, 3, 4, 4]),
            ("C", vec![1, 1, 5, 5]),
        ],
    );
    let mut exam = AssessmentEvidence::from_run("exit", "Exit exam", e(1, 1), &run);
    exam.programme_level = true;
    let offering = Offering {
        course: "EXIT".into(),
        term: "1447-2".into(),
        credit_hours: 0,
        sections: vec![section("1", "main", Track::Mixed)],
        roster: roster(&["A", "B", "C"], "1"),
        assessments: vec![exam],
    };
    let om: Vec<ItemOutcomeMap> = [(0, "K1"), (1, "K1"), (2, "S1"), (3, "S1")]
        .iter()
        .map(|(i, o)| ItemOutcomeMap {
            assessment: "exit".into(),
            item: *i,
            outcome: (*o).into(),
            outcome_version: 1,
            weight: pts(1, 1),
            note: String::new(),
            rationale: None,
            status: MappingStatus::Approved,
        })
        .collect();
    // K1: P = 1.0, 0.6, 0.2 -> 2 of 3 ≥ 0.6 = 2/3. S1: 0.2, 0.8, 1.0 -> 2/3.
    let x = exit_exam_attainment(&offering, &om, &AttainmentPolicy::default());
    assert_eq!(x["K1"], e(2, 3));
    assert_eq!(x["S1"], e(2, 3));
    // X_p follows the deciding class method. Average: K1 (1.0 + 0.6 + 0.2)/3 = 3/5,
    // S1 (0.2 + 0.8 + 1.0)/3 = 2/3. Levels (0.6, 0.7, 0.8): π = 2/3 ≥ 0.6 but < 0.7 ->
    // level 1, on the same scale as the course-level values it blends with.
    let avg = AttainmentPolicy {
        class_method: ClassMethod::Average,
        ..AttainmentPolicy::default()
    };
    let xa = exit_exam_attainment(&offering, &om, &avg);
    assert_eq!((xa["K1"], xa["S1"]), (e(3, 5), e(2, 3)));
    let lv = AttainmentPolicy {
        class_method: ClassMethod::Levels,
        ..AttainmentPolicy::default()
    };
    assert_eq!(exit_exam_attainment(&offering, &om, &lv)["K1"], e(1, 1));
    // Exit-exam evidence never feeds a CLO: the CLO engine ignores programme-level exams.
    let clos = clo_set(&[("CLO1", Domain::Knowledge)]);
    let r = compute_attainment(
        &offering,
        &clos,
        &map(&[("exit", 0, "CLO1")]),
        &AttainmentPolicy::default(),
        &[],
    );
    assert_eq!(r.p("A", "CLO1"), None);
}

#[test]
fn req_a9_tp154_b1_rows() {
    let (offering, clos, mapping) = per_question_offering();
    let links = vec![
        CloPloMap::new("CLO1", "K1", 3),
        CloPloMap::new("CLO2", "S2", 2),
        CloPloMap::new("CLO2", "S1", 3),
    ];
    // UoM practice: the average decides, both values are displayed.
    let policy = AttainmentPolicy::uom();
    let r = compute_attainment(&offering, &clos, &mapping, &policy, &[]);
    let t = tp154_b1(
        &offering,
        &clos,
        &mapping,
        &links,
        &r,
        &policy,
        &BTreeMap::new(),
    );
    assert_eq!(t.groups.len(), 3);
    assert_eq!(t.groups[0].number, "1.0");
    assert_eq!(t.groups[0].heading_en, "Knowledge and understanding");
    assert!(t.groups[2].rows.is_empty(), "Values: no CLO this term");
    // CLO1 (Knowledge): 20 of 25 answered both items -> μ = π = 80.0 % ≥ 70 %.
    let r1 = &t.groups[0].rows[0];
    assert_eq!(r1.number, "1.1");
    assert_eq!(r1.plo_codes, vec!["K1"]);
    assert_eq!(r1.methods, "Midterm Q1–2");
    assert_eq!(r1.targeted, "70.0%");
    assert_eq!(r1.actual, "Average 80.0%; 80.0% of students ≥ 60.0% *");
    assert_eq!(r1.attained, Some(true));
    assert!(!r1.comment_required);
    // CLO2 (Skills): μ = 87/125 = 69.6 % < 70 % -> not attained under the average, though
    // 72.0 % of students reach τ: comment and B.2 recommendation required.
    let r2 = &t.groups[1].rows[0];
    assert_eq!(r2.number, "2.1");
    assert_eq!(r2.plo_codes, vec!["S1", "S2"]);
    assert_eq!(r2.methods, "Midterm Q3–7");
    assert_eq!(r2.actual_average.as_deref(), Some("69.6%"));
    assert_eq!(r2.actual_proportion.as_deref(), Some("72.0%"));
    assert_eq!(r2.attained, Some(false));
    assert!(r2.comment_required);
    assert!(
        t.footnote.contains("average")
            && t.footnote.contains("60.0%")
            && t.footnote.contains("70.0%")
    );
    assert!(!t.draft_watermark);
    // method_only display prints the deciding value alone.
    let p2 = AttainmentPolicy {
        actual_level_display: ActualLevelDisplay::MethodOnly,
        ..AttainmentPolicy::default()
    };
    let r_b = compute_attainment(&offering, &clos, &mapping, &p2, &[]);
    let t2 = tp154_b1(
        &offering,
        &clos,
        &mapping,
        &links,
        &r_b,
        &p2,
        &BTreeMap::new(),
    );
    assert_eq!(t2.groups[1].rows[0].actual, "72.0% *");
    assert_eq!(t2.groups[1].rows[0].actual_average, None);
    // Label compression.
    let l: Vec<String> = ["Q3", "Q4", "Q5", "Q9", "Q12", "Q13", "Essay"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(compress_labels(&l), "Q3–5, Q9, Q12–13, Essay");
}

#[test]
fn req_a9_tp154_a1_distribution() {
    let s = GradeScale::saudi();
    let l = |x: &str| {
        GradeOutcome::letter_for(
            &s,
            &match x {
                "A+" => e(97, 1),
                "A" => e(92, 1),
                "B+" => e(87, 1),
                "B" => e(82, 1),
                "C+" => e(77, 1),
                "C" => e(72, 1),
                "D+" => e(67, 1),
                "D" => e(62, 1),
                _ => e(40, 1),
            },
        )
        .unwrap()
    };
    let st = GradeOutcome::Status;
    let grades = vec![
        l("A+"),
        l("A+"),
        l("A"),
        l("B+"),
        l("B"),
        l("C+"),
        l("C"),
        l("D+"),
        l("D"),
        l("F"),
        l("F"),
        st(StatusCode::DN),
        st(StatusCode::W),
        st(StatusCode::IC),
        st(StatusCode::IP),
        st(StatusCode::NP),
        st(StatusCode::NF),
        st(StatusCode::E),
    ];
    let t = tp154_a1(&grades, &s);
    // 18 students listed. A+ = 2 -> 2/18 = 11.11 %; F = 2 (DN is not an F column entry).
    assert_eq!(t.total, 18);
    assert_eq!(t.letters[0].label, "A+");
    assert_eq!(t.letters[0].arabic, "أ+");
    assert_eq!((t.letters[0].count, t.letters[0].percent), (2, e(100, 9)));
    assert_eq!(t.letters[8].label, "F");
    assert_eq!(t.letters[8].count, 2);
    let col = |n: &str| t.statuses.iter().find(|c| c.label == n).unwrap().count;
    assert_eq!(
        [
            col("Denied Entry"),
            col("In Progress"),
            col("Incomplete"),
            col("Pass"),
            col("Fail"),
            col("Withdrawn")
        ],
        [1, 1, 1, 1, 1, 1]
    );
    // E maps to no column; every listed student is in exactly one cell otherwise.
    let cells: usize = t.letters.iter().chain(&t.statuses).map(|c| c.count).sum();
    assert_eq!(cells, 17);
    let pct_sum: Exact = t.letters.iter().chain(&t.statuses).map(|c| c.percent).sum();
    assert_eq!(pct_sum, e(1700, 18));
}

#[test]
fn req_a9_frameworks_and_abet_crosswalk() {
    let nqf = OutcomeFramework::nqf_ksa();
    assert_eq!(
        nqf.outcomes
            .iter()
            .map(|o| o.code.as_str())
            .collect::<Vec<_>>(),
        ["K", "S", "V"]
    );
    let eac = OutcomeFramework::abet_eac_2026();
    assert_eq!(eac.outcomes.len(), 7);
    assert_eq!(eac.outcomes[6].code, "SO7");
    let cac = OutcomeFramework::abet_cac_2026("Apply computer science theory and fundamentals");
    assert_eq!(cac.outcomes.len(), 6);
    assert_eq!(cac.outcomes[5].code, "SO6");
    let iau = OutcomeFramework::abet_eac_2026().with_extra(
        "SO0",
        "Knowledge (= K1)",
        Some(Domain::Knowledge),
    );
    assert!(iau.outcomes[0].extra);
    assert_eq!(Domain::of_code("K2"), Some(Domain::Knowledge));
    assert_eq!(Domain::of_code("v1"), Some(Domain::Values));
    // CLO -> PLO -> SO stored once, projected for ABET.
    let clos = clo_set(&[("CLO1", Domain::Knowledge), ("CLO2", Domain::Skills)]);
    let links = vec![
        CloPloMap::new("CLO1", "K1", 3),
        CloPloMap::new("CLO2", "S1", 2),
    ];
    let xw = vec![
        Crosswalk::new("S1", "SO1", 3),
        Crosswalk::new("S1", "SO6", 1),
    ];
    let rows = crosswalk_rows(&clos, &links, &xw);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        (
            rows[0].clo.as_str(),
            rows[0].plo.as_str(),
            rows[0].clo_plo,
            rows[0].outcome.as_deref()
        ),
        ("CLO1", "K1", 'M', None)
    );
    assert_eq!(
        (
            rows[1].plo.as_str(),
            rows[1].clo_plo,
            rows[1].outcome.as_deref(),
            rows[1].plo_outcome
        ),
        ("S1", 'P', Some("SO1"), Some(3))
    );
    assert_eq!(rows[2].outcome.as_deref(), Some("SO6"));
}

#[test]
fn req_a9_outcome_exceptions_and_missing_data() {
    let (mut offering, mut clos, mut mapping) = worked_example();
    let codes = |ex: &[omr_model::Exception]| ex.iter().map(|e| e.code.clone()).collect::<Vec<_>>();
    // An unmapped item (SCO-013), a CLO with no evidence (SCO-012), a draft row (SCO-032).
    clos.clos
        .push(Clo::new("CLO5", "never assessed", Domain::Values));
    mapping
        .rows
        .retain(|r| !(r.assessment == "final" && r.item == 3));
    mapping.rows[0].status = MappingStatus::Proposed;
    let ex = mapping.validate(&offering, &clos, true);
    let c = codes(&ex);
    assert!(
        c.contains(&"SCO-013".into())
            && c.contains(&"SCO-012".into())
            && c.contains(&"SCO-032".into()),
        "{c:?}"
    );
    let r = compute_attainment(
        &offering,
        &clos,
        &mapping,
        &AttainmentPolicy::default(),
        &[],
    );
    let c5 = r.clo("CLO5").unwrap();
    assert!(c5.not_assessed);
    assert_eq!(c5.value, None, "not assessed is never 0 %");
    let rc = codes(&r.exceptions);
    assert!(rc.contains(&"SCO-012".into()));
    assert!(rc.contains(&"SCO-023".into()), "N_c = 1 < 10: clo_small_n");
    assert!(
        r.exceptions
            .iter()
            .any(|e| e.code == "SCO-023" && e.sub_reason.as_deref() == Some("clo_small_n"))
    );
    assert!(rc.contains(&"SCO-014".into()), "thin evidence");
    // Split mode: an SBA item split across two CLOs (SCO-034) and a split sum ≠ 1 (SCO-011).
    let sba_run = {
        let forms = FormMap::single_form(vec![
            MasterItem::sba("Q1", 4, pts(1, 1)),
            MasterItem::sba("Q2", 4, pts(1, 1)),
        ]);
        let key = KeyVersion::new(vec![KeyEntry::single(0, 4), KeyEntry::single(0, 4)]);
        let l1 = ResponseSnapshot {
            booklets: vec![BookletL1::present(
                1,
                Some("S1"),
                FormCode(1),
                vec![L1Response::Single(0), L1Response::Blank],
            )],
        };
        score(
            &l1,
            &forms,
            &key,
            &PolicyVersion::default(),
            &Adjustments::default(),
        )
        .unwrap()
    };
    let off2 = Offering {
        assessments: vec![AssessmentEvidence::from_run("t", "Test", e(1, 1), &sba_run)],
        ..offering.clone()
    };
    let cl2 = clo_set(&[("CLO1", Domain::Knowledge), ("CLO2", Domain::Skills)]);
    let half = |i, c: &str, w| {
        let mut r = ItemCloMap::approved("t", i, c, w);
        r.bloom_item = Some(1);
        r
    };
    let split = MappingSet {
        version: 1,
        mode: MappingMode::Split,
        rows: vec![
            half(0, "CLO1", pts(1, 2)),
            half(0, "CLO2", pts(1, 2)),
            half(1, "CLO2", pts(3, 4)),
        ],
    };
    let c = codes(&split.validate(&off2, &cl2, false));
    assert!(
        c.contains(&"SCO-034".into()) && c.contains(&"SCO-011".into()),
        "{c:?}"
    );
    // Bloom: item level 1 below CLO level 3 -> SCO-015 (+ below_level_share).
    let mut cl3 = cl2.clone();
    cl3.clos[0].bloom = Some(3);
    let ex = split.validate(&off2, &cl3, false);
    assert!(
        ex.iter()
            .any(|e| e.code == "SCO-015" && e.sub_reason.is_none())
    );
    assert!(
        ex.iter()
            .any(|e| e.code == "SCO-015" && e.sub_reason.as_deref() == Some("below_level_share"))
    );
    // Retired CLO version -> SCO-011 retired_clo_version.
    let mut cl4 = cl2.clone();
    cl4.clos[1].retired = true;
    assert!(
        split
            .validate(&off2, &cl4, false)
            .iter()
            .any(|e| e.code == "SCO-011" && e.sub_reason.as_deref() == Some("retired_clo_version"))
    );
    // SCO-035: EAMU never blends; mixed scales never roll up.
    let eamu_blend = AttainmentPolicy {
        class_method: ClassMethod::Eamu,
        beta: e(7, 10),
        ..AttainmentPolicy::default()
    };
    assert_eq!(eamu_blend.validate(true)[0].code, "SCO-035");
    assert!(check_like_scales(&[policy::Scale::Proportion, policy::Scale::Levels]).is_some());
    assert!(check_like_scales(&[policy::Scale::Proportion, policy::Scale::Proportion]).is_none());
    // SCO-025: a policy version changed mid-term.
    let mut a = AttainmentPolicy {
        effective_from_term: "1447-1".into(),
        ..AttainmentPolicy::default()
    };
    let b = AttainmentPolicy {
        version: 2,
        target: e(3, 4),
        ..a.clone()
    };
    assert_eq!(mid_term_change(&a, &b, "1447-1").unwrap().code, "SCO-025");
    a.effective_from_term = "1446-2".into();
    assert!(mid_term_change(&a, &b, "1447-1").is_none());
    // SCO-030: the same assessment scored under two key versions (two campus PCs).
    let mut skew = offering.assessments[2].clone();
    skew.key_version = 2;
    skew.students.clear();
    offering.assessments.push(skew);
    let r = compute_attainment(
        &offering,
        &clos,
        &mapping,
        &AttainmentPolicy::default(),
        &[],
    );
    assert!(r.pooled_blocked);
    assert!(r.exceptions.iter().any(|e| e.code == "SCO-030"));
    assert!(
        r.clo("CLO1")
            .unwrap()
            .groups
            .iter()
            .all(|g| g.key != GroupKey::Pooled)
    );
    // The offering-level (pooled) value and verdict are withheld too, while the
    // per-section result stays available; B.1 prints no pooled number.
    let c1 = r.clo("CLO1").unwrap();
    assert_eq!((c1.direct, c1.value, c1.attained), (None, None, None));
    assert!(
        c1.groups
            .iter()
            .any(|g| g.key == GroupKey::Section("101".into()) && g.value.is_some())
    );
    let t = tp154_b1(
        &offering,
        &clos,
        &mapping,
        &[],
        &r,
        &AttainmentPolicy::default(),
        &BTreeMap::new(),
    );
    let row = &t.groups[0].rows[0];
    assert_eq!(row.clo, "CLO1");
    assert!(row.actual.contains("SCO-030"), "{}", row.actual);
    assert_eq!((row.actual_average.as_ref(), row.attained), (None, None));
}

#[test]
fn req_a9_missing_data_policy() {
    // Two assessments: Quiz (W 0.4, one item M 10 → CLO1) and Final (W 0.6, one item M 10).
    let quiz = box_run(
        &[("Q1", 10)],
        &[
            ("A", vec![10]),
            ("B", vec![10]),
            ("C", vec![10]),
            ("W", vec![10]),
            ("D", vec![10]),
        ],
    );
    let fin_forms = FormMap::single_form(vec![MasterItem::score_box("Q1", pts(10, 1))]);
    let fin_l1 = ResponseSnapshot {
        booklets: vec![
            BookletL1::present(
                1,
                Some("A"),
                FormCode(1),
                vec![L1Response::Points(pts(5, 1))],
            ),
            BookletL1::absent(2, Some("B"), FormCode(1), true), // excused: removed from A_{n,c}
            BookletL1::absent(3, Some("C"), FormCode(1), false), // unexcused: 0
            BookletL1::present(
                4,
                Some("W"),
                FormCode(1),
                vec![L1Response::Points(pts(5, 1))],
            ),
            BookletL1::present(5, Some("D"), FormCode(1), vec![L1Response::Unreadable]),
        ],
    };
    let fin = score(
        &fin_l1,
        &fin_forms,
        &KeyVersion::new(vec![KeyEntry::Open]),
        &PolicyVersion::default(),
        &Adjustments::default(),
    )
    .unwrap();
    let mut rost = roster(&["A", "B", "C", "W", "D"], "1");
    rost[3].status = RosterStatus::Withdrawn;
    let offering = Offering {
        course: "C".into(),
        term: "T".into(),
        credit_hours: 3,
        sections: vec![section("1", "main", Track::Mixed)],
        roster: rost,
        assessments: vec![
            AssessmentEvidence::from_run("quiz", "Quiz", e(4, 10), &quiz),
            AssessmentEvidence::from_run("final", "Final", e(6, 10), &fin),
        ],
    };
    let clos = clo_set(&[("CLO1", Domain::Knowledge)]);
    let mapping = map(&[("quiz", 0, "CLO1"), ("final", 0, "CLO1")]);
    let r = compute_attainment(
        &offering,
        &clos,
        &mapping,
        &AttainmentPolicy::default(),
        &[],
    );
    // A: (0.4·1 + 0.6·0.5)/1 = 0.7.
    assert_eq!(r.p("A", "CLO1"), Some(e(7, 10)));
    // B (excused final): only the quiz remains -> P = 1, coverage 0.4 < 0.5 -> out of N_c.
    assert_eq!(r.p("B", "CLO1"), Some(e(1, 1)));
    let b = r.students.iter().find(|s| s.student.0 == "B").unwrap();
    assert!(!b.included && b.coverage == e(4, 10));
    // C (unexcused final): 0 on the final -> (0.4·1 + 0.6·0)/1 = 0.4.
    assert_eq!(r.p("C", "CLO1"), Some(e(4, 10)));
    // W (withdrawn): excluded from N_c. D: pending -> provisional, SCO-033.
    assert!(
        !r.students
            .iter()
            .find(|s| s.student.0 == "W")
            .unwrap()
            .included
    );
    assert!(
        r.students
            .iter()
            .find(|s| s.student.0 == "D")
            .unwrap()
            .provisional
    );
    assert!(r.exceptions.iter().any(|e| e.code == "SCO-033"));
    let c = r.clo("CLO1").unwrap();
    assert_eq!(
        c.stats.n, 3,
        "A, C and D count; B (coverage) and W (withdrawn) do not"
    );
    assert!(c.provisional);
    // D has the quiz (1.0) and a pending final counted as 0 -> 0.4. Only A meets τ = 0.6,
    // so π = 1/3.
    assert_eq!(c.stats.proportion, Some(e(1, 3)));
    let strict = AttainmentPolicy {
        verdict_basis: VerdictBasis::CiLower,
        target: e(1, 10),
        ..AttainmentPolicy::default()
    };
    let r2 = compute_attainment(&offering, &clos, &mapping, &strict, &[]);
    // Wilson lower bound for 1/3 is 0.061 < 0.10 -> not attained on the CI basis, although
    // the point estimate 0.333 ≥ 0.10.
    assert_eq!(r2.clo("CLO1").unwrap().attained, Some(false));
    assert!(r2.clo("CLO1").unwrap().indistinguishable);
}
