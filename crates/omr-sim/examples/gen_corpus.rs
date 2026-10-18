//! Generates a corpus and reports throughput:
//! `cargo run --release -p omr-sim --example gen_corpus -- <out> [sheets] [profile] [dpi] [threads]`.

use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId};
use omr_sim::{CorpusConfig, MarkRates, Profile, write_corpus};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(a.get(1).cloned().unwrap_or_else(|| "corpus".into()));
    let sheets: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let profile = a
        .get(3)
        .and_then(|s| Profile::parse(s))
        .unwrap_or(Profile::Standard);
    let dpi: u32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(200);
    let threads: usize = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(4);

    let mut spec = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_5555_6666_7777_8888_9999_aaaa_bbbb,
        )),
        100,
        5,
        CancelMode::EraseOnly,
    );
    spec.title = "CHEM 101 Final Examination".into();
    spec.course = "CHEM 101".into();
    spec.forms = 4;
    let t = Template::build(spec)?;
    let key = ExamKey::from_bytes([0x11; 32]);
    let roster: Vec<String> = (0..300)
        .map(|i| format!("44{:07}", 1_000 + i * 37))
        .collect();
    let cfg = CorpusConfig {
        sheets,
        profile,
        seed: 2026,
        dpi,
        format: omr_sim::ImageFormat::Jpeg,
        roster,
        rates: match profile {
            Profile::Clean => MarkRates::clean(),
            Profile::Standard => MarkRates::standard(),
            Profile::Hard => MarkRates::hard(),
        },
        serial_start: 1,
        threads,
    };
    let t0 = Instant::now();
    let s = write_corpus(&t, &key, &cfg, &out)?;
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "{} sheets / {} pages ({} profile, {dpi} dpi, {threads} threads) in {dt:.2} s = {:.1} pages/s, {:.1} MB",
        s.sheets,
        s.pages,
        profile.name(),
        s.pages as f64 / dt,
        s.image_bytes as f64 / 1e6
    );
    Ok(())
}
