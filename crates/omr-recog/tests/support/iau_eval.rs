//! Real-data evaluation on the owner's IAU answer sheets (restricted data: read from the path in
//! `OMR_IAU_DIR`, never copied; only aggregate metrics are produced).
//!
//! `OMR_IAU_DIR` must contain `pages/<TAG>_<PP>.png` (200-dpi coverage gray), `reference.json`
//! (adjudicated labels), and optionally `measurements.json` / `proto_reads.json` (the Python
//! prototype's centres and decisions). Shared by `tests/real_iau.rs` and
//! `examples/eval_iau.rs`.

#![allow(dead_code)]

use omr_forms::legacy::LegacyField;
use omr_imaging::{CaptureKind, decode};
use omr_model::Response;
use omr_recog::{Baseline, LegacyEngine, LegacyPageResult, PageMeasurement, PageRequest};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Questions per exam, by file tag.
pub fn questions_for(tag: &str) -> u16 {
    match tag {
        "BA" | "BB" => 29,
        _ => 28,
    }
}

/// One page's identity and per-stage timings.
pub struct PageRun {
    pub tag: String,
    pub page: u32,
    pub measurement: PageMeasurement,
    pub capture: CaptureKind,
    pub decode_ms: f64,
    pub register_ms: f64,
    pub measure_ms: f64,
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn quantiles(v: &mut [f64]) -> Value {
    if v.is_empty() {
        return json!(null);
    }
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    json!({ "min": q(0.0), "median": q(0.5), "p95": q(0.95), "max": q(1.0), "mean": v.iter().sum::<f64>() / v.len() as f64 })
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn answer_text(r: &Response) -> String {
    match r {
        Response::Single(i) => char::from(b'A' + i).to_string(),
        Response::Blank => "BLANK".into(),
        Response::Multi(v) | Response::Set(v) => {
            format!(
                "MULTI:{}",
                v.iter().map(|i| char::from(b'A' + i)).collect::<String>()
            )
        }
        Response::Unreadable { .. } => "UNREADABLE".into(),
    }
}

/// Runs the page stage on every page (single thread, in file order).
pub fn run_pages(engine: &LegacyEngine, dir: &Path, repeat: usize) -> Vec<PageRun> {
    let repeat = repeat.max(1);
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("pages"))
        .expect("pages directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|f| {
            let stem = f.file_stem().unwrap().to_string_lossy().to_string();
            let (tag, pg) = stem.rsplit_once('_').expect("TAG_PP.png");
            let bytes = std::fs::read(f).expect("read page");
            let t0 = Instant::now();
            let d = decode(&bytes).expect("decode page");
            let decode_ms = ms(t0);
            // Timing: the minimum over `repeat` runs (robust to other load on the machine).
            let (mut register_ms, mut measure_ms) = (f64::MAX, f64::MAX);
            let mut measurement = None;
            for _ in 0..repeat {
                let t1 = Instant::now();
                let reg = engine.register_page(&d.image);
                register_ms = register_ms.min(ms(t1));
                let t2 = Instant::now();
                let m = engine.measure_registered(&d.image, reg);
                measure_ms = measure_ms.min(ms(t2));
                measurement = Some(m);
            }
            let measurement = measurement.expect("repeat >= 1");
            PageRun {
                tag: tag.to_owned(),
                page: pg.parse().expect("page number"),
                measurement,
                capture: d.capture,
                decode_ms,
                register_ms,
                measure_ms,
            }
        })
        .collect()
}

/// Decides every page under one baseline per page (`baseline_of(index)`).
pub fn decide_all(
    engine: &LegacyEngine,
    runs: &[PageRun],
    baseline_of: &dyn Fn(usize) -> Baseline,
) -> Vec<LegacyPageResult> {
    runs.iter()
        .enumerate()
        .map(|(i, r)| {
            let req = PageRequest {
                n_questions: questions_for(&r.tag),
                high_stakes: false,
                capture: r.capture,
                page: Some(format!("{}_{:02}", r.tag, r.page)),
            };
            engine.decide(&r.measurement, &baseline_of(i), &req)
        })
        .collect()
}

#[derive(Default)]
struct FieldTally {
    exact: u32,
    flagged: u32,
    wrong_unflagged: u32,
}

impl FieldTally {
    fn add(&mut self, ours: Option<String>, reference: &str) {
        match ours {
            None => self.flagged += 1,
            Some(o) if o.eq_ignore_ascii_case(reference) => self.exact += 1,
            Some(_) => self.wrong_unflagged += 1,
        }
    }

    fn json(&self) -> Value {
        json!({ "exact": self.exact, "flagged": self.flagged, "wrong_unflagged": self.wrong_unflagged })
    }
}

/// Scores decisions against `reference.json`; returns aggregate metrics.
pub fn score(runs: &[PageRun], results: &[LegacyPageResult], reference: &Value) -> Value {
    let refs: BTreeMap<(String, u64), &Value> = reference
        .as_array()
        .expect("reference list")
        .iter()
        .map(|p| {
            (
                (
                    p["tag"].as_str().unwrap().to_owned(),
                    p["page"].as_u64().unwrap(),
                ),
                p,
            )
        })
        .collect();
    let (mut total, mut auto, mut silent, mut review) = (0u32, 0u32, 0u32, 0u32);
    let mut review_reasons: BTreeMap<String, u32> = BTreeMap::new();
    let mut review_truth: BTreeMap<String, u32> = BTreeMap::new();
    let mut codes: BTreeMap<String, u32> = BTreeMap::new();
    let (mut sn, mut sec, mut ver, mut exam) = (
        FieldTally::default(),
        FieldTally::default(),
        FieldTally::default(),
        FieldTally::default(),
    );
    let mut levels = Vec::new();
    // Identity contract: an ID grid that is not confident never yields exact digits.
    let mut exact_while_flagged = 0u32;
    let mut unconfident_proposals = 0u32;
    let mut missing = 0;
    for (run, res) in runs.iter().zip(results) {
        let Some(r) = refs.get(&(run.tag.clone(), u64::from(run.page))) else {
            missing += 1;
            continue;
        };
        levels.push(f64::from(res.level.0) / 1024.0);
        for e in &res.exceptions {
            *codes
                .entry(format!("{}/{:?}", e.code, e.severity))
                .or_default() += 1;
        }
        let ref_answers = r["answers"].as_array().unwrap();
        for qo in &res.questions {
            total += 1;
            let truth = ref_answers[usize::from(qo.question)]
                .as_str()
                .unwrap()
                .to_uppercase();
            let ours = answer_text(&qo.response);
            for e in &qo.exceptions {
                *codes
                    .entry(format!("{}/{:?}", e.code, e.severity))
                    .or_default() += 1;
            }
            if qo.auto_accepted {
                auto += 1;
                if ours != truth {
                    silent += 1;
                }
            } else {
                review += 1;
                let why = qo
                    .exceptions
                    .iter()
                    .map(|e| format!("{}:{}", e.code, e.sub_reason.clone().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join(",");
                *review_reasons.entry(why).or_default() += 1;
                let kind = if truth == "BLANK" {
                    "truth_blank"
                } else if ours == truth {
                    "proposal_correct"
                } else {
                    "proposal_wrong"
                };
                *review_truth.entry(kind.into()).or_default() += 1;
            }
        }
        for g in [&res.student_number, &res.section] {
            if !g.confident() && g.read.exact_digits().is_some() {
                exact_while_flagged += 1;
            }
            unconfident_proposals += g
                .read
                .columns
                .iter()
                .zip(&g.review)
                .filter(|(c, r)| {
                    **r && c.state == omr_model::ColumnState::Multi && c.marked.len() == 1
                })
                .count() as u32;
        }
        let grid_text = |g: &omr_recog::GridField| {
            let t = g.text();
            (!t.contains('?')).then_some(t)
        };
        sn.add(
            grid_text(&res.student_number),
            r["student_number"].as_str().unwrap(),
        );
        sec.add(grid_text(&res.section), r["section"].as_str().unwrap());
        let choice_text = |c: &omr_recog::ChoiceRead| {
            c.text().map(|s| {
                if s.is_empty() {
                    "BLANK".to_owned()
                } else {
                    s.to_owned()
                }
            })
        };
        ver.add(choice_text(&res.version), r["version"].as_str().unwrap());
        exam.add(choice_text(&res.exam), r["exam"].as_str().unwrap());
    }
    let ser = if auto > 0 {
        f64::from(silent) / f64::from(auto)
    } else {
        0.0
    };
    let ar = if total > 0 {
        f64::from(review) / f64::from(total)
    } else {
        0.0
    };
    let bound95 = if silent == 0 && auto > 0 {
        3.0 / f64::from(auto)
    } else {
        f64::NAN
    };
    json!({
        "pages": results.len(),
        "pages_without_reference": missing,
        "questions": total,
        "auto_accepted": auto,
        "silent_errors": silent,
        "ser": ser,
        "ser_upper_95_rule_of_three": bound95,
        "review": review,
        "ar": ar,
        "review_reasons": review_reasons,
        "review_vs_truth": review_truth,
        "student_number": sn.json(),
        "section": sec.json(),
        "version": ver.json(),
        "exam": exam.json(),
        "id_exact_digits_while_flagged": exact_while_flagged,
        "id_unconfident_single_proposals": unconfident_proposals,
        "exception_counts": codes,
        "page_level": quantiles(&mut levels),
    })
}

/// Registration and timing aggregates.
pub fn registration_metrics(runs: &[PageRun], n_template: usize) -> Value {
    let mut matched: Vec<f64> = runs
        .iter()
        .map(|r| f64::from(r.measurement.registration.matched))
        .collect();
    let mut rms: Vec<f64> = runs
        .iter()
        .map(|r| r.measurement.registration.rms_px)
        .collect();
    let mut scale: Vec<f64> = runs
        .iter()
        .map(|r| r.measurement.registration.scale)
        .collect();
    let mut rot: Vec<f64> = runs
        .iter()
        .map(|r| r.measurement.registration.rotation_rad.to_degrees().abs())
        .collect();
    let mut det: Vec<f64> = runs
        .iter()
        .map(|r| f64::from(r.measurement.registration.detections))
        .collect();
    let mut ncc: Vec<f64> = runs
        .iter()
        .map(|r| r.measurement.registration.coarse_ncc)
        .collect();
    let mut alias: Vec<f64> = runs
        .iter()
        .map(|r| r.measurement.registration.alias_margin)
        .collect();
    let mut radius: BTreeMap<String, u32> = BTreeMap::new();
    for r in runs {
        *radius
            .entry(format!("{:.1}", r.measurement.registration.ring_radius_px))
            .or_default() += 1;
    }
    let failed = runs.iter().filter(|r| !r.measurement.registered()).count();
    let mut unreadable_by: BTreeMap<String, u32> = BTreeMap::new();
    for r in runs {
        for (j, b) in r.measurement.bubbles.iter().enumerate() {
            if let Some(c) = b.unreadable {
                let f = engine_fields(j, &r.measurement);
                *unreadable_by.entry(format!("{f}/{c:?}")).or_default() += 1;
            }
        }
    }
    let unreadable: usize = runs
        .iter()
        .map(|r| {
            r.measurement
                .bubbles
                .iter()
                .filter(|b| !b.readable())
                .count()
        })
        .sum();
    let mut dec: Vec<f64> = runs.iter().map(|r| r.decode_ms).collect();
    let mut reg: Vec<f64> = runs.iter().map(|r| r.register_ms).collect();
    let mut mea: Vec<f64> = runs.iter().map(|r| r.measure_ms).collect();
    let mut rm: Vec<f64> = runs.iter().map(|r| r.register_ms + r.measure_ms).collect();
    let mut all: Vec<f64> = runs
        .iter()
        .map(|r| r.decode_ms + r.register_ms + r.measure_ms)
        .collect();
    let captures: BTreeMap<String, u32> = runs.iter().fold(BTreeMap::new(), |mut m, r| {
        *m.entry(format!("{:?}", r.capture)).or_default() += 1;
        m
    });
    json!({
        "template_bubbles": n_template,
        "failed_pages": failed,
        "unreadable_bubbles": unreadable,
        "unreadable_by_field_cause": unreadable_by,
        "matched": quantiles(&mut matched),
        "rms_px": quantiles(&mut rms),
        "scale": quantiles(&mut scale),
        "abs_rotation_deg": quantiles(&mut rot),
        "detections": quantiles(&mut det),
        "coarse_ncc": quantiles(&mut ncc),
        "alias_margin": quantiles(&mut alias),
        "ring_radius_px": radius,
        "capture": captures,
        "ms_per_page_single_thread": {
            "decode_png": quantiles(&mut dec),
            "register": quantiles(&mut reg),
            "measure": quantiles(&mut mea),
            "register_plus_measure": quantiles(&mut rm),
            "total_with_decode": quantiles(&mut all),
        },
    })
}

fn engine_fields(j: usize, _m: &PageMeasurement) -> &'static str {
    let t = omr_forms::legacy::LegacyTemplate::iau_ccsit_v1();
    match t.bubbles()[j].field {
        LegacyField::Answers => "answers",
        LegacyField::StudentNumber => "student_number",
        LegacyField::Section => "section",
        LegacyField::Date => "date",
        LegacyField::Gender => "gender",
        LegacyField::Version => "version",
        LegacyField::Exam => "exam",
    }
}

fn proto_key(engine: &LegacyEngine, j: usize) -> (String, String, String) {
    let t = engine.template();
    let b = t.bubbles()[j];
    let s = |v: &dyn std::fmt::Display| v.to_string();
    match b.field {
        LegacyField::Answers => (
            "answers".into(),
            s(&(b.group + 1)),
            t.option_labels()[usize::from(b.option)].clone(),
        ),
        LegacyField::StudentNumber => ("student_number".into(), s(&b.group), s(&b.option)),
        LegacyField::Section => ("section".into(), s(&b.group), s(&b.option)),
        LegacyField::Date => (
            "date".into(),
            t.date_columns()[usize::from(b.group)].clone(),
            s(&b.option),
        ),
        LegacyField::Gender | LegacyField::Version | LegacyField::Exam => {
            let name = match b.field {
                LegacyField::Gender => "gender",
                LegacyField::Version => "version",
                _ => "exam",
            };
            (
                name.into(),
                "0".into(),
                t.choice_labels(b.field)[usize::from(b.option)].clone(),
            )
        }
    }
}

fn jstr(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Centre distances to the prototype's centres (`measurements.json`).
pub fn compare_centres(engine: &LegacyEngine, runs: &[PageRun], meas: &Value) -> Value {
    let order = meas["order"].as_array().unwrap();
    let idx: BTreeMap<(String, String, String), usize> = order
        .iter()
        .enumerate()
        .map(|(i, o)| ((jstr(&o[0]), jstr(&o[1]), jstr(&o[2])), i))
        .collect();
    let map: Vec<Option<usize>> = (0..engine.template().bubbles().len())
        .map(|j| idx.get(&proto_key(engine, j)).copied())
        .collect();
    let pages: BTreeMap<(String, u64), &Value> = meas["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| ((jstr(&p["tag"]), p["page"].as_u64().unwrap()), p))
        .collect();
    let (mut all, mut snapped, mut inner_diff) = (Vec::new(), Vec::new(), Vec::new());
    let mut snap_agree = 0u32;
    let mut n = 0u32;
    for r in runs {
        let Some(p) = pages.get(&(r.tag.clone(), u64::from(r.page))) else {
            continue;
        };
        let centres = p["centres"].as_array().unwrap();
        let snaps = p["snap"].as_array().unwrap();
        let inner = p["inner"].as_array().unwrap();
        for (j, b) in r.measurement.bubbles.iter().enumerate() {
            let Some(k) = map[j] else { continue };
            if !b.readable() {
                continue;
            }
            let c = &centres[k];
            let (x, y) = (c[0].as_f64().unwrap(), c[1].as_f64().unwrap());
            let d = ((b.centre.x - x).powi(2) + (b.centre.y - y).powi(2)).sqrt();
            all.push(d);
            let ps = snaps[k].as_i64().unwrap() == 1;
            if ps && b.snapped {
                snapped.push(d);
            }
            n += 1;
            if ps == b.snapped {
                snap_agree += 1;
            }
            if let Some(v) = inner[k].as_f64() {
                inner_diff.push((b.inner.as_f64() - v).abs());
            }
        }
    }
    json!({
        "bubbles_compared": n,
        "unmapped_template_bubbles": map.iter().filter(|m| m.is_none()).count(),
        "centre_distance_px_all": quantiles(&mut all),
        "centre_distance_px_both_snapped": quantiles(&mut snapped),
        "snap_flag_agreement": f64::from(snap_agree) / f64::from(n.max(1)),
        "inner_abs_diff": quantiles(&mut inner_diff),
    })
}

/// Decision agreement with the prototype's reads (`proto_reads.json`).
pub fn compare_decisions(runs: &[PageRun], results: &[LegacyPageResult], proto: &Value) -> Value {
    let pages: BTreeMap<(String, u64), &Value> = proto
        .as_array()
        .unwrap()
        .iter()
        .map(|p| ((jstr(&p["tag"]), p["page"].as_u64().unwrap()), p))
        .collect();
    let mut same = 0u32;
    let mut n = 0u32;
    let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
    let (mut sn_same, mut sec_same, mut ver_same, mut np) = (0u32, 0u32, 0u32, 0u32);
    for (r, res) in runs.iter().zip(results) {
        let Some(p) = pages.get(&(r.tag.clone(), u64::from(r.page))) else {
            continue;
        };
        np += 1;
        let pa = p["answers"].as_array().unwrap();
        for qo in &res.questions {
            let Some(a) = pa.get(usize::from(qo.question)) else {
                continue;
            };
            n += 1;
            let pr = a["read"].as_str().unwrap().to_uppercase();
            let prev = a["review"].as_bool().unwrap();
            let ours = answer_text(&qo.response);
            let orev = !qo.auto_accepted;
            if pr == ours && prev == orev {
                same += 1;
            } else {
                let k = format!(
                    "proto_{}_ours_{}{}",
                    if prev { "review" } else { "auto" },
                    if orev { "review" } else { "auto" },
                    if pr == ours {
                        "_same_read"
                    } else {
                        "_different_read"
                    }
                );
                *kinds.entry(k).or_default() += 1;
            }
        }
        if p["student_number"].as_str() == Some(res.student_number.text().as_str()) {
            sn_same += 1;
        }
        if p["section"].as_str() == Some(res.section.text().as_str()) {
            sec_same += 1;
        }
        let v = res.version.text().map(|s| {
            if s.is_empty() {
                "BLANK".to_owned()
            } else {
                s.to_owned()
            }
        });
        if p["version"].as_str().map(str::to_uppercase) == v.map(|s| s.to_uppercase()) {
            ver_same += 1;
        }
    }
    json!({
        "questions_compared": n,
        "identical_read_and_routing": same,
        "differences": kinds,
        "pages_compared": np,
        "student_number_text_identical": sn_same,
        "section_text_identical": sec_same,
        "version_identical": ver_same,
    })
}

/// Runs the whole evaluation; `None` when `OMR_IAU_DIR` is unset.
pub fn evaluate() -> Option<Value> {
    let dir = PathBuf::from(std::env::var_os("OMR_IAU_DIR")?);
    let t0 = Instant::now();
    let mut cfg = omr_recog::LegacyConfig::bitonal_v1();
    if let Some(v) = std::env::var("OMR_EVAL_CAND_NCC")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        cfg.registration.candidate_ncc_milli = v;
    }
    if let Some(v) = std::env::var("OMR_EVAL_HYSTERESIS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        cfg.decision.hysteresis = v;
    }
    if std::env::var("OMR_EVAL_FALLBACK").is_ok_and(|v| v == "field_median") {
        cfg.baseline.fallback = omr_recog::BaselineFallback::FieldMedian;
    }
    let engine = LegacyEngine::new(
        omr_forms::legacy::LegacyTemplate::iau_ccsit_v1(),
        omr_forms::legacy::IAU_CCSIT_V1_BLANK_PNG,
        cfg,
    )
    .expect("engine");
    let engine_ms = ms(t0);
    let repeat: usize = std::env::var("OMR_EVAL_REPEAT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let runs = run_pages(&engine, &dir, repeat);
    let reference = read_json(&dir.join("reference.json")).expect("reference.json");
    let all: Vec<&PageMeasurement> = runs.iter().map(|r| &r.measurement).collect();
    let pooled = engine.batch_baseline(&all);
    let t1 = Instant::now();
    let res_pooled = decide_all(&engine, &runs, &|_| pooled.clone());
    let decide_ms = ms(t1) / runs.len().max(1) as f64;
    // Per-file batch slices.
    let mut slices: BTreeMap<String, Baseline> = BTreeMap::new();
    for tag in runs.iter().map(|r| r.tag.clone()) {
        if !slices.contains_key(&tag) {
            let ms: Vec<&PageMeasurement> = runs
                .iter()
                .filter(|r| r.tag == tag)
                .map(|r| &r.measurement)
                .collect();
            slices.insert(tag.clone(), engine.batch_baseline(&ms));
        }
    }
    let res_slice = decide_all(&engine, &runs, &|i| slices[&runs[i].tag].clone());
    let prior = engine.prior_baseline();
    let res_single = decide_all(&engine, &runs, &|_| prior.clone());
    let mut out = json!({
        "engine": {
            "config_version": engine.config().version,
            "config_sha384": engine.config_sha384(),
            "template_sha384": engine.template_sha384(),
            "engine_build_ms": engine_ms,
            "decide_ms_per_page": decide_ms,
            "hysteresis_1024_gray": engine.config().decision.hysteresis,
            "hysteresis_1024_bitonal": engine.config().decision.hysteresis * engine.config().decision.bitonal_hysteresis_factor,
            "timing_repeat_min_of": repeat,
        },
        "registration": registration_metrics(&runs, engine.template().bubbles().len()),
        "decisions_pooled_batch": score(&runs, &res_pooled, &reference),
        "decisions_per_file_batch": score(&runs, &res_slice, &reference),
        "decisions_single_page_prior": score(&runs, &res_single, &reference),
        "baseline_fallback_positions": pooled.source.iter().filter(|s| matches!(s, omr_recog::BaselineSource::Fallback)).count(),
    });
    if let Some(meas) = read_json(&dir.join("measurements.json")) {
        out["vs_prototype_centres"] = compare_centres(&engine, &runs, &meas);
    }
    if let Some(proto) = read_json(&dir.join("proto_reads.json")) {
        out["vs_prototype_decisions_pooled"] = compare_decisions(&runs, &res_pooled, &proto);
    }
    if let Some(path) = std::env::var_os("OMR_EVAL_OUT") {
        std::fs::write(&path, serde_json::to_vec_pretty(&out).unwrap()).expect("write metrics");
    }
    Some(out)
}
