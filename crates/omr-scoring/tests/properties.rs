//! Property tests for scoring invariants (spec §10.1, §10.3, §10.5).

use num_traits::Signed;
use omr_model::FormCode;
use omr_scoring::num::round_exact;
use omr_scoring::*;
use proptest::prelude::*;

/// A random exam: per item (options m, key option, max points numerator over 4).
fn exam() -> impl Strategy<Value = Vec<(u8, u8, i64)>> {
    prop::collection::vec(
        (2u8..=6).prop_flat_map(|m| (Just(m), 0..m, 1i64..=8)),
        2..12,
    )
}

fn build(exam: &[(u8, u8, i64)]) -> (Vec<MasterItem>, KeyVersion) {
    let items: Vec<MasterItem> = exam
        .iter()
        .enumerate()
        .map(|(i, &(m, _, mx))| MasterItem::sba(format!("Q{i}"), m, pts(mx, 4)))
        .collect();
    let key = KeyVersion::new(
        exam.iter()
            .map(|&(m, k, _)| KeyEntry::single(k, m))
            .collect(),
    );
    (items, key)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Scoring is invariant under form permutation: the same master-space answers printed on
    /// a shuffled form (π_f, σ_f) score identically (§10.3).
    #[test]
    fn permutation_invariance(
        exam in exam(),
        seed in any::<u64>(),
        answers in prop::collection::vec(any::<u8>(), 12),
    ) {
        let (items, key) = build(&exam);
        let k = items.len();
        let mut rng = omr_scoring::stats::SplitMix64::new(seed);
        let mut order: Vec<usize> = (0..k).collect();
        for i in (1..k).rev() {
            order.swap(i, rng.below(i as u64 + 1) as usize);
        }
        let maps: Vec<Vec<u8>> = order
            .iter()
            .map(|&i| {
                let m = items[i].options;
                let mut v: Vec<u8> = (0..m).collect();
                for x in (1..m as usize).rev() {
                    v.swap(x, rng.below(x as u64 + 1) as usize);
                }
                v
            })
            .collect();
        let form_a = Form::identity(FormCode(1), &items);
        let form_b = Form::permuted(FormCode(2), &order, &maps);
        let forms = FormMap { items: items.clone(), forms: vec![form_a, form_b], per_form_normalization: false };
        prop_assert!(forms.validate().is_empty());
        // Master-space answers: option a % (m + 1), where m means blank.
        let master: Vec<Option<u8>> = items
            .iter()
            .zip(&answers)
            .map(|(it, a)| {
                let v = a % (it.options + 1);
                (v < it.options).then_some(v)
            })
            .collect();
        let resp_a: Vec<L1Response> = master
            .iter()
            .map(|o| o.map_or(L1Response::Blank, L1Response::Single))
            .collect();
        let resp_b: Vec<L1Response> = order
            .iter()
            .zip(&maps)
            .map(|(&i, sigma)| match master[i] {
                None => L1Response::Blank,
                Some(o) => {
                    let printed = sigma.iter().position(|&x| x == o).unwrap() as u8;
                    L1Response::Single(printed)
                }
            })
            .collect();
        let l1 = ResponseSnapshot {
            booklets: vec![
                BookletL1::present(1, None, FormCode(1), resp_a),
                BookletL1::present(2, None, FormCode(2), resp_b),
            ],
        };
        let run = score(&l1, &forms, &key, &PolicyVersion::default(), &Adjustments::default()).unwrap();
        for i in 0..k {
            prop_assert_eq!(run.item_score(0, i), run.item_score(1, i));
        }
        prop_assert_eq!(run.students[0].final_pct, run.students[1].final_pct);
        prop_assert_eq!(run.students[0].raw, run.students[1].raw);
    }

    /// Bounds: without penalties or bonus, 0 ≤ s_{n,i} ≤ M_i and 0 ≤ x% ≤ 100, for SBA and
    /// every multi-select rule (§10.4, §10.7 property checks).
    #[test]
    fn score_bounds(
        exam in exam(),
        rule in prop::sample::select(vec![
            MultiRule::Dichotomous, MultiRule::Mtf, MultiRule::Ps1t, MultiRule::Ps50, MultiRule::PlusMinus,
        ]),
        marks in prop::collection::vec(any::<u32>(), 12),
    ) {
        // Every odd item becomes a select-all item keyed {0, 1}.
        let mut items = Vec::new();
        let mut entries = Vec::new();
        for (i, &(m, k, mx)) in exam.iter().enumerate() {
            if i % 2 == 1 && m >= 3 {
                items.push(MasterItem::multi_select(format!("Q{i}"), m, SelectMode::SelectAll, pts(mx, 4)));
                entries.push(KeyEntry::set(&[0, 1]));
            } else {
                items.push(MasterItem::sba(format!("Q{i}"), m, pts(mx, 4)));
                entries.push(KeyEntry::single(k, m));
            }
        }
        let forms = FormMap::single_form(items.clone());
        let key = KeyVersion::new(entries);
        let mut policy = PolicyVersion::default();
        for (i, it) in items.iter().enumerate() {
            if matches!(it.kind, ItemKind::MultiSelect(_)) {
                policy.multi_rules.insert(i, rule);
            }
        }
        let resp: Vec<L1Response> = items
            .iter()
            .zip(&marks)
            .map(|(it, &mk)| {
                let mask = mk & it.option_mask();
                match it.kind {
                    ItemKind::MultiSelect(_) => L1Response::Set(options_of(mask)),
                    _ if mask == 0 => L1Response::Blank,
                    _ if mask.count_ones() == 1 => L1Response::Single(mask.trailing_zeros() as u8),
                    _ => L1Response::Multi(options_of(mask)),
                }
            })
            .collect();
        let l1 = ResponseSnapshot { booklets: vec![BookletL1::present(1, None, FormCode(1), resp)] };
        let run = score(&l1, &forms, &key, &policy, &Adjustments::default()).unwrap();
        for (i, it) in items.iter().enumerate() {
            let s = run.item_score(0, i);
            prop_assert!(s >= pts(0, 1) && s <= it.max_points, "item {i}: {s}");
        }
        let x = run.students[0].final_pct.unwrap();
        prop_assert!(x >= exact(0, 1) && x <= exact(100, 1));
    }

    /// Rescoring is idempotent and the prepared path equals the one-shot path (§10.1).
    #[test]
    fn rescoring_is_idempotent(exam in exam(), answers in prop::collection::vec(any::<u8>(), 12)) {
        let (items, key) = build(&exam);
        let forms = FormMap::single_form(items.clone());
        let resp: Vec<L1Response> = items
            .iter()
            .zip(&answers)
            .map(|(it, a)| {
                let v = a % (it.options + 1);
                if v < it.options { L1Response::Single(v) } else { L1Response::Blank }
            })
            .collect();
        let l1 = ResponseSnapshot { booklets: vec![BookletL1::present(7, Some("S"), FormCode(1), resp)] };
        let p = PolicyVersion::default();
        let a = Adjustments::default();
        let r1 = score(&l1, &forms, &key, &p, &a).unwrap();
        let r2 = score(&l1, &forms, &key, &p, &a).unwrap();
        let prep = prepare(&l1, &forms).unwrap();
        let r3 = score_prepared(&prep, &forms, &key, &p, &a).unwrap();
        prop_assert_eq!(&r1, &r2);
        prop_assert_eq!(&r1, &r3);
    }

    /// Formula scoring: blind guessing gains 0 in expectation for any m_i and a_i (§10.5).
    #[test]
    fn formula_scoring_is_fair(m in 2u8..=26, a_raw in 1u8..=25) {
        let a = 1 + a_raw % (m - 1).max(1);
        let a = a.min(m - 1);
        let accepted: Vec<u8> = (0..a).collect();
        let items = vec![MasterItem::sba("Q", m, pts(1, 1))];
        let forms = FormMap::single_form(items);
        let key = KeyVersion::new(vec![KeyEntry::any_of(&accepted, m)]);
        let policy = PolicyVersion { negative: NegativeMarking::Formula, floors: Floors::NONE, ..PolicyVersion::default() };
        let booklets = (0..m)
            .map(|o| BookletL1::present(u32::from(o) + 1, None, FormCode(1), vec![L1Response::Single(o)]))
            .collect();
        let run = score(&ResponseSnapshot { booklets }, &forms, &key, &policy, &Adjustments::default()).unwrap();
        let sum: Points = (0..m as usize).map(|n| run.item_score(n, 0)).sum();
        prop_assert_eq!(sum, pts(0, 1));
    }

    /// Exact rounding lands within half a unit (half strategies) or one unit (directed).
    #[test]
    fn rounding_is_close(n in -1_000_000i128..1_000_000, d in 1i128..10_000, dp in 0u32..5) {
        let x = exact(n, d);
        let unit = exact(1, 10i128.pow(dp));
        for s in [RoundingStrategy::MidpointAwayFromZero, RoundingStrategy::MidpointNearestEven] {
            let r = round_exact(&x, dp, s);
            let diff = if r > x { r - x } else { x - r };
            prop_assert!(diff * exact(2, 1) <= unit);
        }
        let r = round_exact(&x, dp, RoundingStrategy::ToZero);
        prop_assert!(r.abs() <= x.abs() && (x - r).abs() < unit);
    }
}
