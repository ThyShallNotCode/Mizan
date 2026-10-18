//! Property tests for the CTT statistics (spec §12.1).

use omr_analytics::*;
use proptest::prelude::*;

fn data() -> impl Strategy<Value = (Vec<u8>, Vec<Vec<Option<u8>>>)> {
    (2usize..6).prop_flat_map(|k| {
        (
            prop::collection::vec(0u8..4, k),
            prop::collection::vec(
                prop::collection::vec(prop::option::weighted(0.9, 0u8..4), k),
                12..40,
            ),
        )
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// KR-20 equals α on dichotomous unit-weighted data; α ≤ 1; correlations lie in [−1, 1];
    /// p lies in [0, 1].
    #[test]
    fn reliability_and_correlation_bounds((keys, answers) in data()) {
        let d = AnalysisData::from_choices(&vec![4; keys.len()], &keys, &answers);
        let rep = analyze(&d, &AnalyticsConfig::default());
        let t = &rep.tests[0];
        if let (Some(a), Some(k)) = (t.alpha, t.kr20) {
            prop_assert!((a - k).abs() < 1e-9);
            prop_assert!(a <= 1.0 + 1e-12);
        }
        for s in &rep.items {
            if let Some(p) = s.p { prop_assert!((0.0..=1.0).contains(&p)); }
            for r in [s.r_ir, s.r_pb].into_iter().flatten() {
                prop_assert!((-1.0 - 1e-9..=1.0 + 1e-9).contains(&r));
            }
            let total: f64 = s.options.iter().map(|o| o.proportion).sum();
            prop_assert!(total >= 1.0 - 1e-9, "every examinee chose an option or omitted");
        }
    }

    /// Examinee order does not change p, r_ir or α (determinism, §12).
    #[test]
    fn examinee_order_invariance((keys, answers) in data(), rot in 1usize..11) {
        let d1 = AnalysisData::from_choices(&vec![4; keys.len()], &keys, &answers);
        let mut a2 = answers.clone();
        let r = rot % a2.len();
        a2.rotate_left(r);
        let d2 = AnalysisData::from_choices(&vec![4; keys.len()], &keys, &a2);
        let (s1, s2) = (item_stats(&d1, 0.27), item_stats(&d2, 0.27));
        for (x, y) in s1.iter().zip(&s2) {
            prop_assert_eq!(x.p.map(|v| (v * 1e9).round()), y.p.map(|v| (v * 1e9).round()));
            prop_assert_eq!(x.r_ir.map(|v| (v * 1e9).round()), y.r_ir.map(|v| (v * 1e9).round()));
        }
        let (t1, t2) = (test_stats(&d1), test_stats(&d2));
        prop_assert_eq!(t1[0].alpha.map(|v| (v * 1e9).round()), t2[0].alpha.map(|v| (v * 1e9).round()));
    }
}
