use omr_model::{Darkness, LogitMilli};

#[test]
fn darkness_quantisation_is_clamped_and_deterministic() {
    assert_eq!(Darkness::from_f64(-0.2).0, 0);
    assert_eq!(Darkness::from_f64(1.7).0, 1024);
    assert_eq!(Darkness::from_f64(0.5).0, 512);
    assert_eq!(Darkness::from_f64(f64::NAN).0, 0);
}

#[test]
fn logit_thresholds_match_spec() {
    assert_eq!(LogitMilli::from_prob(0.999).0, 6907);
    assert!(LogitMilli::from_prob(0.999) >= LogitMilli::STANDARD_LQ);
    assert!(LogitMilli::from_prob(0.9999) >= LogitMilli::HIGH_STAKES_LQ);
}
