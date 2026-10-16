//! Own-form evaluation on semi-synthetic corpora (A1 standard slice and the hard slice).
//!
//! ```sh
//! OMR_EVAL_OUT=/path/to/ownform-synth.json \
//!   cargo run --release -p omr-recog --example eval_ownform -- [standard_sheets] [hard_sheets] [seed]
//! ```
//! Defaults: 500 standard sheets (50,000 questions), 300 hard sheets, seed 20_261_016 (disjoint
//! from the calibration seeds 1–9 and the committed test seeds). Prints the JSON report and
//! writes it to `OMR_EVAL_OUT` when set. Synthetic data only.

#[path = "../tests/support/ownform_eval.rs"]
mod ownform_eval;

use omr_sim::Profile;
use ownform_eval::{EvalSpec, evaluate};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let std_sheets = arg(0, 500) as u32;
    let hard_sheets = arg(1, 300) as u32;
    let seed = arg(2, 20_261_016);
    let mut out = serde_json::Map::new();
    for (profile, sheets) in [
        (Profile::Standard, std_sheets),
        (Profile::Hard, hard_sheets),
    ] {
        if sheets == 0 {
            continue;
        }
        let mut spec = EvalSpec::new(profile, sheets, seed);
        spec.timing_pages = 60.min(sheets as usize);
        eprintln!("evaluating {} sheets, profile {}", sheets, profile.name());
        out.insert(profile.name().to_owned(), evaluate(&spec));
    }
    let report = json!({
        "spec": "§8.2–§8.4 own-form path; A1 (SER/AR) on omr-sim corpora",
        "profiles": serde_json::Value::Object(out),
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Ok(path) = std::env::var("OMR_EVAL_OUT") {
        if let Some(dir) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(dir).expect("create output directory");
        }
        std::fs::write(&path, &text).expect("write report");
        eprintln!("wrote {path}");
    }
    println!("{text}");
}
