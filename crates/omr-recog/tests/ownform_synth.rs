//! Requirement A1 on the own-form path: semi-synthetic sheets from `omr-sim` (truth by
//! construction), JPEG at the sampled quality, decoded, registered from the tags, identified by
//! the page code and decided by the shared core. Synthetic data only.
//!
//! The small corpora run on every `cargo test`. The A1 proof runs (500 sheets per profile,
//! 50,000 questions each, on the held-out seed) are `#[ignore]`d because they take about a
//! minute in release; `scripts/acceptance.sh` runs them with `--include-ignored` and collects
//! the aggregates they write to `$OMR_ACCEPT_OUT`.

#[path = "support/ownform_eval.rs"]
mod ownform_eval;

use omr_sim::Profile;
use ownform_eval::{EvalSpec, evaluate};
use serde_json::Value;

/// The held-out corpus seed: thresholds were chosen on seeds 1 and 3, the committed small
/// tests use 7_001–7_003, so this seed has never been looked at while tuning.
const HELD_OUT_SEED: u64 = 20_261_016;

/// Writes the aggregates of an A1 run to `$OMR_ACCEPT_OUT/<name>.json` (acceptance script).
fn report(name: &str, v: &Value) {
    let Ok(dir) = std::env::var("OMR_ACCEPT_OUT") else {
        return;
    };
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("features_by_class"); // calibration detail, not evidence
    }
    std::fs::create_dir_all(&dir).expect("create OMR_ACCEPT_OUT");
    std::fs::write(
        std::path::Path::new(&dir).join(format!("{name}.json")),
        serde_json::to_vec_pretty(&v).expect("json"),
    )
    .expect("write the A1 report");
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn check_registration(v: &Value) {
    let r = &v["registration"];
    assert_eq!(r["failed_pages"], 0, "{r:#}");
    assert_eq!(r["code_not_verified"], 0, "{r:#}");
    assert_eq!(r["wrong_page_or_payload"], 0, "{r:#}");
    assert_eq!(r["pages_not_auto_gradable"], 0, "{r:#}");
    assert!(
        r["true_centre_error_px_max"].as_f64().unwrap() < 0.5,
        "{r:#}"
    );
}

#[test]
fn req_a1_ownform_standard_slice_zero_silent_errors() {
    let mut spec = EvalSpec::new(Profile::Standard, 30, 7_001);
    spec.threads = threads();
    let v = evaluate(&spec);
    check_registration(&v);
    for mode in ["pooled_batch", "single_page_prior"] {
        let d = &v["decisions"][mode];
        assert_eq!(d["questions"], 3_000, "{mode}");
        assert_eq!(d["silent_errors"], 0, "{mode}: {d:#}");
        assert_eq!(d["auto_accepted_on_review_truth"], 0, "{mode}: {d:#}");
        assert!(d["ar"].as_f64().unwrap() <= 0.01, "{mode}: {d:#}");
        assert_eq!(d["student_id"]["wrong_unflagged"], 0, "{mode}: {d:#}");
        assert!(
            d["student_id"]["exact_confident"].as_u64().unwrap() >= 29,
            "{mode}: {d:#}"
        );
    }
}

/// A1 proof: the standard slice on ≥ 50,000 held-out questions (500 sheets × 100 questions,
/// 200-dpi JPEG, two forms), 0 silent errors and AR ≤ 1 % in both baseline modes. SER is per
/// question (§23.1) with the one-sided 95 % bound 3/n at k = 0.
#[test]
#[ignore = "A1 long run (about a minute in release); scripts/acceptance.sh runs it"]
fn req_a1_ownform_standard_slice_50k_held_out_questions() {
    let mut spec = EvalSpec::new(Profile::Standard, 500, HELD_OUT_SEED);
    spec.threads = threads();
    spec.timing_pages = 60;
    let v = evaluate(&spec);
    report("a1-ownform-standard", &v);
    check_registration(&v);
    for mode in ["pooled_batch", "single_page_prior"] {
        let d = &v["decisions"][mode];
        let n = d["questions"].as_u64().unwrap();
        assert!(n >= 50_000, "{mode}: {n} questions");
        assert_eq!(d["silent_errors"], 0, "{mode}: {d:#}");
        assert_eq!(d["auto_accepted_on_review_truth"], 0, "{mode}: {d:#}");
        assert!(d["ar"].as_f64().unwrap() <= 0.01, "{mode}: {d:#}");
        let bound = d["ser_per_question_upper95"].as_f64().unwrap();
        assert!((bound - 3.0 / n as f64).abs() < 1e-12, "{mode}: {bound}");
        assert_eq!(d["student_id"]["wrong_unflagged"], 0, "{mode}: {d:#}");
    }
}

/// A1, reported separately: the hard slice (light marks, erasures, ticks and cross-outs,
/// strong distortion, dirty-glass streaks) on the same held-out seed. Not a certification:
/// the classical cascade cannot separate a light second fill from erasure residue where the
/// simulator's ranges overlap (README, known limitation pending the §8.7 class cascade), so
/// the guard is "at most 2 silent errors in 50,000 questions", with ticks and cross-outs
/// (§8.8 R classes) never auto-accepted.
#[test]
#[ignore = "A1 long run (about a minute in release); scripts/acceptance.sh runs it"]
fn req_a1_ownform_hard_slice_reported_separately() {
    let mut spec = EvalSpec::new(Profile::Hard, 500, HELD_OUT_SEED);
    spec.threads = threads();
    let v = evaluate(&spec);
    report("a1-ownform-hard", &v);
    check_registration(&v);
    let d = &v["decisions"]["pooled_batch"];
    assert!(d["questions"].as_u64().unwrap() >= 50_000, "{d:#}");
    assert!(d["silent_errors"].as_u64().unwrap() <= 2, "{d:#}");
    assert_eq!(d["auto_accepted_on_review_truth"], 0, "{d:#}");
    assert!(d["ar"].as_f64().unwrap() <= 0.10, "{d:#}");
    assert_eq!(d["student_id"]["wrong_unflagged"], 0, "{d:#}");
}

#[test]
fn ownform_hard_slice_registers_and_reviews_ambiguous_marks() {
    let mut spec = EvalSpec::new(Profile::Hard, 12, 7_002);
    spec.threads = threads();
    let v = evaluate(&spec);
    check_registration(&v);
    let d = &v["decisions"]["pooled_batch"];
    // The hard slice is reported, not certified: light fills as faint as erasure residue are
    // inherently ambiguous without the class cascade (README). Ticks and cross-outs, which
    // §8.8 sends to review under `erase_only`, must never be auto-accepted.
    assert!(d["ser"].as_f64().unwrap() <= 0.002, "{d:#}");
    assert_eq!(d["auto_accepted_on_review_truth"], 0, "{d:#}");
    assert!(d["ar"].as_f64().unwrap() <= 0.10, "{d:#}");
    assert_eq!(d["student_id"]["wrong_unflagged"], 0, "{d:#}");
}

#[test]
fn req_a1_ownform_two_page_booklets_route_by_page_code() {
    let mut spec = EvalSpec::new(Profile::Standard, 6, 7_003);
    spec.questions = 180;
    spec.threads = threads();
    let v = evaluate(&spec);
    assert_eq!(v["corpus"]["pages"], 12, "{:#}", v["corpus"]);
    check_registration(&v);
    let d = &v["decisions"]["pooled_batch"];
    assert_eq!(d["questions"], 6 * 180);
    assert_eq!(d["silent_errors"], 0, "{d:#}");
    assert!(d["ar"].as_f64().unwrap() <= 0.02, "{d:#}");
}
