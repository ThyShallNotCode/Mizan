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
