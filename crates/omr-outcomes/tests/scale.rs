//! Recompute cost of the CLO engine at the A4 scale (10,000 students × 100 items, 6 CLOs,
//! 10 sections over 2 campuses and 2 tracks). Spec §11 targets ≤ 50 ms for a weight or
//! mapping edit; this test reports the measured time and guards against regressions.

use omr_model::{FormCode, StudentKey};
use omr_outcomes::*;
use omr_scoring::stats::SplitMix64;
use omr_scoring::*;
use std::time::{Duration, Instant};

#[test]
fn clo_recompute_10k_x100_6_clos() {
    let (n, k) = (10_000usize, 100usize);
    let mut rng = SplitMix64::new(5);
    let forms = FormMap::single_form(
        (0..k)
            .map(|i| MasterItem::sba(format!("Q{}", i + 1), 5, pts(1, 1)))
            .collect(),
    );
    let key = KeyVersion::new((0..k).map(|_| KeyEntry::single(0, 5)).collect());
    let l1 = ResponseSnapshot {
        booklets: (0..n)
            .map(|s| {
                BookletL1::present(
                    s as u32 + 1,
                    Some(&format!("S{s:05}")),
                    FormCode(1),
                    (0..k)
                        .map(|_| {
                            L1Response::Single(if rng.below(100) < 70 {
                                0
                            } else {
                                rng.below(5) as u8
                            })
                        })
                        .collect(),
                )
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
    let ev = AssessmentEvidence::from_run("final", "Final", exact(1, 1), &run);
    let offering = Offering {
        course: "C".into(),
        term: "T".into(),
        credit_hours: 3,
        sections: (0..10)
            .map(|s| SectionInfo {
                id: format!("{s}"),
                campus: if s < 5 { "main".into() } else { "B01".into() },
                track: if s % 2 == 0 {
                    Track::Male
                } else {
                    Track::Female
                },
            })
            .collect(),
        roster: (0..n)
            .map(|s| RosterEntry {
                student: StudentKey(format!("S{s:05}")),
                section: format!("{}", s % 10),
                status: RosterStatus::Enrolled,
                passing: Some(true),
            })
            .collect(),
        assessments: vec![ev],
    };
    let clos = CloSet {
        course: "C".into(),
        version: 1,
        clos: (1..=6)
            .map(|c| Clo::new(&format!("CLO{c}"), "x", Domain::Skills))
            .collect(),
    };
    let mapping = MappingSet {
        version: 1,
        mode: MappingMode::Full,
        rows: (0..k)
            .map(|i| ItemCloMap::approved("final", i, &format!("CLO{}", i % 6 + 1), pts(1, 1)))
            .collect(),
    };
    let policy = AttainmentPolicy::default();
    let mut best = Duration::MAX;
    let mut result = None;
    for _ in 0..3 {
        let t = Instant::now();
        let r = compute_attainment(&offering, &clos, &mapping, &policy, &[]);
        best = best.min(t.elapsed());
        result = Some(r);
    }
    let r = result.unwrap();
    assert_eq!(r.clos.len(), 6);
    assert!(r.clos.iter().all(|c| c.stats.n == n));
    // 15 report groups per CLO: pooled, 10 sections, 2 campuses, 2 tracks.
    assert!(r.clos.iter().all(|c| c.groups.len() == 15));
    eprintln!(
        "CLO recompute (10k × 100, 6 CLOs, 15 groups each): best of 3 = {:.1} ms (spec target 50 ms)",
        best.as_secs_f64() * 1e3
    );
    if !cfg!(debug_assertions) {
        assert!(
            best < Duration::from_millis(500),
            "CLO recompute regressed: {best:?}"
        );
    }
}
