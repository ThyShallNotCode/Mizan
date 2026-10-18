//! Timing of one booklet decode against large synthetic rosters (run with
//! `cargo test --release -p omr-identity --test perf_identity -- --ignored --nocapture`).

use omr_identity::*;
use omr_model::{ColumnState, IdColumnRead, IdGridRead, LogitMilli};
use std::time::Instant;

fn grid(p: &str) -> IdGridRead {
    IdGridRead {
        columns: p
            .bytes()
            .enumerate()
            .map(|(i, b)| IdColumnRead {
                column: i as u8,
                state: ColumnState::Single,
                marked: vec![b - b'0'],
                logits: (0..10u8)
                    .map(|k| LogitMilli(if k == b - b'0' { 9000 } else { -9000 }))
                    .collect(),
            })
            .collect(),
    }
}

#[test]
#[ignore]
fn decode_time_per_booklet() {
    let scheme = StudentIdScheme::iau();
    for n in [300usize, 1000, 5000] {
        let keys: Vec<String> = (0..n as u64)
            .map(|i| format!("22{}{:07}", 1 + i % 2, (i * 7919 + 13) % 10_000_000))
            .collect();
        let recs = keys
            .iter()
            .map(|k| RosterRecord {
                student_key: k.clone(),
                name: "S".into(),
                ..Default::default()
            })
            .collect();
        let (r, _) = RosterSnapshot::build(recs, &scheme, &ImportOptions::default()).unwrap();
        for typed in [false, true] {
            let reps = 20;
            let t0 = Instant::now();
            for k in keys.iter().take(reps) {
                let ev = BookletEvidence {
                    grid: Some(grid(k)),
                    typed: typed.then(|| TypedId {
                        text: k.clone(),
                        source: TypedSource::Ocr,
                    }),
                    ..Default::default()
                };
                let d = decode_booklet(
                    &ev,
                    &r,
                    &scheme,
                    &PriorContext::default(),
                    &IdentityPolicy::default(),
                );
                assert!(d.student.is_some());
            }
            let ms = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
            eprintln!("roster {n:5} typed {typed:5}: {ms:.2} ms per booklet");
        }
    }
}
