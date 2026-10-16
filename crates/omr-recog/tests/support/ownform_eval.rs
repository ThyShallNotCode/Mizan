//! Own-form evaluation on a semi-synthetic corpus generated in memory with `omr-sim` (truth by
//! construction, §23.3): render → marks → capture → JPEG at the sampled quality → decode →
//! page stage → pooled batch baseline (and the single-page prior) → decisions, compared with
//! the truth. Shared by `tests/ownform_synth.rs` (small corpora) and
//! `examples/eval_ownform.rs` (≥ 300 sheets per profile). Synthetic only: no real data.
#![allow(dead_code)]

use omr_forms::render::rasterize_static;
use omr_forms::{ExamKey, SheetInstance, Template, TemplateSpec};
use omr_imaging::GrayImage;
use omr_model::{CancelMode, ExamId, FormCode, PageNo, Response};
use omr_recog::ownform::{
    CodeOutcome, CodeRegistry, OwnFormConfig, OwnFormEngine, OwnPageMeasurement, OwnPageRequest,
    OwnPageResult, PageRoute,
};
use omr_sim::{CorpusConfig, Difficulty, MarkRates, PageTruth, Profile, Simulator};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

/// What to generate and evaluate.
#[derive(Clone, Debug)]
pub struct EvalSpec {
    /// Booklets.
    pub sheets: u32,
    /// Capture profile.
    pub profile: Profile,
    /// Corpus seed.
    pub seed: u64,
    /// Questions per exam (100 fit one side; more give multi-page booklets).
    pub questions: u16,
    /// Options per question.
    pub options: u8,
    /// Forms.
    pub forms: u16,
    /// Capture resolution.
    pub dpi: u32,
    /// Worker threads for generation + page stage.
    pub threads: usize,
    /// Mark rates (`None` = the profile's default: standard → standard, hard → hard).
    pub rates: Option<MarkRates>,
    /// Engine configuration.
    pub config: OwnFormConfig,
    /// Measure `timing_pages` pages again on one thread for ms/page.
    pub timing_pages: usize,
}

impl EvalSpec {
    /// Defaults for a profile.
    pub fn new(profile: Profile, sheets: u32, seed: u64) -> Self {
        Self {
            sheets,
            profile,
            seed,
            questions: 100,
            options: 5,
            forms: 2,
            dpi: 200,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            rates: None,
            config: OwnFormConfig::gray_v1(),
            timing_pages: 0,
        }
    }
}

/// The evaluation exam (fixed ID so corpora are reproducible).
pub fn template(questions: u16, options: u8, forms: u16) -> Template {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_aaaa_bbbb_7ccc_8ddd_0000_0000_0042,
        )),
        questions,
        options,
        CancelMode::EraseOnly,
    );
    s.forms = forms;
    Template::build(s).expect("template")
}

/// The evaluation K_exam (synthetic).
pub fn key() -> ExamKey {
    ExamKey::from_bytes([0x5a; 32])
}

/// Converts an `image` buffer.
pub fn to_gray(img: &image::GrayImage) -> GrayImage {
    GrayImage::from_raw(
        img.width() as usize,
        img.height() as usize,
        img.as_raw().clone(),
    )
    .expect("dimensions")
}

/// The engine with blank-render priors for every page.
pub fn engine(t: &Template, k: &ExamKey, dpi: u32, config: OwnFormConfig) -> OwnFormEngine {
    let mut e = OwnFormEngine::new(t.clone(), config);
    for p in 1..=t.page_count() {
        let inst = SheetInstance::new(t, k, FormCode(1), PageNo(p));
        let blank = rasterize_static(&inst, dpi as f32).expect("render");
        e = e
            .with_blank_page(PageNo(p), &to_gray(&blank), f64::from(dpi))
            .expect("prior");
    }
    e
}

/// One captured page: JPEG bytes as a scanner would deliver them, plus the truth.
pub struct CapturedPage {
    pub jpeg: Vec<u8>,
    pub truth: PageTruth,
}

/// Generates sheet `i` of the corpus (deterministic by seed and index).
pub fn capture_sheet(
    sim: &Simulator<'_>,
    t: &Template,
    ccfg: &CorpusConfig,
    i: u32,
) -> Vec<CapturedPage> {
    let (script, mut rng) = omr_sim::corpus::sheet_script(t, ccfg, &[], i).expect("script");
    sim.simulate(&script, ccfg.profile, &mut rng)
        .expect("simulate")
        .into_iter()
        .map(|mut p| {
            p.truth.sheet = u64::from(i);
            let jpeg =
                omr_sim::capture::encode_jpeg(&p.image, p.truth.capture.jpeg_quality, ccfg.dpi)
                    .expect("jpeg");
            CapturedPage {
                jpeg,
                truth: p.truth,
            }
        })
        .collect()
}

/// A measured page with its truth.
pub struct Measured {
    pub truth: PageTruth,
    pub m: OwnPageMeasurement,
    pub measure_ms: f64,
    pub decode_ms: f64,
}

/// Decodes and measures one captured page.
pub fn measure(
    e: &OwnFormEngine,
    reg: &CodeRegistry,
    p: CapturedPage,
    dpi: u32,
) -> (Measured, GrayImage) {
    let t0 = Instant::now();
    let d = omr_imaging::decode(&p.jpeg).expect("decode");
    let decode_ms = t0.elapsed().as_secs_f64() * 1e3;
    let req = OwnPageRequest {
        device_dpi: Some(dpi),
        capture: d.capture,
        page: Some(format!("s{}p{}", p.truth.sheet, p.truth.page)),
        ..OwnPageRequest::default()
    };
    let t1 = Instant::now();
    let m = e.measure_page(&d.image, &req, reg);
    let measure_ms = t1.elapsed().as_secs_f64() * 1e3;
    (
        Measured {
            truth: p.truth,
            m,
            measure_ms,
            decode_ms,
        },
        d.image,
    )
}

/// Generates and measures the whole corpus on `threads` workers; results in (sheet, page)
/// order.
pub fn run_corpus(
    spec: &EvalSpec,
    t: &Template,
    e: &OwnFormEngine,
    reg: &CodeRegistry,
) -> Vec<Measured> {
    let k = key();
    let sim = Simulator::new(t, &k, spec.dpi);
    let mut ccfg = CorpusConfig::standard(spec.sheets, spec.seed);
    ccfg.profile = spec.profile;
    ccfg.dpi = spec.dpi;
    ccfg.rates = spec.rates.unwrap_or(match spec.profile {
        Profile::Hard => MarkRates::hard(),
        Profile::Clean => MarkRates::clean(),
        Profile::Standard => MarkRates::standard(),
    });
    let next = AtomicU32::new(0);
    let out: Mutex<Vec<Measured>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..spec.threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= spec.sheets {
                        break;
                    }
                    for p in capture_sheet(&sim, t, &ccfg, i) {
                        let (m, _) = measure(e, reg, p, spec.dpi);
                        out.lock().expect("lock").push(m);
                    }
                }
            });
        }
    });
    let mut v = out.into_inner().expect("lock");
    v.sort_by_key(|m| (m.truth.sheet, m.truth.page));
    v
}

#[derive(Default, Clone)]
struct Tally {
    questions: u64,
    auto: u64,
    silent: u64,
    review: u64,
    unreadable: u64,
    auto_on_review_truth: u64,
    by_difficulty: BTreeMap<String, [u64; 3]>,
    review_reasons: BTreeMap<String, u64>,
    silent_examples: Vec<String>,
}

fn diff_name(d: Difficulty) -> String {
    format!("{d:?}").to_lowercase()
}

fn tally_page(t: &mut Tally, truth: &PageTruth, r: &OwnPageResult) {
    let by_q: BTreeMap<u16, &omr_model::QuestionOutcome> =
        r.questions.iter().map(|q| (q.question, q)).collect();
    for qt in &truth.questions {
        t.questions += 1;
        let e = t.by_difficulty.entry(diff_name(qt.difficulty)).or_default();
        e[0] += 1;
        let Some(o) = by_q.get(&qt.question) else {
            t.review += 1;
            e[2] += 1;
            continue;
        };
        if o.auto_accepted {
            t.auto += 1;
            if matches!(qt.expected, omr_sim::Expected::Review) {
                t.auto_on_review_truth += 1;
            }
            let want = match &qt.intended {
                Response::Multi(v) if v.len() == 1 => Response::Single(v[0]),
                r => r.clone(),
            };
            if o.response != want {
                t.silent += 1;
                e[1] += 1;
                if t.silent_examples.len() < 20 {
                    t.silent_examples.push(format!(
                        "sheet {} q {}: truth {:?} ({:?}), read {:?}",
                        truth.sheet, qt.question, qt.intended, qt.difficulty, o.response
                    ));
                }
            }
        } else {
            t.review += 1;
            e[2] += 1;
            if o.response.is_unreadable() {
                t.unreadable += 1;
            }
            let why = o
                .exceptions
                .iter()
                .find(|x| x.severity >= omr_model::Severity::Error)
                .map(|x| {
                    format!(
                        "{}{} [{}]",
                        x.code,
                        x.sub_reason
                            .as_deref()
                            .map(|s| format!(" {s}"))
                            .unwrap_or_default(),
                        diff_name(qt.difficulty)
                    )
                })
                .unwrap_or_else(|| {
                    format!("page not auto-gradable [{}]", diff_name(qt.difficulty))
                });
            *t.review_reasons.entry(why).or_default() += 1;
        }
    }
}

fn tally_json(t: &Tally) -> Value {
    let n = t.questions.max(1) as f64;
    let by: BTreeMap<String, Value> = t
        .by_difficulty
        .iter()
        .map(|(k, [q, s, r])| {
            (
                k.clone(),
                json!({"questions": q, "silent_errors": s, "review": r,
                       "ar": *r as f64 / (*q).max(1) as f64}),
            )
        })
        .collect();
    json!({
        "questions": t.questions,
        "auto_accepted": t.auto,
        "silent_errors": t.silent,
        "ser": t.silent as f64 / t.auto.max(1) as f64,
        "ser_upper95_at_k0": if t.silent == 0 { 3.0 / t.auto.max(1) as f64 } else { f64::NAN },
        "review": t.review,
        "ar": t.review as f64 / n,
        "unreadable": t.unreadable,
        "auto_accepted_on_review_truth": t.auto_on_review_truth,
        "by_difficulty": by,
        "review_reasons": t.review_reasons,
        "silent_examples": t.silent_examples,
    })
}

fn id_tally(pages: &[(&Measured, &OwnPageResult)]) -> Value {
    let (mut exact, mut flagged, mut wrong) = (0u64, 0u64, 0u64);
    for (m, r) in pages {
        let (Some(truth), Some(g)) = (&m.truth.student_id, &r.student_id) else {
            continue;
        };
        if m.truth.page != 1 {
            continue;
        }
        let text = g.text();
        if g.confident() && text == *truth {
            exact += 1;
        } else if !g.confident() {
            flagged += 1;
        } else {
            wrong += 1;
        }
    }
    json!({"exact_confident": exact, "flagged": flagged, "wrong_unflagged": wrong})
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1]
}

/// Runs the evaluation and returns aggregate metrics (no per-sheet data beyond the first few
/// silent-error descriptions, which are synthetic).
pub fn evaluate(spec: &EvalSpec) -> Value {
    let t = template(spec.questions, spec.options, spec.forms);
    let k = key();
    let reg = CodeRegistry::for_template(&t, k.clone());
    let e = engine(&t, &k, spec.dpi, spec.config.clone());
    let t0 = Instant::now();
    let measured = run_corpus(spec, &t, &e, &reg);
    let wall = t0.elapsed().as_secs_f64();

    // Registration and identity.
    let mut reg_fail = 0u64;
    let mut code_fail = 0u64;
    let mut wrong_page = 0u64;
    let mut not_allowed = 0u64;
    let mut outcomes: BTreeMap<String, u64> = BTreeMap::new();
    let mut codes: BTreeMap<String, u64> = BTreeMap::new();
    let mut rms_um = Vec::new();
    let mut tick_med = Vec::new();
    let mut ticks_applied = 0u64;
    let mut true_err = Vec::new();
    for m in &measured {
        let Some(a) = &m.m.registration else {
            reg_fail += 1;
            continue;
        };
        rms_um.push(a.rms_um);
        if let Some(tf) = &m.m.ticks {
            tick_med.push(tf.median_um);
            ticks_applied += u64::from(tf.applied);
        }
        let id = m.m.identity.as_ref();
        *outcomes
            .entry(format!("{:?}", id.map(|i| i.outcome)))
            .or_default() += 1;
        if !id.is_some_and(|i| i.verified()) {
            code_fail += 1;
        }
        if id
            .and_then(|i| i.copies.iter().find_map(|c| c.payload.clone()))
            .is_some_and(|p| p != m.truth.payload)
        {
            wrong_page += 1;
        }
        if m.m.page != Some(PageNo(m.truth.page)) {
            wrong_page += 1;
        }
        if !m.m.auto_accept_allowed {
            not_allowed += 1;
        }
        for x in &m.m.exceptions {
            *codes
                .entry(format!(
                    "{} {:?}{}",
                    x.code,
                    x.severity,
                    x.sub_reason
                        .as_deref()
                        .map(|s| format!(" {s}"))
                        .unwrap_or_default()
                ))
                .or_default() += 1;
        }
        // True registration error at the bubble centres (truth homography).
        let h = m.truth.mm_to_px;
        if let Some(g) = t.page(PageNo(m.truth.page)) {
            for (b, bm) in g.bubbles.iter().zip(&m.m.bubbles) {
                let c = b.centre_mm();
                let (u, v) = omr_sim::profile::apply_h(&h, c.x, c.y);
                true_err.push((bm.centre.x - u).hypot(bm.centre.y - v));
            }
        }
    }

    // Decisions: pooled batch baseline and the single-page prior.
    let refs: Vec<&OwnPageMeasurement> = measured.iter().map(|m| &m.m).collect();
    let pooled = e.batch_baseline(&refs);
    let prior = e.batch_baseline(&[]);
    let mut modes = serde_json::Map::new();
    for (name, b) in [("pooled_batch", &pooled), ("single_page_prior", &prior)] {
        let mut tally = Tally::default();
        let results: Vec<OwnPageResult> = measured.iter().map(|m| e.decide(&m.m, b)).collect();
        for (m, r) in measured.iter().zip(&results) {
            tally_page(&mut tally, &m.truth, r);
        }
        let pairs: Vec<(&Measured, &OwnPageResult)> = measured.iter().zip(&results).collect();
        let mut v = tally_json(&tally);
        v["student_id"] = id_tally(&pairs);
        modes.insert(name.to_owned(), v);
    }

    // Feature distributions by true bubble class (pooled baseline δ, dark fraction, annulus
    // ink), for threshold calibration.
    let mut feat: BTreeMap<String, Vec<[f64; 4]>> = BTreeMap::new();
    if let Some(b) = pooled.pages.get(&1) {
        for m in &measured {
            if m.m.page != Some(PageNo(1)) || m.m.features.is_empty() {
                continue;
            }
            let g = t.page(PageNo(1)).expect("page");
            let mut offset: BTreeMap<u16, usize> = BTreeMap::new();
            let mut acc = 0usize;
            for q in g.questions() {
                offset.insert(q, acc);
                acc += t.question_bubbles(q).len();
            }
            for qt in &m.truth.questions {
                let Some(&o) = offset.get(&qt.question) else {
                    continue;
                };
                for (i, c) in qt.classes.iter().enumerate() {
                    let j = o + i;
                    let bm = &m.m.bubbles[j];
                    if !bm.readable() {
                        continue;
                    }
                    let d = f64::from(bm.inner.0) - f64::from(b.beta[j].0);
                    let f = m.m.features[j];
                    feat.entry(format!("{c:?}")).or_default().push([
                        d / 1.024,
                        f64::from(f.dark_permille),
                        f64::from(f.annulus_ink_permille),
                        f64::from(f.dark_permille) - f64::from(bm.inner.0) / 1.024,
                    ]);
                }
            }
        }
    }
    let feature_quantiles: BTreeMap<String, Value> = feat
        .into_iter()
        .map(|(k, v)| {
            let q = |i: usize| {
                let mut col: Vec<f64> = v.iter().map(|r| r[i]).collect();
                [0.001, 0.01, 0.05, 0.5, 0.95, 0.99, 0.999, 1.0]
                    .map(|p| (pct(&mut col, p) * 10.0).round() / 10.0)
            };
            (
                k,
                json!({"n": v.len(), "quantiles": "p0.1 p1 p5 p50 p95 p99 p99.9 max",
                       "delta_permille": q(0), "dark_permille": q(1),
                       "annulus_ink_permille": q(2), "dark_minus_inner_permille": q(3)}),
            )
        })
        .collect();

    // Timing on one thread (rayon pool of 1 so the detector's internal loops run serially).
    let mut timing = json!(null);
    if spec.timing_pages > 0 {
        let sim = Simulator::new(&t, &k, spec.dpi);
        let mut ccfg = CorpusConfig::standard(spec.sheets, spec.seed);
        ccfg.profile = spec.profile;
        ccfg.dpi = spec.dpi;
        let pages: Vec<CapturedPage> = (0..spec.timing_pages as u32)
            .flat_map(|i| capture_sheet(&sim, &t, &ccfg, i % spec.sheets.max(1)))
            .take(spec.timing_pages)
            .collect();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("pool");
        let (mut ms, mut dec, mut decide) = (Vec::new(), Vec::new(), Vec::new());
        pool.install(|| {
            for p in pages {
                let (mm, _) = measure(&e, &reg, p, spec.dpi);
                let t2 = Instant::now();
                let _ = e.decide(&mm.m, &pooled);
                decide.push(t2.elapsed().as_secs_f64() * 1e3);
                ms.push(mm.measure_ms);
                dec.push(mm.decode_ms);
            }
        });
        timing = json!({
            "pages": ms.len(),
            "threads": 1,
            "page_stage_ms_median": pct(&mut ms.clone(), 0.5),
            "page_stage_ms_p95": pct(&mut ms.clone(), 0.95),
            "page_stage_ms_max": pct(&mut ms, 1.0),
            "jpeg_decode_ms_median": pct(&mut dec, 0.5),
            "decide_ms_median": pct(&mut decide, 0.5),
        });
    }
    let n_pages = measured.len() as u64;
    json!({
        "corpus": {
            "generator": "omr-sim (in memory), JPEG at the sampled quality, decoded by omr-imaging",
            "profile": spec.profile.name(),
            "seed": spec.seed,
            "sheets": spec.sheets,
            "pages": n_pages,
            "questions_per_sheet": spec.questions,
            "options": spec.options,
            "forms": spec.forms,
            "dpi": spec.dpi,
        },
        "engine": {
            "config_sha384": e.config_sha384(),
            "template_sha384": e.template_sha384(),
            "detector": e.detector_name(),
        },
        "registration": {
            "failed_pages": reg_fail,
            "code_not_verified": code_fail,
            "wrong_page_or_payload": wrong_page,
            "pages_not_auto_gradable": not_allowed,
            "code_outcomes": outcomes,
            "rms_um_median": pct(&mut rms_um.clone(), 0.5),
            "rms_um_max": pct(&mut rms_um, 1.0),
            "true_centre_error_px_median": pct(&mut true_err.clone(), 0.5),
            "true_centre_error_px_p99": pct(&mut true_err.clone(), 0.99),
            "true_centre_error_px_max": pct(&mut true_err, 1.0),
            "tick_median_um_median": pct(&mut tick_med, 0.5),
            "pages_with_tick_corrections": ticks_applied,
        },
        "page_exceptions": codes,
        "decisions": Value::Object(modes),
        "features_by_class": feature_quantiles,
        "wall_seconds_generate_and_measure": wall,
        "timing": timing,
    })
}

/// Pages measured and decided in single-page mode, for small targeted tests.
pub fn single(
    e: &OwnFormEngine,
    reg: &CodeRegistry,
    img: &GrayImage,
    dpi: Option<u32>,
) -> (OwnPageMeasurement, OwnPageResult) {
    let req = OwnPageRequest {
        device_dpi: dpi,
        ..OwnPageRequest::default()
    };
    let m = e.measure_page(img, &req, reg);
    let r = e.decide(&m, &e.batch_baseline(&[]));
    (m, r)
}

/// True if a result routes to this template with a verified code.
pub fn routed_ok(m: &OwnPageMeasurement) -> bool {
    m.route == PageRoute::ThisTemplate
        && m.identity
            .as_ref()
            .is_some_and(|i| matches!(i.outcome, CodeOutcome::Verified | CodeOutcome::OneCopyValid))
}
