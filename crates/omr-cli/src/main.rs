//! `omr`: headless command-line front end (spec §4.3, §16.4).
#![forbid(unsafe_code)]

fn main() {
    println!("omr {}", env!("CARGO_PKG_VERSION"));
}
