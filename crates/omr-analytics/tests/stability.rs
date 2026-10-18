//! Flag stability oracles (spec §12.2 "Flag stability", §12.3 "Bootstrap"): a genuine
//! mis-key is a high-stability SCO-001; one high scorer choosing a distractor on an item nearly
//! everyone answers correctly is low confidence *(proposed)* and no longer an SCO-001 ERROR;
//! the job is seeded from the data's content (never the run's version history) and
//! deterministic; the bootstrap gives every item's percentile CI of r_ir; N ≥ 1,000 uses the
//! Fisher-z rule; the synchronous path never bootstraps. Synthetic classes only.

use omr_analytics::*;
use omr_scoring::stats::SplitMix64;

/// A class of `n` examinees on `k` four-option items (key A = option 0) whose answers follow a
/// latent ability; item `swapped` is mis-keyed: the class's correct answer is C (option 2), and
/// the key says A, the item's attractive distractor (most wrong answers).
fn class(n: usize, k: usize, swapped: Option<usize>, seed: u64) -> AnalysisData {
    let mut rng = SplitMix64::new(seed);
    let answers: Vec<Vec<Option<u8>>> = (0..n)
        .map(|_| {
            let ability = 25 + rng.below(70); // 25–94 % chance of knowing an item
            (0..k)
                .map(|i| {
                    let correct = if Some(i) == swapped { 2 } else { 0 };
                    if rng.below(100) < ability {
                        Some(correct)
                    } else if Some(i) == swapped && rng.below(100) < 75 {
                        Some(0)
                    } else {
                        // A wrong guess among the other three options.
                        let mut o = rng.below(3) as u8;
                        if o >= correct {
                            o += 1;
                        }
                        Some(o)
                    }
                })
                .collect()
        })
        .collect();
    AnalysisData::from_choices(&vec![4; k], &vec![0; k], &answers)
}

fn miskey_flag(rep: &AnalyticsReport, item: usize) -> &ItemFlag {
    rep.flags
        .iter()
        .find(|f| f.item == item && f.rule == Rule::ProbableMiskey)
        .expect("rule 1 fires")
}

#[test]
fn genuine_miskey_is_a_high_stability_sco001() {
    let d = class(150, 25, Some(7), 11);
    let cfg = AnalyticsConfig::default();
    let sync = analyze(&d, &cfg);
    assert!(
        sync.stability.is_none(),
        "the synchronous path never bootstraps"
    );
    assert!(sync.flags.iter().all(|f| f.stability.is_none()));
    let f = miskey_flag(&sync, 7);
    assert_eq!(f.option, Some(2));
    let rep = analyze_with_stability(&d, &cfg);
    let s = rep.stability.as_ref().unwrap();
    assert_eq!(s.method, StabilityMethod::Bootstrap);
    assert_eq!(s.resamples, 1000);
    let st = s
        .flags
        .iter()
        .find(|s| s.item == 7 && s.rule == Rule::ProbableMiskey)
        .unwrap();
    assert!(st.share.unwrap() > 0.99, "{st:?}");
    assert_eq!(st.ci_ends, Some((true, true)), "{st:?}");
    assert!(!st.low_confidence);
    let f = miskey_flag(&rep, 7);
    assert_eq!(f.confidence, Confidence::Normal);
    assert!(f.stability.unwrap() > 0.99);
    assert!(!f.message.contains("low confidence"));
    let e = rep.exceptions.iter().find(|e| e.code == "SCO-001").unwrap();
    assert!(!e.detail.contains("low confidence"), "{}", e.detail);
}

/// The F2 case: 97 % of the class answer the item correctly, and the one examinee who chose a
/// distractor is the top scorer, so r_ir(key) < 0 and r_d > 0.10 fire rule 1 — on the strength
/// of one person.
fn lone_top_scorer() -> AnalysisData {
    let n = 33;
    let k = 20;
    let mut rng = SplitMix64::new(5);
    let mut answers: Vec<Vec<Option<u8>>> = (0..n)
        .map(|e| {
            // Examinee 0 is the top scorer (knows everything); the others vary.
            let ability = if e == 0 { 100 } else { 30 + rng.below(55) };
            (0..k)
                .map(|_| {
                    if rng.below(100) < ability {
                        Some(0)
                    } else {
                        Some(1 + rng.below(3) as u8)
                    }
                })
                .collect()
        })
        .collect();
    // Item 4: everyone but the top scorer picks the key; the top scorer picks D.
    for (e, row) in answers.iter_mut().enumerate() {
        row[4] = Some(if e == 0 { 3 } else { 0 });
    }
    AnalysisData::from_choices(&vec![4; k], &vec![0; k], &answers)
}

#[test]
fn lone_high_scorer_on_an_easy_item_is_low_confidence() {
    let d = lone_top_scorer();
    let cfg = AnalyticsConfig::default();
    let sync = analyze(&d, &cfg);
    let item = &sync.items[4];
    assert!((item.p.unwrap() - 32.0 / 33.0).abs() < 1e-12);
    assert!(item.r_ir.unwrap() < 0.0);
    let (lo, hi) = item.r_ir_ci.unwrap();
    assert!(lo < 0.0 && hi > 0.0, "the CI crosses 0: [{lo}, {hi}]");
    assert_eq!(miskey_flag(&sync, 4).confidence, Confidence::Normal);
    let rep = analyze_with_stability(&d, &cfg);
    let st = rep
        .stability
        .as_ref()
        .unwrap()
        .flags
        .iter()
        .find(|s| s.item == 4 && s.rule == Rule::ProbableMiskey)
        .unwrap()
        .clone();
    assert!(st.low_confidence, "{st:?}");
    // The rule fails at the upper end of the key's CI, and fires in well under all resamples
    // (the top scorer is missing from ≈ 37 % of them).
    assert_eq!(st.ci_ends.map(|e| e.1), Some(false), "{st:?}");
    assert!(st.share.unwrap() < 0.7, "{st:?}");
    let f = miskey_flag(&rep, 4);
    assert_eq!(f.confidence, Confidence::Low);
    assert!(
        f.message.contains("low confidence (proposed)"),
        "{}",
        f.message
    );
    // C1: a proposed (low-confidence) mis-key is a non-blocking SCO-002 WARN, never the
    // SCO-001 ERROR that blocks release.
    assert!(
        !rep.exceptions
            .iter()
            .any(|e| e.code == "SCO-001" && e.target.question == Some(4))
    );
    let e = rep
        .exceptions
        .iter()
        .find(|e| e.code == "SCO-002" && e.target.question == Some(4))
        .unwrap();
    assert_eq!(e.severity, omr_model::Severity::Warn);
    assert_eq!(e.sub_reason.as_deref(), Some(PROPOSED_MISKEY));
    assert!(
        e.detail.contains("low confidence (proposed)"),
        "{}",
        e.detail
    );
}

#[test]
fn stability_is_seeded_from_the_content_and_deterministic() {
    let mut d = class(120, 15, Some(3), 21);
    let cfg = AnalyticsConfig::default();
    let flags = analyze(&d, &cfg).flags;
    let a = stability_job(&d, &flags, &cfg);
    let b = stability_job(&d, &flags, &cfg);
    assert_eq!(a, b, "same run, same shares");
    assert_eq!(a.seed, content_seed(&d));
    // Applying twice does not stack labels.
    let mut f1 = flags.clone();
    apply_to_flags(&mut f1, &a);
    let once = f1.clone();
    apply_to_flags(&mut f1, &a);
    assert_eq!(f1, once);
    // C8: the same responses and key reached through another key history (another score-run
    // ID: another key version number) draw the same resamples, so the shares and labels are
    // identical.
    d.run_id = "another-run-key-version-2".into();
    let c = stability_job(&d, &flags, &cfg);
    assert_eq!(c.seed, a.seed);
    assert_eq!(c.flags, a.flags);
    assert_eq!(c.item_ci, a.item_ci);
    // Other content (one examinee's answer changed) is another seed.
    let mut e = d.clone();
    let cell = e.idx(0, 0);
    e.choice[cell] = if e.choice[cell] == 1 { 2 } else { 1 };
    assert_ne!(content_seed(&e), a.seed);
}

#[test]
fn the_bootstrap_gives_every_item_a_percentile_ci_of_r_ir() {
    // §12.3: below N = 1,000 the job's resamples give each item's percentile 95 % CI of r_ir,
    // shown beside the synchronous Fisher-z CI.
    let d = class(80, 12, Some(4), 9);
    let cfg = AnalyticsConfig::default();
    let rep = analyze_with_stability(&d, &cfg);
    let s = rep.stability.as_ref().unwrap();
    assert_eq!(s.item_ci.len(), 12);
    for (it, ci) in rep.items.iter().zip(&s.item_ci) {
        assert_eq!(ci.item, it.item);
        assert_eq!(ci.defined, 1000);
        let (lo, hi) = ci.ci.expect("defined in every resample");
        let r = it.r_ir.unwrap();
        assert!(
            lo < hi && lo - 0.05 <= r && r <= hi + 0.05,
            "{r} vs [{lo}, {hi}]"
        );
        assert_eq!(s.r_ir_ci(it.item), Some((lo, hi)));
    }
    // The mis-keyed item's CI lies below 0.
    assert!(s.r_ir_ci(4).unwrap().1 < 0.0);
    // The one-outlier item's r_ir is undefined whenever the outlier is not drawn (≈ 37 % of
    // the resamples): no percentile CI is claimed for it.
    let lone = analyze_with_stability(&lone_top_scorer(), &cfg);
    let ci = &lone.stability.as_ref().unwrap().item_ci[4];
    assert!(ci.defined < 900 && ci.ci.is_none(), "{ci:?}");
}

#[test]
fn large_classes_use_the_fisher_z_rule() {
    let d = class(1_200, 12, Some(5), 3);
    let cfg = AnalyticsConfig::default();
    let rep = analyze_with_stability(&d, &cfg);
    let s = rep.stability.as_ref().unwrap();
    assert_eq!(s.method, StabilityMethod::FisherZ);
    assert_eq!(s.resamples, 0);
    let st = s
        .flags
        .iter()
        .find(|s| s.item == 5 && s.rule == Rule::ProbableMiskey)
        .unwrap();
    assert_eq!(st.share, None);
    assert!(
        s.item_ci.is_empty() && s.seed.is_empty(),
        "no bootstrap, no seed"
    );
    assert_eq!(st.ci_ends, Some((true, true)));
    assert!(!st.low_confidence);
    assert_eq!(miskey_flag(&rep, 5).confidence, Confidence::Normal);
}
