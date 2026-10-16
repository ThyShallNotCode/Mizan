//! Requirement A1 on the own-form path, small committed corpora (the ≥ 300-sheet runs are in
//! `examples/eval_ownform.rs`): semi-synthetic sheets from `omr-sim` (truth by construction),
//! JPEG at the sampled quality, decoded, registered from the tags, identified by the page code
//! and decided by the shared core. Synthetic data only.

#[path = "support/ownform_eval.rs"]
mod ownform_eval;

use omr_sim::Profile;
use ownform_eval::{EvalSpec, evaluate};
use serde_json::Value;

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
