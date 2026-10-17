//! The recognition engines behind one interface (§8.1): the legacy-form path (§6.10) and the
//! own-form path (§6.3–§6.4). Page stage = [`Recognizer::measure`] (pure, one task per page),
//! batch stage = [`Recognizer::baseline`] then [`Recognizer::decide`]. Key-blind (R2).

use crate::AppError;
use crate::exam::{ExamDefinition, TemplateChoice};
use omr_forms::legacy::{LegacyField, LegacyTemplate};
use omr_forms::{ExamKey, SheetInstance, Template};
use omr_imaging::{CaptureKind, GrayImage};
use omr_model::{Darkness, EngineManifest, Exception, FormCode, PageNo, QuestionOutcome, Response};
use omr_recog::ownform::{
    CodeRegistry, OwnBaseline, OwnFormConfig, OwnFormEngine, OwnPageMeasurement, OwnPageRequest,
    PageRoute,
};
use omr_recog::{
    Baseline, BubbleMeasure, ChoiceRead, DecisionConfig, GridField, LegacyConfig, LegacyEngine,
    PageMeasurement, PageRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::ops::Range;

/// Working resolution of every engine (dpi).
pub const WORKING_DPI: u32 = 200;

/// A page brought to the engine's working resolution.
#[derive(Clone, Debug)]
pub struct WorkPage {
    /// Working gray image.
    pub image: GrayImage,
    /// Capture classification of the original.
    pub capture: CaptureKind,
    /// Resolution of the original, when known (PDF placement) or estimated.
    pub source_dpi: Option<u32>,
    /// Resolution of `image`, when known.
    pub dpi: Option<u32>,
}

/// One bubble of the crop geometry, in working-image pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BubbleGeom {
    /// Option index (0-based) or digit.
    pub option: u8,
    /// Centre x (working px).
    pub cx: f32,
    /// Centre y (working px).
    pub cy: f32,
    /// Radius (working px).
    pub r: f32,
    /// δ = inner darkness − β_p (0..1, clamped at 0).
    pub fill: f32,
    /// True when the bubble was unobservable.
    pub unreadable: bool,
}

/// Geometry of one bubble group (question, ID column, choice field).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct GroupGeom {
    /// Bubbles in option order.
    pub bubbles: Vec<BubbleGeom>,
}

/// A decided page in one shape for both engines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageRead {
    /// True when the page was registered and belongs to this exam's template.
    pub registered: bool,
    /// Template page (1-based; legacy sheets have one page).
    pub page_no: u16,
    /// Questions read on this page (0-based exam question numbers, `< n_questions`).
    pub questions: Vec<QuestionOutcome>,
    /// Student-ID grid (page 1).
    pub student_id: Option<GridField>,
    /// Bubbled version / form grid.
    pub form_choice: Option<ChoiceRead>,
    /// Form from a MAC-verified page code (authoritative, §10.3 priority 1).
    pub code_form: Option<FormCode>,
    /// Booklet serial from a MAC-verified page code.
    pub code_serial: Option<u32>,
    /// Page-level exceptions (REG-*, IDN-001/002, COL-004, CAP-*, MRK-015).
    pub exceptions: Vec<Exception>,
    /// Registration summary (integers, milli units).
    pub registration: serde_json::Value,
    /// Page mark level (1/1024).
    pub level: Darkness,
    /// Selection threshold on the δ scale (1/1024): θ_sel = max(abs, rel · level).
    pub theta_sel: u16,
    /// Hysteresis band half-width (1/1024).
    pub hysteresis: u16,
    /// Per-question geometry, indexed like `questions`.
    pub question_geom: Vec<GroupGeom>,
    /// Per-ID-column geometry.
    pub id_geom: Vec<GroupGeom>,
    /// Form/version choice geometry.
    pub form_geom: Option<GroupGeom>,
}

/// Page-stage output of either engine.
#[derive(Clone, Debug)]
pub enum Measured {
    /// Legacy path.
    Legacy(Box<PageMeasurement>, CaptureKind),
    /// Own-form path.
    Own(Box<OwnPageMeasurement>),
}

impl Measured {
    /// True when the page registered (and, for own forms, belongs to this template).
    pub fn registered(&self) -> bool {
        match self {
            Measured::Legacy(m, _) => m.registered(),
            Measured::Own(m) => m.route == PageRoute::ThisTemplate && m.registration.is_some(),
        }
    }

    /// True for a bitonal capture.
    pub fn bitonal(&self) -> bool {
        match self {
            Measured::Legacy(_, c) => c.is_bitonal(),
            Measured::Own(m) => m.request.capture.is_bitonal(),
        }
    }
}

/// Batch-stage baseline of either engine.
#[derive(Clone, Debug)]
pub enum BatchBaseline {
    /// Legacy β_p.
    Legacy(Baseline),
    /// Own-form β_p per page.
    Own(OwnBaseline),
}

impl BatchBaseline {
    /// Batch exceptions (MRK-016 `prior_model`).
    pub fn exceptions(&self) -> &[Exception] {
        match self {
            BatchBaseline::Legacy(b) => &b.exceptions,
            BatchBaseline::Own(b) => &b.exceptions,
        }
    }
}

/// The engine of one exam.
pub enum Recognizer {
    /// Legacy template.
    Legacy {
        /// Engine.
        engine: Box<LegacyEngine>,
        /// Questions used.
        questions: u16,
        /// High-stakes mode.
        high_stakes: bool,
    },
    /// Own form.
    Own {
        /// Engine.
        engine: Box<OwnFormEngine>,
        /// Page-code secrets.
        registry: Box<CodeRegistry>,
        /// Questions used.
        questions: u16,
        /// High-stakes mode.
        high_stakes: bool,
    },
}

impl std::fmt::Debug for Recognizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Recognizer::Legacy { engine, .. } => write!(f, "Legacy({})", engine.template_sha384()),
            Recognizer::Own { engine, .. } => write!(f, "Own({})", engine.template_sha384()),
        }
    }
}

fn legacy_template(name: &str) -> Result<LegacyTemplate, AppError> {
    match name {
        "iau-ccsit-v1" => Ok(LegacyTemplate::iau_ccsit_v1()),
        other => Err(AppError::Definition(format!(
            "unknown legacy template `{other}`"
        ))),
    }
}

/// Builds the own-form template of a definition.
pub fn own_template(def: &ExamDefinition) -> Result<Template, AppError> {
    match &def.template {
        TemplateChoice::Generated(spec) => Template::build((**spec).clone())
            .map_err(|e| AppError::Definition(format!("sheet template: {e}"))),
        TemplateChoice::Legacy(_) => Err(AppError::Definition("not a generated sheet".into())),
    }
}

fn gray_from_image(img: &image::GrayImage) -> GrayImage {
    GrayImage::from_raw(
        img.width() as usize,
        img.height() as usize,
        img.as_raw().clone(),
    )
    .expect("raster dimensions are consistent")
}

impl Recognizer {
    /// Builds the engine of an exam (blank-render priors for own forms at the working dpi).
    pub fn new(def: &ExamDefinition) -> Result<Self, AppError> {
        match &def.template {
            TemplateChoice::Legacy(name) => {
                let template = legacy_template(name)?;
                let mut cfg = LegacyConfig::bitonal_v1();
                cfg.decision.multi_review = def.multi_review.to_recog();
                let engine =
                    LegacyEngine::new(template, omr_forms::legacy::IAU_CCSIT_V1_BLANK_PNG, cfg)
                        .map_err(|e| AppError::Definition(format!("legacy engine: {e}")))?;
                Ok(Recognizer::Legacy {
                    engine: Box::new(engine),
                    questions: def.questions,
                    high_stakes: def.high_stakes,
                })
            }
            TemplateChoice::Generated(_) => {
                let t = own_template(def)?;
                let key = ExamKey::from_bytes(def.page_code_key()?);
                let mut cfg = OwnFormConfig::gray_v1();
                cfg.decision.multi_review = def.multi_review.to_recog();
                cfg.decision_bitonal.multi_review = def.multi_review.to_recog();
                let mut e = OwnFormEngine::new(t.clone(), cfg);
                for p in 1..=t.page_count() {
                    let inst = SheetInstance::new(&t, &key, FormCode(1), PageNo(p));
                    let blank = omr_forms::render::rasterize_static(&inst, WORKING_DPI as f32)
                        .map_err(|e| AppError::Definition(format!("blank render: {e}")))?;
                    e = e
                        .with_blank_page(
                            PageNo(p),
                            &gray_from_image(&blank),
                            f64::from(WORKING_DPI),
                        )
                        .map_err(|e| AppError::Definition(format!("blank prior: {e}")))?;
                }
                let registry = CodeRegistry::for_template(&t, key);
                Ok(Recognizer::Own {
                    engine: Box::new(e),
                    registry: Box::new(registry),
                    questions: def.questions,
                    high_stakes: def.high_stakes,
                })
            }
        }
    }

    /// Engine manifest (template + configuration hashes, §14.6).
    pub fn manifest(&self) -> EngineManifest {
        let mut m = EngineManifest {
            app_version: env!("CARGO_PKG_VERSION").into(),
            cpu_level: "baseline".into(),
            ..EngineManifest::default()
        };
        match self {
            Recognizer::Legacy { engine, .. } => engine.stamp(&mut m),
            Recognizer::Own { engine, .. } => engine.stamp(&mut m),
        }
        m
    }

    /// Template pages.
    pub fn page_count(&self) -> u16 {
        match self {
            Recognizer::Legacy { .. } => 1,
            Recognizer::Own { engine, .. } => engine.template().page_count(),
        }
    }

    /// True for the legacy path.
    pub fn is_legacy(&self) -> bool {
        matches!(self, Recognizer::Legacy { .. })
    }

    /// Brings a decoded page to the working resolution. `known_dpi` is the capture resolution
    /// from the PDF placement, if any (`None`: estimated from the page size, header DPI
    /// distrusted, §8.2).
    pub fn to_working(
        &self,
        image: GrayImage,
        capture: CaptureKind,
        known_dpi: Option<u32>,
    ) -> WorkPage {
        match self {
            Recognizer::Legacy { engine, .. } => {
                let src = known_dpi
                    .unwrap_or_else(|| engine.estimated_dpi(image.width(), image.height()));
                let (num, den) = if known_dpi.is_some() {
                    reduce_ratio(WORKING_DPI, src)
                } else {
                    engine.working_ratio(image.width(), image.height())
                };
                let image = if (num, den) == (1, 1) {
                    image
                } else {
                    omr_imaging::resample_area(&image, num, den)
                };
                WorkPage {
                    image,
                    capture,
                    source_dpi: Some(src),
                    dpi: Some(WORKING_DPI),
                }
            }
            Recognizer::Own { .. } => {
                // A4 portrait (landscape when wider than tall): dpi from the long side.
                let long = image.width().max(image.height()) as f64;
                let est = long / (297.0 / 25.4);
                let src = known_dpi.or_else(|| snap_dpi(est));
                match src {
                    Some(s) if s > WORKING_DPI + WORKING_DPI / 4 => {
                        let (num, den) = reduce_ratio(WORKING_DPI, s);
                        WorkPage {
                            image: omr_imaging::resample_area(&image, num, den),
                            capture,
                            source_dpi: Some(s),
                            dpi: Some(WORKING_DPI),
                        }
                    }
                    s => WorkPage {
                        image,
                        capture,
                        source_dpi: s,
                        dpi: s,
                    },
                }
            }
        }
    }

    /// Page stage (pure; one rayon task per page).
    pub fn measure(&self, page: &WorkPage, label: &str) -> Measured {
        match self {
            Recognizer::Legacy { engine, .. } => {
                Measured::Legacy(Box::new(engine.measure_page(&page.image)), page.capture)
            }
            Recognizer::Own {
                engine,
                registry,
                high_stakes,
                ..
            } => {
                let req = OwnPageRequest {
                    device_dpi: page.dpi,
                    header_dpi: None,
                    capture: page.capture,
                    high_stakes: *high_stakes,
                    page: Some(label.to_string()),
                };
                Measured::Own(Box::new(engine.measure_page(&page.image, &req, registry)))
            }
        }
    }

    /// Batch stage, part 1: the baseline over one batch slice (order-independent).
    pub fn baseline(&self, pages: &[&Measured]) -> BatchBaseline {
        match self {
            Recognizer::Legacy { engine, .. } => {
                let ms: Vec<&PageMeasurement> = pages
                    .iter()
                    .filter_map(|m| match m {
                        Measured::Legacy(m, _) if m.registered() => Some(m.as_ref()),
                        _ => None,
                    })
                    .collect();
                if ms.is_empty() {
                    BatchBaseline::Legacy(engine.prior_baseline())
                } else {
                    BatchBaseline::Legacy(engine.batch_baseline(&ms))
                }
            }
            Recognizer::Own { engine, .. } => {
                let ms: Vec<&OwnPageMeasurement> = pages
                    .iter()
                    .filter_map(|m| match m {
                        Measured::Own(m) => Some(m.as_ref()),
                        _ => None,
                    })
                    .collect();
                BatchBaseline::Own(engine.batch_baseline(&ms))
            }
        }
    }

    /// The single-page prior baseline (provisional first results).
    pub fn prior_baseline(&self) -> BatchBaseline {
        match self {
            Recognizer::Legacy { engine, .. } => BatchBaseline::Legacy(engine.prior_baseline()),
            Recognizer::Own { engine, .. } => BatchBaseline::Own(engine.batch_baseline(&[])),
        }
    }

    fn decision_config(&self, bitonal: bool) -> DecisionConfig {
        match self {
            Recognizer::Legacy { engine, .. } => {
                let mut c = engine.config().decision.clone();
                if bitonal {
                    c.hysteresis = c.hysteresis.saturating_mul(c.bitonal_hysteresis_factor);
                }
                c
            }
            Recognizer::Own { engine, .. } => {
                let cfg = engine.config();
                let mut c = if bitonal {
                    cfg.decision_bitonal.clone()
                } else {
                    cfg.decision.clone()
                };
                if bitonal {
                    c.hysteresis = c.hysteresis.saturating_mul(c.bitonal_hysteresis_factor);
                }
                c
            }
        }
    }

    /// Batch stage, part 2: decisions for one page.
    pub fn decide(&self, m: &Measured, b: &BatchBaseline, label: &str) -> PageRead {
        match (self, m, b) {
            (
                Recognizer::Legacy {
                    engine,
                    questions,
                    high_stakes,
                },
                Measured::Legacy(m, capture),
                BatchBaseline::Legacy(b),
            ) => {
                let req = PageRequest {
                    n_questions: *questions,
                    high_stakes: *high_stakes,
                    capture: *capture,
                    page: Some(label.to_string()),
                };
                let r = engine.decide(m, b, &req);
                let t = engine.template();
                let cfg = self.decision_config(capture.is_bitonal());
                let geom = |range: Range<usize>| GroupGeom {
                    bubbles: range
                        .clone()
                        .map(|j| bubble_geom(&m.bubbles[j], b.beta[j], (j - range.start) as u8))
                        .collect(),
                };
                let question_geom = t
                    .groups(LegacyField::Answers)
                    .iter()
                    .take(r.questions.len())
                    .map(|g| geom(g.clone()))
                    .collect();
                let id_geom = t
                    .groups(LegacyField::StudentNumber)
                    .iter()
                    .map(|g| geom(g.clone()))
                    .collect();
                let form_geom = t
                    .groups(LegacyField::Version)
                    .first()
                    .map(|g| geom(g.clone()));
                let q = &r.registration;
                PageRead {
                    registered: m.registered(),
                    page_no: 1,
                    theta_sel: theta_sel(&cfg, r.level),
                    hysteresis: cfg.hysteresis,
                    questions: r.questions,
                    student_id: Some(r.student_number),
                    form_choice: Some(r.version),
                    code_form: None,
                    code_serial: None,
                    exceptions: r.exceptions,
                    registration: json!({
                        "engine": "legacy",
                        "matched": q.matched,
                        "template_points": q.template_points,
                        "rms_px_milli": milli(q.rms_px),
                        "scale_milli": milli(q.scale),
                        "rotation_urad": (q.rotation_rad * 1e6).round() as i64,
                    }),
                    level: r.level,
                    question_geom,
                    id_geom,
                    form_geom,
                }
            }
            (
                Recognizer::Own {
                    engine, questions, ..
                },
                Measured::Own(m),
                BatchBaseline::Own(b),
            ) => {
                let _ = label;
                let r = engine.decide(m, b);
                let cfg = self.decision_config(m.request.capture.is_bitonal());
                let t = engine.template();
                let registered = m.route == PageRoute::ThisTemplate && m.registration.is_some();
                let page_no = r.page.map_or(0, |p| p.0);
                let beta = r.page.and_then(|p| b.pages.get(&p.0)).map(|bb| &bb.beta);
                let (mut question_geom, mut id_geom, mut form_geom) =
                    (Vec::new(), Vec::new(), None);
                if let (Some(g), Some(beta)) = (r.page.and_then(|p| t.page(p)), beta)
                    && m.bubbles.len() == beta.len()
                {
                    let mut k = 0usize;
                    let mut take = |n: usize| {
                        let range = k..k + n;
                        k += n;
                        GroupGeom {
                            bubbles: range
                                .clone()
                                .map(|j| {
                                    bubble_geom(&m.bubbles[j], beta[j], (j - range.start) as u8)
                                })
                                .collect(),
                        }
                    };
                    let mut qg = Vec::new();
                    for q in g.questions() {
                        let gg = take(t.question_bubbles(q).len());
                        qg.push((q, gg));
                    }
                    let mut cols: Vec<usize> = Vec::new();
                    let mut last = None;
                    for b in &g.id_bubbles {
                        if last == Some(b.column) {
                            *cols.last_mut().expect("a column") += 1;
                        } else {
                            cols.push(1);
                            last = Some(b.column);
                        }
                    }
                    id_geom = cols.into_iter().map(&mut take).collect();
                    if !g.form_bubbles.is_empty() {
                        form_geom = Some(take(g.form_bubbles.len()));
                    }
                    question_geom = r
                        .questions
                        .iter()
                        .map(|qo| {
                            qg.iter()
                                .find(|(q, _)| *q == qo.question)
                                .map(|(_, g)| g.clone())
                                .unwrap_or_default()
                        })
                        .collect();
                }
                let questions_kept: Vec<QuestionOutcome> = r
                    .questions
                    .into_iter()
                    .filter(|q| q.question < *questions)
                    .collect();
                question_geom.truncate(questions_kept.len());
                let code = r.code;
                let reg = m.registration.as_ref();
                PageRead {
                    registered,
                    page_no,
                    theta_sel: theta_sel(&cfg, r.level),
                    hysteresis: cfg.hysteresis,
                    questions: questions_kept,
                    student_id: r.student_id,
                    form_choice: r.form,
                    code_form: code.map(|c| c.form),
                    code_serial: code.map(|c| c.serial.0),
                    exceptions: r.exceptions,
                    registration: json!({
                        "engine": "own",
                        "route": format!("{:?}", m.route),
                        "tags": reg.map(|s| s.tags_used.len()),
                        "rms_um": reg.map(|s| s.rms_um.round() as i64),
                        "px_per_mm_milli": reg.map(|s| milli(s.px_per_mm)),
                        "quarter_turns": reg.map(|s| s.quarter_turns),
                        "mirrored": reg.map(|s| s.mirrored),
                    }),
                    level: r.level,
                    question_geom,
                    id_geom,
                    form_geom,
                }
            }
            _ => unreachable!("engine, measurement and baseline kinds always match"),
        }
    }

    /// Option labels of the version/form choice (legacy: A–D; own: the form grid's labels).
    pub fn form_labels(&self) -> Vec<String> {
        match self {
            Recognizer::Legacy { engine, .. } => engine
                .template()
                .choice_labels(LegacyField::Version)
                .to_vec(),
            Recognizer::Own { engine, .. } => engine
                .template()
                .forms()
                .map(|f| f.letter().to_string())
                .collect(),
        }
    }

    /// Bubble radius at the working resolution (px), for crop overlays without a measurement.
    pub fn nominal_radius_px(&self) -> f32 {
        match self {
            Recognizer::Legacy { engine, .. } => engine.template().bubble_radius_px() as f32,
            Recognizer::Own { .. } => (2.1 * f64::from(WORKING_DPI) / 25.4) as f32,
        }
    }
}

fn milli(x: f64) -> i64 {
    if x.is_finite() {
        (x * 1000.0).round() as i64
    } else {
        0
    }
}

fn theta_sel(cfg: &DecisionConfig, level: Darkness) -> u16 {
    let rel = (u32::from(level.0) * u32::from(cfg.select_rel_permille) / 1000) as u16;
    cfg.select_abs.max(rel)
}

fn bubble_geom(b: &BubbleMeasure, beta: Darkness, option: u8) -> BubbleGeom {
    let delta = (i32::from(b.inner.0) - i32::from(beta.0)).max(0);
    BubbleGeom {
        option,
        cx: b.centre.x as f32,
        cy: b.centre.y as f32,
        r: b.radius_px as f32,
        fill: (delta as f32 / 1024.0).min(1.0),
        unreadable: b.unreadable.is_some(),
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// `(num, den)` = `dst / src` reduced.
pub fn reduce_ratio(dst: u32, src: u32) -> (u32, u32) {
    if dst == 0 || src == 0 {
        return (1, 1);
    }
    let g = gcd(dst, src);
    (dst / g, src / g)
}

/// Snaps an estimated resolution to a common scanner dpi within 10 %.
pub fn snap_dpi(est: f64) -> Option<u32> {
    const COMMON: [u32; 11] = [100, 150, 200, 240, 250, 300, 360, 400, 500, 600, 1200];
    if !est.is_finite() || est < 1.0 {
        return None;
    }
    COMMON
        .iter()
        .copied()
        .min_by(|&a, &b| {
            (est - f64::from(a))
                .abs()
                .total_cmp(&(est - f64::from(b)).abs())
        })
        .filter(|&c| (est - f64::from(c)).abs() <= 0.10 * f64::from(c))
}

/// The response a booklet holds before review: auto-accepted reads as decided, everything
/// else pending (unreadable stays unreadable: never blank).
pub fn initial_response(q: &QuestionOutcome) -> Response {
    q.response.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_and_snapping() {
        assert_eq!(reduce_ratio(200, 300), (2, 3));
        assert_eq!(reduce_ratio(200, 600), (1, 3));
        assert_eq!(reduce_ratio(200, 200), (1, 1));
        assert_eq!(snap_dpi(296.0), Some(300));
        assert_eq!(snap_dpi(203.0), Some(200));
        assert_eq!(snap_dpi(1.0e9), None);
    }
}
