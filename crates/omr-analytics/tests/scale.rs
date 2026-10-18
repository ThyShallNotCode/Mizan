//! Synchronous analytics cost at 10,000 examinees × 100 items (spec §12: point statistics
//! and analytic CIs est. < 10 ms, inside the 50 ms key-change budget). Reported, with a
//! loose regression guard in release builds.

use omr_analytics::*;
use omr_scoring::stats::SplitMix64;
use std::time::{Duration, Instant};

#[test]
fn analytics_10k_x100_synchronous_path() {
    let (n, k) = (10_000usize, 100usize);
    let mut rng = SplitMix64::new(9);
    let keys: Vec<u8> = (0..k).map(|_| rng.below(4) as u8).collect();
    let answers: Vec<Vec<Option<u8>>> = (0..n)
        .map(|_| {
            let ability = 30 + rng.below(65);
            keys.iter()
                .map(|&kk| {
                    let r = rng.below(100);
                    if r < ability {
                        Some(kk)
                    } else if r < 97 {
                        Some(rng.below(4) as u8)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .collect();
    let d = AnalysisData::from_choices(&vec![4; k], &keys, &answers);
    let mut best = Duration::MAX;
    let mut rep = None;
    for _ in 0..3 {
        let t = Instant::now();
        let r = analyze(&d, &AnalyticsConfig::default());
        best = best.min(t.elapsed());
        rep = Some(r);
    }
    let rep = rep.unwrap();
    assert_eq!(rep.items.len(), k);
    assert!(rep.tests[0].alpha.unwrap() > 0.8);
    eprintln!(
        "analytics 10k × 100 synchronous path: best of 3 = {:.1} ms",
        best.as_secs_f64() * 1e3
    );
    if !cfg!(debug_assertions) {
        assert!(
            best < Duration::from_millis(2000),
            "analytics regressed: {best:?}"
        );
    }
}

/// The flag-stability job (§12.3: a background job, never on the synchronous path) at
/// N = 300 × k = 100: 1,000 seeded resamples recomputing only the flagged items. Reported, with
/// a loose regression guard in release builds; also with every item flagged (the worst case).
#[test]
fn stability_job_300_x100_background_cost() {
    let (n, k) = (300usize, 100usize);
    let mut rng = SplitMix64::new(17);
    let keys: Vec<u8> = (0..k).map(|_| rng.below(4) as u8).collect();
    let answers: Vec<Vec<Option<u8>>> = (0..n)
        .map(|_| {
            let ability = 30 + rng.below(65);
            keys.iter()
                .enumerate()
                .map(|(i, &kk)| {
                    let r = rng.below(100);
                    if r < ability {
                        // Items 10 and 40 are mis-keyed: the class knows the next option.
                        Some(if i == 10 || i == 40 { (kk + 1) % 4 } else { kk })
                    } else if r < 97 {
                        Some(rng.below(4) as u8)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .collect();
    let d = AnalysisData::from_choices(&vec![4; k], &keys, &answers);
    let cfg = AnalyticsConfig::default();
    let rep = analyze(&d, &cfg);
    let ranked = rep.flags.iter().filter(|f| f.rank() <= 4).count();
    assert!(ranked >= 2, "the two mis-keys are flagged");
    let t = Instant::now();
    let s = stability_job(&d, &rep.flags, &cfg);
    let dt = t.elapsed();
    assert_eq!(s.resamples, 1000);
    // Worst case: every item carries a rank-2 flag.
    let all: Vec<ItemFlag> = (0..k)
        .map(|i| ItemFlag {
            item: i,
            master: i,
            label: format!("Q{}", i + 1),
            rule: Rule::TopGroupPrefers,
            option: Some((keys[i] + 1) % 4),
            message: String::new(),
            confidence: Confidence::Normal,
            stability: None,
        })
        .collect();
    let t = Instant::now();
    let w = stability_job(&d, &all, &cfg);
    let dt_all = t.elapsed();
    assert_eq!(w.flags.len(), k);
    eprintln!(
        "stability job N = 300 × k = 100, 1,000 resamples: {:.1} ms for {ranked} rank 1–4 flags, \
         {:.1} ms with all {k} items flagged",
        dt.as_secs_f64() * 1e3,
        dt_all.as_secs_f64() * 1e3
    );
    if !cfg!(debug_assertions) {
        assert!(
            dt < Duration::from_secs(5),
            "stability job regressed: {dt:?}"
        );
    }
}
