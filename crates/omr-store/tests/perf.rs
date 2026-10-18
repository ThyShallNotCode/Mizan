//! Throughput probe of the bulk lane and of chain verification (run with
//! `cargo test -p omr-store --release --test perf -- --ignored --nocapture`).

mod common;

use common::*;
use omr_model::{LogitMilli, QuestionOutcome, Response};
use omr_store::{Actor, BatchRecord, PageRecognition, RecognitionRunRecord, RunKind};
use serde_json::json;
use std::time::Instant;

#[test]
#[ignore = "timing probe"]
fn bulk_lane_pages_per_second() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    let me = instructor();
    p.decision(|tx| {
        tx.create_batch(
            &BatchRecord {
                batch_id: "b".into(),
                exam_id: exam_id(),
                header: None,
                campus: None,
                operator: None,
                device_profile: None,
                expected_count: None,
                state: "open".into(),
            },
            &me,
        )?;
        tx.create_recognition_run(&RecognitionRunRecord {
            run_id: "r".into(),
            exam_id: exam_id(),
            kind: RunKind::Provisional,
            parent_run: None,
            engine_manifest: Default::default(),
        })?;
        Ok(())
    })
    .unwrap();
    let pages = 500usize;
    let t0 = Instant::now();
    for g in 0..pages / 50 {
        let staged: Vec<_> = (0..50)
            .map(|k| {
                p.objects()
                    .stage_bytes(format!("page {g} {k}").as_bytes())
                    .unwrap()
            })
            .collect();
        p.bulk(|tx| {
            for (k, s) in staged.iter().enumerate() {
                let id = format!("p{g}-{k}");
                tx.spool_page("b", &id, (g * 50 + k) as i64, s, &Actor::engine("ingest"))?;
                tx.record_page_recognition(
                    "r",
                    &PageRecognition {
                        page_id: id,
                        registration: json!({"matched": 640}),
                        questions: (0..100)
                            .map(|q| QuestionOutcome {
                                question: q,
                                response: Response::Single((q % 5) as u8),
                                auto_accepted: true,
                                logit: LogitMilli(9000),
                                exceptions: vec![],
                            })
                            .collect(),
                        exceptions: vec![],
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    }
    let ingest = t0.elapsed();
    let t1 = Instant::now();
    let rep = p.verify_chain(None, &[]).unwrap();
    let verify = t1.elapsed();
    eprintln!(
        "bulk lane: {pages} pages × 100 questions in {:.0} ms ({:.0} pages/s); verify {} events in {:.0} ms",
        ingest.as_secs_f64() * 1e3,
        pages as f64 / ingest.as_secs_f64(),
        rep.events,
        verify.as_secs_f64() * 1e3
    );
}
