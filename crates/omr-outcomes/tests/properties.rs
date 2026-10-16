//! Property tests for the outcome engine (spec §11.2 split-mode identity, §11.3 bounds).

use omr_model::{FormCode, StudentKey};
use omr_outcomes::mapping::evidence_share;
use omr_outcomes::*;
use omr_scoring::stats::{Z95, wilson_ci};
use omr_scoring::{
    Adjustments, BookletL1, Exact, Floors, FormMap, KeyEntry, KeyVersion, L1Response, MasterItem,
    NegativeMarking, PolicyVersion, ResponseSnapshot, RosterStatus, exact, pts, score,
};
use proptest::prelude::*;

const CLOS: [&str; 4] = ["CLO1", "CLO2", "CLO3", "CLO4"];

/// One assessment: weight (raw), items (M_i, split weights over CLOs), student scores.
type Spec = (u8, Vec<(i64, [u8; 4])>, Vec<Vec<i64>>);

fn assessment() -> impl Strategy<Value = Spec> {
    (
        1u8..=5,
        prop::collection::vec((1i64..=10, prop::array::uniform4(0u8..=3)), 1..=4),
        prop::collection::vec(prop::collection::vec(0i64..=10, 4), 3),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// §11.2: in split mode with every scored item mapped (no bonus, voids, choice groups,
    /// floors, negative scores or drops), the e-weighted mean of a student's CLO
    /// performances equals the uncurved course percentage Σ_a W_a·total_a/M_a.
    #[test]
    fn split_mode_identity(specs in prop::collection::vec(assessment(), 1..=3)) {
        let wsum: i128 = specs.iter().map(|s| i128::from(s.0)).sum();
        let mut assessments = Vec::new();
        let mut rows = Vec::new();
        let mut expected = [Exact::default(); 3];
        for (k, (w, items, scores)) in specs.iter().enumerate() {
            let id = format!("A{k}");
            let forms = FormMap::single_form(
                items.iter().enumerate().map(|(i, (m, _))| MasterItem::score_box(format!("Q{i}"), pts(*m, 1))).collect(),
            );
            let key = KeyVersion::new(items.iter().map(|_| KeyEntry::Open).collect());
            let l1 = ResponseSnapshot {
                booklets: scores
                    .iter()
                    .enumerate()
                    .map(|(n, sc)| {
                        BookletL1::present(n as u32 + 1, Some(&format!("S{n}")), FormCode(1),
                            items.iter().zip(sc).map(|((m, _), s)| L1Response::Points(pts((*s).min(*m), 1))).collect())
                    })
                    .collect(),
            };
            let run = score(&l1, &forms, &key, &PolicyVersion::default(), &Adjustments::default()).unwrap();
            let wa = exact(i128::from(*w), wsum);
            let ma: i64 = items.iter().map(|(m, _)| m).sum();
            for (n, sc) in scores.iter().enumerate() {
                let total: i64 = items.iter().zip(sc).map(|((m, _), s)| (*s).min(*m)).sum();
                expected[n] += wa * exact(i128::from(total), i128::from(ma));
            }
            assessments.push(AssessmentEvidence::from_run(&id, &id, wa, &run));
            for (i, (_, split)) in items.iter().enumerate() {
                // Split weights: raw integers (at least one positive), normalized to sum 1.
                let mut raw = *split;
                if raw.iter().all(|x| *x == 0) { raw[i % 4] = 1; }
                let s: i64 = raw.iter().map(|x| i64::from(*x)).sum();
                for (c, r) in raw.iter().enumerate() {
                    if *r > 0 {
                        rows.push(ItemCloMap::approved(&id, i, CLOS[c], pts(i64::from(*r), s)));
                    }
                }
            }
        }
        let offering = Offering {
            course: "C".into(), term: "T".into(), credit_hours: 3,
            sections: vec![SectionInfo { id: "1".into(), campus: "main".into(), track: Track::Mixed }],
            roster: (0..3).map(|n| RosterEntry { student: StudentKey(format!("S{n}")), section: "1".into(), status: RosterStatus::Enrolled, passing: None }).collect(),
            assessments,
        };
        let clos = CloSet { course: "C".into(), version: 1, clos: CLOS.iter().map(|c| Clo::new(c, c, Domain::Skills)).collect() };
        let mapping = MappingSet { version: 1, mode: MappingMode::Split, rows };
        prop_assert!(mapping.validate(&offering, &clos, false).iter().all(|e| e.code != "SCO-011"));
        let r = compute_attainment(&offering, &clos, &mapping, &AttainmentPolicy::default(), &[]);
        let shares: Vec<Exact> = CLOS.iter().map(|c| evidence_share(&mapping, &offering, c)).collect();
        prop_assert_eq!(shares.iter().copied().sum::<Exact>(), exact(1, 1));
        for (n, exp) in expected.iter().enumerate() {
            let got: Exact = CLOS
                .iter()
                .zip(&shares)
                .filter_map(|(c, e)| r.p(&format!("S{n}"), c).map(|p| e * p))
                .sum();
            prop_assert_eq!(got, *exp, "student {}", n);
        }
    }

    /// P_{n,c} ∈ [0, 1] even under formula scoring: negative item scores are clamped at 0 for
    /// CLO evidence while the grade keeps the penalty (§10.1 layer rule).
    #[test]
    fn performance_is_bounded(answers in prop::collection::vec(prop::collection::vec(0u8..=4, 6), 1..8)) {
        let forms = FormMap::single_form((0..6).map(|i| MasterItem::sba(format!("Q{i}"), 4, pts(1, 1))).collect());
        let key = KeyVersion::new((0..6).map(|_| KeyEntry::single(0, 4)).collect());
        let policy = PolicyVersion { negative: NegativeMarking::Formula, floors: Floors::NONE, ..PolicyVersion::default() };
        let l1 = ResponseSnapshot {
            booklets: answers.iter().enumerate().map(|(n, a)| BookletL1::present(n as u32 + 1, Some(&format!("S{n}")), FormCode(1),
                a.iter().map(|&o| if o == 4 { L1Response::Blank } else { L1Response::Single(o) }).collect())).collect(),
        };
        let run = score(&l1, &forms, &key, &policy, &Adjustments::default()).unwrap();
        let offering = Offering {
            course: "C".into(), term: "T".into(), credit_hours: 3, sections: vec![], roster: vec![],
            assessments: vec![AssessmentEvidence::from_run("t", "Test", exact(1, 1), &run)],
        };
        let clos = CloSet { course: "C".into(), version: 1, clos: vec![Clo::new("CLO1", "x", Domain::Knowledge), Clo::new("CLO2", "y", Domain::Skills)] };
        let mapping = MappingSet { version: 1, mode: MappingMode::Full, rows: (0..6).map(|i| ItemCloMap::approved("t", i, if i < 3 { "CLO1" } else { "CLO2" }, pts(1 + i as i64, 1))).collect() };
        let r = compute_attainment(&offering, &clos, &mapping, &AttainmentPolicy::default(), &[]);
        for s in &r.students {
            if let Some(p) = s.p {
                prop_assert!(p >= exact(0, 1) && p <= exact(1, 1), "{p}");
            }
        }
        for c in &r.clos {
            prop_assert!(c.stats.met <= c.stats.n);
            if let Some(p) = c.stats.proportion { prop_assert!(p >= exact(0, 1) && p <= exact(1, 1)); }
        }
    }

    /// Wilson intervals are within [0, 1] and contain p̂; pooled π lies between the section
    /// rates (§11.3).
    #[test]
    fn wilson_and_pooling(x1 in 0u64..50, n1 in 1u64..50, x2 in 0u64..50, n2 in 1u64..50) {
        let (x1, x2) = (x1.min(n1), x2.min(n2));
        let (lo, hi) = wilson_ci(x1, n1, Z95);
        let p = x1 as f64 / n1 as f64;
        prop_assert!((0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi));
        prop_assert!(lo <= p + 1e-12 && p <= hi + 1e-12);
        let (pooled, _) = omr_outcomes::pool_proportions(&[(x1 as usize, n1 as usize), (x2 as usize, n2 as usize)]).unwrap();
        let r1 = exact(x1 as i128, n1 as i128);
        let r2 = exact(x2 as i128, n2 as i128);
        prop_assert!(pooled >= r1.min(r2) && pooled <= r1.max(r2));
    }

    /// The integer fast path (grade-consistent, points-pooled) equals an independent exact
    /// reference on hand-built evidence with fractional M_i, scores and weights (§11.3).
    #[test]
    fn fast_path_matches_exact_reference(
        a_specs in prop::collection::vec(
            (1i128..=4, prop::collection::vec((1i64..=12, 1i64..=4, 0i64..=48, 1i64..=3), 1..=4)),
            1..=3,
        ),
        pooled in any::<bool>(),
    ) {
        // Item (M = m/q, score = min(s, m)/q... scaled by 4), w = wn/2, all to CLO1.
        let wsum: i128 = a_specs.iter().map(|a| a.0).sum();
        let mut assessments = Vec::new();
        let mut rows = Vec::new();
        let (mut ref_num, mut ref_den) = (Exact::default(), Exact::default());
        for (k, (wa, items)) in a_specs.iter().enumerate() {
            let id = format!("A{k}");
            let w_a = exact(*wa, wsum);
            let ma: Exact = items.iter().map(|(m, q, _, _)| exact(i128::from(*m), i128::from(*q))).sum();
            let ev_items: Vec<EvidenceItem> = items.iter().enumerate().map(|(i, (m, q, _, _))| EvidenceItem {
                label: format!("Q{i}"), max: pts(*m, *q), bonus: false, voided: false, indivisible: false,
            }).collect();
            let scores: Vec<ItemEvidence> = items.iter().map(|(m, q, s, _)| ItemEvidence::Counted {
                score: pts((*s).min(*m * 4), 4 * *q), attempted: true,
            }).collect();
            let (mut num, mut den) = (Exact::default(), Exact::default());
            for (i, (m, q, s, wn)) in items.iter().enumerate() {
                let w = exact(i128::from(*wn), 2);
                num += w * exact(i128::from((*s).min(*m * 4)), i128::from(4 * *q));
                den += w * exact(i128::from(*m), i128::from(*q));
                rows.push(ItemCloMap::approved(&id, i, "CLO1", pts(*wn, 2)));
            }
            let omega = if pooled { den } else { w_a * den / ma };
            ref_num += omega * num / den;
            ref_den += omega;
            assessments.push(AssessmentEvidence {
                id: id.clone(), name: id.clone(), weight: w_a, direct: true, programme_level: false,
                items: ev_items,
                students: vec![StudentEvidence { student: StudentKey("S".into()), status: EvidenceStatus::Present, provisional: false, items: scores }],
                run_id: "r".into(), key_version: 1, policy_version: 1, score_denom: None,
            });
        }
        let offering = Offering { course: "C".into(), term: "T".into(), credit_hours: 3, sections: vec![], roster: vec![], assessments };
        let clos = CloSet { course: "C".into(), version: 1, clos: vec![Clo::new("CLO1", "x", Domain::Knowledge)] };
        let mapping = MappingSet { version: 1, mode: MappingMode::Full, rows };
        let policy = AttainmentPolicy {
            student_method: if pooled { StudentMethod::PointsPooled } else { StudentMethod::GradeConsistent },
            ..AttainmentPolicy::default()
        };
        let r = compute_attainment(&offering, &clos, &mapping, &policy, &[]);
        prop_assert_eq!(r.p("S", "CLO1"), Some(ref_num / ref_den));
    }
}
