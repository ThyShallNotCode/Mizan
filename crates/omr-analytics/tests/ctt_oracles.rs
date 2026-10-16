//! CTT and mis-key oracle tests (spec §12.1–§12.3). Expected values were hand-derived and
//! cross-checked with numpy/scipy (population variance, Pearson r, scipy.stats.f).

use omr_analytics::*;
use omr_model::FormCode;
use omr_scoring::{
    Adjustments, BookletL1, FormMap, KeyEntry, KeyVersion, L1Response, MasterItem, PolicyVersion,
    ResponseSnapshot, Section, SelectMode, Void, VoidMode, prepare, pts, score_prepared,
};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Five examinees × three dichotomous items (1 = correct):
/// E1 1 1 1 (X 3), E2 1 1 0 (2), E3 1 0 0 (1), E4 0 1 0 (1), E5 0 0 0 (0).
fn tiny() -> AnalysisData {
    let r = |v: [u8; 3]| {
        v.iter()
            .map(|&c| Some(if c == 1 { 0 } else { 1 }))
            .collect::<Vec<_>>()
    };
    AnalysisData::from_choices(
        &[4, 4, 4],
        &[0, 0, 0],
        &[
            r([1, 1, 1]),
            r([1, 1, 0]),
            r([1, 0, 0]),
            r([0, 1, 0]),
            r([0, 0, 0]),
        ],
    )
}

#[test]
fn ctt_reliability_hand_oracle() {
    let d = tiny();
    let t = &test_stats(&d)[0];
    // p = (3/5, 3/5, 1/5); X = (3, 2, 1, 1, 0): μ = 1.4, σ² = 5.2/5 = 1.04 (÷N).
    // Σpq = .24 + .24 + .16 = .64. KR-20 = 3/2·(1 − .64/1.04) = 0.576923.
    assert!(close(t.kr20.unwrap(), 0.576_923_076_923_077_2));
    // KR-21 = 3/2·(1 − 1.4·1.6/(3·1.04)) = 0.423077.
    assert!(close(t.kr21.unwrap(), 0.423_076_923_076_923_6));
    // α on {0, 1} items with population variances equals KR-20.
    assert!(close(t.alpha.unwrap(), 0.576_923_076_923_077_1));
    // SEM = √1.04·√(1 − α) = 0.663325.
    assert!(close(t.sem.unwrap(), 0.663_324_958_071_079_9));
    // Feldt CI with F(4, 8): [1 − .423077·F.975, 1 − .423077·F.025] = [−1.137652, 0.952885].
    let (lo, hi) = t.alpha_ci.unwrap();
    assert!((lo + 1.137_652_091_961_485).abs() < 1e-7 && (hi - 0.952_884_555_455_434).abs() < 1e-7);
    // α-if-deleted: 0.571429, 0.571429, 0.285714 (no item raises α by ≥ .01).
    let aid: Vec<f64> = t.alpha_if_deleted.iter().map(|a| a.unwrap()).collect();
    assert!(close(aid[0], 0.571_428_571_428_571_2) && close(aid[2], 0.285_714_285_714_285_6));
    assert!(t.alpha_rise_items.is_empty());
    assert_eq!(t.summary.mean, 1.4);
    assert_eq!(t.summary.median, 1.0);
    assert!(close(t.summary.sd.unwrap(), 1.04f64.sqrt()));
    assert_eq!(t.band, Some(AlphaBand::Below));
    assert!(t.low_confidence);
    // Item 1 corrected item–rest: u = (1,1,1,0,0), R = X − u = (2,1,0,1,0):
    // cov = .12, var u = .24, var R = .56 -> r = .12/√(.24·.56) = 0.327327.
    let r = pearson(&[1.0, 1.0, 1.0, 0.0, 0.0], &[2.0, 1.0, 0.0, 1.0, 0.0]).unwrap();
    assert!(close(r, 0.327_326_835_353_988_6));
    // Lord's CSEM at k = 50, x = 30: √(30·20/49) = 3.4993 ≈ 3.5 (§12.1 example).
    assert!((csem_lord(30.0, 50).unwrap() - 3.499_271_061_118_826).abs() < 1e-12);
    // N = 5 < 10: counts, p and option distribution only; SCO-023.
    let rep = analyze(&d, &AnalyticsConfig::default());
    assert!(
        rep.items
            .iter()
            .all(|s| s.tier == SmallN::Minimal && s.r_ir.is_none())
    );
    assert_eq!(rep.items[2].p, Some(0.2));
    assert!(rep.flags.is_empty());
    assert!(rep.exceptions.iter().any(|e| e.code == "SCO-023"));
}

/// 20 response patterns × 3 (N = 60). Items 1–4 keyed A: examinee e is right iff
/// e < 16, 14, 12, 10 (else B). Item 5 is keyed A but the strong examinees (e < 12) choose C;
/// e 12–15 choose A; e 16–19 choose B.
fn miskey_data(copies: usize) -> AnalysisData {
    let mut answers = Vec::new();
    for _ in 0..copies {
        for e in 0..20u8 {
            let mut r: Vec<Option<u8>> = [16u8, 14, 12, 10]
                .iter()
                .map(|&t| Some(if e < t { 0 } else { 1 }))
                .collect();
            r.push(Some(if e < 12 {
                2
            } else if e < 16 {
                0
            } else {
                1
            }));
            answers.push(r);
        }
    }
    AnalysisData::from_choices(&[4; 5], &[0; 5], &answers)
}

#[test]
fn ctt_item_statistics_and_ranked_miskey_rules() {
    let d = miskey_data(3);
    let rep = analyze(&d, &AnalyticsConfig::default());
    let s5 = &rep.items[4];
    assert_eq!(s5.n, 60);
    assert_eq!(s5.tier, SmallN::Moderate);
    // p = (.8, .7, .6, .5, .2).
    let ps: Vec<f64> = rep.items.iter().map(|s| s.p.unwrap()).collect();
    assert_eq!(ps, vec![0.8, 0.7, 0.6, 0.5, 0.2]);
    // r_ir (numpy): .845154, .826160, .665133, .589506, and −.338502 for the mis-keyed item.
    let rir: Vec<f64> = rep.items.iter().map(|s| s.r_ir.unwrap()).collect();
    for (got, want) in rir.iter().zip([
        0.845_154_254_728_516_4,
        0.826_159_598_709_403_1,
        0.665_133_039_913_304_6,
        0.589_506_344_746_562_7,
        -0.338_501_600_193_165_1,
    ]) {
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }
    assert_eq!(s5.discrimination, Some(Discrimination::ProbableFlaw));
    // Option r for item 5: A (key) −.338502, B −.800095, C +.929660, D n/a (never chosen).
    let r_of = |o: u8| s5.options.iter().find(|x| x.option == Some(o)).unwrap();
    assert!((r_of(2).r.unwrap() - 0.929_659_979_114_771_7).abs() < 1e-9);
    assert!((r_of(1).r.unwrap() + 0.800_094_691_365_663_4).abs() < 1e-9);
    assert_eq!(r_of(3).r, None);
    assert!(r_of(3).non_functioning);
    // Upper 27 %: ⌈60·.27⌉ = 17 -> cut at X = 4, ties included -> 30 examinees, all chose C.
    assert_eq!((r_of(2).upper, r_of(0).upper), (1.0, 0.0));
    assert!((r_of(1).lower - 2.0 / 3.0).abs() < 1e-12);
    // Trace line of C rises to 1 in the top quintiles.
    assert_eq!(r_of(2).trace[4], 1.0);
    assert_eq!(r_of(2).trace[0], 0.0);
    // D index (N ≥ 50) for item 1: upper p 1.0, lower p (18 lowest by X) = 6/18.
    let d1 = rep.items[0].d_index.unwrap();
    assert!((d1 - (1.0 - 6.0 / 18.0)).abs() < 1e-12);
    // Ranked flags for item 5: rule 1 probable mis-key C, rule 2 top group prefers C,
    // rule 3 distractor outperforms key (p_key .2 < 1/4, p_C .6, r_C > r_key), then the
    // rank-6 low discrimination and non-functioning distractor D.
    let f5: Vec<(Rule, Option<u8>)> = rep
        .flags
        .iter()
        .filter(|f| f.item == 4)
        .map(|f| (f.rule, f.option))
        .collect();
    assert_eq!(
        f5,
        vec![
            (Rule::ProbableMiskey, Some(2)),
            (Rule::TopGroupPrefers, Some(2)),
            (Rule::DistractorOutperformsKey, Some(2)),
            (Rule::LowDiscrimination, None),
            (Rule::NonFunctioningDistractor, Some(3)),
        ]
    );
    assert_eq!(rep.flags[0].rule, Rule::ProbableMiskey, "sorted by rank");
    assert!(
        rep.flags
            .iter()
            .filter(|f| f.item == 4)
            .all(|f| f.confidence == Confidence::Normal)
    );
    assert!(rep.flags[0].message.starts_with("Probable mis-key: C"));
    let sco001: Vec<_> = rep
        .exceptions
        .iter()
        .filter(|e| e.code == "SCO-001")
        .collect();
    assert_eq!(sco001.len(), 1);
    assert_eq!(sco001[0].target.question, Some(4));
    assert_eq!(
        rep.exceptions
            .iter()
            .filter(|e| e.code == "SCO-002")
            .count(),
        2
    );
    // Healthy items raise no rank 1–5 flags.
    assert!(
        rep.flags
            .iter()
            .filter(|f| f.item < 4)
            .all(|f| f.rank() == 6)
    );
}

#[test]
fn small_n_tiers_change_rule_behaviour() {
    // The same 20 patterns once (N = 20, tier 10–29): rules 1–2 read "possible —
    // insufficient data", rules 3–4 are suppressed, r_ir carries a low-confidence badge.
    let d = miskey_data(1);
    let rep = analyze(&d, &AnalyticsConfig::default());
    let s5 = &rep.items[4];
    assert_eq!(s5.tier, SmallN::Low);
    assert!(s5.low_confidence && s5.d_index.is_none());
    let f5: Vec<_> = rep.flags.iter().filter(|f| f.item == 4).collect();
    assert_eq!(f5[0].rule, Rule::ProbableMiskey);
    assert_eq!(f5[0].confidence, Confidence::InsufficientData);
    assert!(f5[0].message.ends_with("(possible — insufficient data)"));
    assert_eq!(f5[1].rule, Rule::TopGroupPrefers);
    assert!(f5.iter().all(|f| f.rule != Rule::DistractorOutperformsKey));
    assert!(rep.exceptions.iter().any(|e| e.code == "SCO-023"));
    // N = 40 (tier 30–49): D hidden; rule 3 low-confidence.
    let rep40 = analyze(&miskey_data(2), &AnalyticsConfig::default());
    assert!(rep40.items[4].d_index.is_none());
    let r3 = rep40
        .flags
        .iter()
        .find(|f| f.item == 4 && f.rule == Rule::DistractorOutperformsKey)
        .unwrap();
    assert_eq!(r3.confidence, Confidence::Low);
    assert_eq!(SmallN::of(9), SmallN::Minimal);
    assert_eq!(SmallN::of(100), SmallN::Full);
    assert_eq!(suppressed(3, 5), "< 5");
    assert_eq!(suppressed(12, 5), "12");
}

#[test]
fn flag_stability_is_seeded_and_ranks() {
    let d = miskey_data(1);
    let cfg = AnalyticsConfig::default();
    let st = item_stats(&d, cfg.upper_fraction);
    let mut flags = detect(&d, &st, &cfg);
    let mut again = flags.clone();
    flag_stability(&d, &mut flags, &cfg, 200);
    flag_stability(&d, &mut again, &cfg, 200);
    assert_eq!(flags, again, "seeded from the run ID: reproducible");
    let miskey = flags
        .iter()
        .find(|f| f.item == 4 && f.rule == Rule::ProbableMiskey)
        .unwrap();
    assert!(
        miskey.stability.unwrap() > 0.9,
        "a strong mis-key is stable"
    );
}

#[test]
fn analytics_from_score_run_with_forms_voids_and_not_reached() {
    // Six SBA items (key A) plus a select-all item keyed {A, B} that is really {A, C}.
    let mut items: Vec<MasterItem> = (1..=6)
        .map(|i| MasterItem::sba(format!("Q{i}"), 4, pts(1, 1)))
        .collect();
    items.push(MasterItem::multi_select(
        "Q7",
        4,
        SelectMode::SelectAll,
        pts(2, 1),
    ));
    let mut forms = FormMap::single_form(items.clone());
    let mut form_b = omr_scoring::Form::identity(FormCode(2), &items);
    form_b.positions.reverse();
    forms.forms.push(form_b);
    let mut entries: Vec<KeyEntry> = (0..6).map(|_| KeyEntry::single(0, 4)).collect();
    entries.push(KeyEntry::set(&[0, 1]));
    let key = KeyVersion::new(entries);
    let mut booklets = Vec::new();
    for e in 0..60u32 {
        let strong = e < 30;
        let mut r: Vec<L1Response> = (0..6)
            .map(|i| {
                if e % 10 == 8 && i >= 4 {
                    L1Response::Blank // trailing blanks on form A: not reached (speeded)
                } else if strong || (e + i) % 3 == 0 {
                    L1Response::Single(0)
                } else {
                    L1Response::Single(1 + (e as u8 % 3))
                }
            })
            .collect();
        r.push(L1Response::Set(if strong {
            vec![0, 2]
        } else {
            vec![0, 1]
        }));
        let form = if e % 2 == 0 { FormCode(1) } else { FormCode(2) };
        if form == FormCode(2) {
            r.reverse();
        }
        booklets.push(BookletL1::present(e + 1, Some(&format!("S{e}")), form, r));
    }
    booklets.push(BookletL1::absent(99, Some("ABS"), FormCode(1), true));
    let l1 = ResponseSnapshot { booklets };
    let prep = prepare(&l1, &forms).unwrap();
    let policy = PolicyVersion {
        sections: vec![Section {
            name: "A".into(),
            items: (0..6).collect(),
            speeded: true,
        }],
        ..PolicyVersion::default()
    };
    let adj = Adjustments {
        voids: vec![Void {
            item: 5,
            mode: VoidMode::Drop,
            reason: "PRINT_ERROR".into(),
        }],
        ..Adjustments::default()
    };
    let run = score_prepared(&prep, &forms, &key, &policy, &adj).unwrap();
    let data = AnalysisData::from_run(&run, &prep.matrix);
    // The voided item leaves the analysis; the absent student is not an examinee.
    assert_eq!(data.items.len(), 6);
    assert_eq!(data.n, 60);
    let rep = analyze(&data, &AnalyticsConfig::default());
    // Not-reached cells (the trailing Q5–Q6 blanks of examinees 8, 18, …, 58, all on form A;
    // Q6 is voided) are missing for difficulty: Q5 is reached by 60 − 6 = 54 examinees.
    assert_eq!(rep.items[4].n, 54);
    // Multi-select mis-keyed option: keyed B has r < 0 while unkeyed C has r > .10 -> SCO-001.
    let q7 = rep.flags.iter().find(|f| f.label == "Q7").unwrap();
    assert_eq!((q7.rule, q7.option), (Rule::MiskeyedOption, Some(2)));
    assert!(rep.exceptions.iter().any(|e| e.code == "SCO-001"));
    // Q6 (master 5) is voided and leaves the analysis, so Q7 is analysis item 5 but master
    // item 6: the SCO-001 exception must point at master item 6, option C, not at Q6.
    assert_eq!((q7.item, q7.master), (5, 6));
    let sco001 = rep
        .exceptions
        .iter()
        .find(|e| e.code == "SCO-001" && e.detail.starts_with("Q7"))
        .unwrap();
    assert_eq!(
        (sco001.target.question, sco001.target.bubble),
        (Some(6), Some(2))
    );
    // What-if: accept the corrected key {A, C}; the flag disappears.
    let key2 = key.revise([(6, KeyEntry::set(&[0, 2]))], "KEY_ERROR");
    let run2 = score_prepared(&prep, &forms, &key2, &policy, &adj).unwrap();
    let rep2 = analyze(
        &AnalysisData::from_run(&run2, &prep.matrix),
        &AnalyticsConfig::default(),
    );
    assert!(rep2.flags.iter().all(|f| f.label != "Q7" || f.rank() > 1));
    let q7_stats = rep2.items.iter().find(|s| s.label == "Q7").unwrap();
    assert!(q7_stats.r_ir.unwrap() > 0.3);
}

#[test]
fn negative_alpha_raises_sco018() {
    // Anti-correlated items (1,0), (0,1), (1,0), (0,1), (1,1): σ²_X = .16, σ²_i = .24 each,
    // α = 2·(1 − .48/.16) = −4: reported as is, with SCO-018 and the mis-key hint.
    let r = |a: u8, b: u8| {
        vec![
            Some(if a == 1 { 0 } else { 1 }),
            Some(if b == 1 { 0 } else { 1 }),
        ]
    };
    let d = AnalysisData::from_choices(
        &[4, 4],
        &[0, 0],
        &[r(1, 0), r(0, 1), r(1, 0), r(0, 1), r(1, 1)],
    );
    let rep = analyze(&d, &AnalyticsConfig::default());
    let a = rep.tests[0].alpha.unwrap();
    assert!(a < 0.0, "α = {a}");
    assert!(
        rep.exceptions
            .iter()
            .any(|e| e.code == "SCO-018" && e.detail.contains("negative"))
    );
}
