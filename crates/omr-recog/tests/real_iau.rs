//! Requirement A1 on real data: the owner's 143 IAU answer sheets (restricted personal data).
//!
//! Skips unless `OMR_IAU_DIR` points at the scratch directory holding `pages/` and
//! `reference.json`; nothing is copied, and only aggregate metrics are produced (written to
//! `OMR_EVAL_OUT` when set). Run the release-mode example for timings:
//! `cargo run --release -p omr-recog --example eval_iau`.

#[path = "support/iau_eval.rs"]
mod iau_eval;

#[test]
fn req_a1_real_iau_sheets_zero_silent_errors() {
    let Some(v) = iau_eval::evaluate() else {
        eprintln!("OMR_IAU_DIR is not set: skipping the real-data evaluation");
        return;
    };
    assert_eq!(v["registration"]["failed_pages"], 0, "{v:#}");
    for mode in [
        "decisions_pooled_batch",
        "decisions_per_file_batch",
        "decisions_single_page_prior",
    ] {
        let s = &v[mode];
        assert_eq!(s["silent_errors"], 0, "{mode}: {s:#}");
        assert!(s["ar"].as_f64().unwrap() <= 0.005, "{mode}: {s:#}");
        assert!(
            s["auto_accepted"].as_u64().unwrap() >= 4_080,
            "{mode}: {s:#}"
        );
        for field in ["student_number", "section", "version", "exam"] {
            assert_eq!(s[field]["wrong_unflagged"], 0, "{mode} {field}: {s:#}");
        }
    }
}
