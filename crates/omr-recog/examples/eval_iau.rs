//! Real-data evaluation on the IAU sheets (aggregate metrics only).
//!
//! ```sh
//! OMR_IAU_DIR=/path/to/realdata OMR_EVAL_OUT=/path/to/metrics.json \
//!   cargo run --release -p omr-recog --example eval_iau
//! ```
//! Without `OMR_IAU_DIR` it prints a notice and exits.

#[path = "../tests/support/iau_eval.rs"]
mod iau_eval;

fn main() {
    match iau_eval::evaluate() {
        Some(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        None => eprintln!("OMR_IAU_DIR is not set; nothing to evaluate"),
    }
}
