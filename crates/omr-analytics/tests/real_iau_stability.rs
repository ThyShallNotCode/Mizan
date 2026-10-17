//! Flag stability on the two real IAU exams graded with the instructors' keys (restricted
//! data, exam-confidential keys). Skips unless `OMR_IAU_REFERENCE` (the adjudicated reference
//! answers) and `OMR_IAU_KEYS` (the folder with `key_cs522.csv` and `key_arti503.csv`) are set.
//! Prints aggregate counts and timings only — never a key letter, an answer or a student. With
//! `OMR_STABILITY_OUT` set, the per-flag stability (item label, rule, share, CI ends; no option
//! letters) is written to that file for the maintainers, never to the console.
//!
//! The analysis input mirrors the app's: one master item per version and question (other
//! version's students not presented), the reference answers scored with that version's key.

use omr_analytics::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Instant;

struct Exam {
    name: &'static str,
    key_csv: &'static str,
    /// (file tag, version)
    tags: &'static [(&'static str, &'static str)],
}

const EXAMS: [Exam; 2] = [
    Exam {
        name: "cs522",
        key_csv: "key_cs522.csv",
        tags: &[("BA", "A"), ("BB", "B")],
    },
    Exam {
        name: "arti503",
        key_csv: "key_arti503.csv",
        tags: &[("AI1A", "A"), ("AI1B", "B"), ("MS1B", "B")],
    },
];

fn read_key(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let text = std::fs::read_to_string(path).expect("key CSV");
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines
        .next()
        .unwrap()
        .split(',')
        .map(|s| s.trim().to_owned())
        .collect();
    let mut cols: Vec<(String, Vec<u8>)> = header[1..header.len() - 1]
        .iter()
        .map(|h| (h.clone(), Vec::new()))
        .collect();
    for l in lines {
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        for (k, c) in cols.iter_mut().enumerate() {
            c.1.push(f[1 + k].as_bytes()[0] - b'A');
        }
    }
    cols
}

fn build(exam: &Exam, reference: &Value, keys: &[(String, Vec<u8>)]) -> AnalysisData {
    let pages: Vec<&Value> = reference
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| exam.tags.iter().any(|(t, _)| p["tag"] == *t))
        .collect();
    let n = pages.len();
    let q = keys[0].1.len();
    let versions: Vec<&str> = keys.iter().map(|(v, _)| v.as_str()).collect();
    let k = versions.len() * q;
    let mut u = vec![0.0; n * k];
    let mut choice = vec![0u32; n * k];
    let mut status = vec![CellStatus::NotPresented; n * k];
    for (e, p) in pages.iter().enumerate() {
        let tag = p["tag"].as_str().unwrap();
        let version = exam.tags.iter().find(|(t, _)| *t == tag).unwrap().1;
        let vi = versions.iter().position(|v| *v == version).unwrap();
        let answers = p["answers"].as_array().unwrap();
        for (j, answer) in answers.iter().enumerate().take(q) {
            let i = vi * q + j;
            status[i * n + e] = CellStatus::Reached;
            let a = answer.as_str().unwrap_or("BLANK");
            let bits: u32 = if a == "BLANK" || a == "UNREADABLE" {
                0
            } else {
                a.trim_start_matches("MULTI:")
                    .bytes()
                    .filter(u8::is_ascii_uppercase)
                    .map(|b| 1u32 << (b - b'A'))
                    .fold(0, |x, y| x | y)
            };
            choice[i * n + e] = bits;
            if bits == 1 << keys[vi].1[j] {
                u[i * n + e] = 1.0;
            }
        }
    }
    AnalysisData {
        items: (0..k)
            .map(|i| ItemInfo {
                master: i,
                label: format!("{}-Q{}", versions[i / q], i % q + 1),
                options: 5,
                max: 1.0,
                keyed: 1 << keys[i / q].1[i % q],
                multi_select: false,
                choice: true,
            })
            .collect(),
        n,
        x: u.clone(),
        u,
        choice,
        status,
        penalties: false,
        run_id: format!("real-{}", exam.name),
    }
}

#[test]
fn real_exams_flag_stability_aggregates() {
    let (Some(r), Some(k)) = (
        std::env::var_os("OMR_IAU_REFERENCE"),
        std::env::var_os("OMR_IAU_KEYS"),
    ) else {
        eprintln!("OMR_IAU_REFERENCE / OMR_IAU_KEYS not set: skipping");
        return;
    };
    let reference: Value = serde_json::from_slice(&std::fs::read(r).unwrap()).unwrap();
    let key_dir = PathBuf::from(k);
    let cfg = AnalyticsConfig::default();
    let mut detail = Vec::new();
    for exam in &EXAMS {
        let keys = read_key(&key_dir.join(exam.key_csv));
        let d = build(exam, &reference, &keys);
        let t0 = Instant::now();
        let sync = analyze(&d, &cfg);
        let sync_ms = t0.elapsed().as_secs_f64() * 1e3;
        let t1 = Instant::now();
        let s = stability_job(&d, &sync.flags, &cfg);
        let job_ms = t1.elapsed().as_secs_f64() * 1e3;
        let mut rep = sync.clone();
        apply_stability(&mut rep, s.clone());
        let by = |rank: u8| s.flags.iter().filter(move |f| f.rule.rank() == rank);
        let (high, low): (Vec<&FlagStability>, Vec<&FlagStability>) =
            s.flags.iter().partition(|f| !f.low_confidence);
        eprintln!(
            "{}: N = {}, {} items; rank 1–4 flags: {} (rule 1: {}, 2: {}, 3: {}, 4: {}); high \
             confidence {}, low confidence (proposed) {}; synchronous analyze {:.1} ms, \
             stability job ({:?}, {} resamples) {:.1} ms",
            exam.name,
            d.n,
            d.items.len(),
            s.flags.len(),
            by(1).count(),
            by(2).count(),
            by(3).count(),
            by(4).count(),
            high.len(),
            low.len(),
            sync_ms,
            s.method,
            s.resamples,
            job_ms
        );
        assert_eq!(
            s.flags.len(),
            rep.flags.iter().filter(|f| f.rank() <= 4).count()
        );
        // Every low-confidence flag is labelled in the report and its exception.
        for f in rep.flags.iter().filter(|f| f.rank() <= 4) {
            let st = s
                .flags
                .iter()
                .find(|x| x.item == f.item && x.rule == f.rule && x.option == f.option)
                .unwrap();
            assert_eq!(
                st.low_confidence,
                f.message.contains("low confidence (proposed)")
            );
        }
        // Deterministic: the same run gives the same shares.
        assert_eq!(stability_job(&d, &sync.flags, &cfg), s);
        detail.push(json!({
            "exam": exam.name,
            "n": d.n,
            "method": format!("{:?}", s.method),
            "job_ms": job_ms,
            "flags": s.flags.iter().map(|f| json!({
                "item": f.label,
                "rule": format!("{:?}", f.rule),
                "share": f.share,
                "ci_ends": f.ci_ends,
                "low_confidence": f.low_confidence,
                "p": sync.items[f.item].p,
                "r_ir": sync.items[f.item].r_ir,
                "r_ir_ci": sync.items[f.item].r_ir_ci,
            })).collect::<Vec<_>>(),
        }));
    }
    if let Some(out) = std::env::var_os("OMR_STABILITY_OUT") {
        std::fs::write(out, serde_json::to_vec_pretty(&detail).unwrap()).unwrap();
    }
}
