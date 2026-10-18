//! The own-form path (§6.3–§6.4, §8.2–§8.4, §8.11): the university's generated A4 sheets
//! with four AprilTag 36h11 corner tags, row ticks and a QR v3-M page code printed twice.
//!
//! **Page stage** ([`OwnFormEngine::measure_page`], pure; one task per page in `omr-app`):
//! 1. corner tags in the four image-corner windows, quad search on a decimated copy, decoding
//!    at native resolution; mirrored hypothesis if the IDs are missing or wound the wrong way
//!    ([`tags`], §8.3 steps 1–2);
//! 2. the 16 black-square corners refined to sub-pixel by edge-line fits ([`refine`]);
//! 3. Stage A: seeded RANSAC homography + normalised DLT, affine vs homography by AICc, one
//!    leave-one-out pass; orientation (REG-002), mirroring (REG-005), residual (REG-003),
//!    scale / skew / anisotropy (REG-004) gates ([`stage_a`], §8.4);
//! 4. both page-code copies decoded with `rxing` where Stage A puts them and verified with
//!    `omr-forms` under K_exam from a key-free [`CodeRegistry`]; IDN-001 / IDN-002 / COL-004
//!    per the §6.4 copy table ([`code`], §8.3 step 3);
//! 5. the verified page's bubbles mapped through the homography, shifted by the row-tick
//!    residuals where these show a non-homographic stretch ([`ticks`]), and measured as
//!    normalised darkness D = (B − I)/(B − K) over the 0.8 r₀ inner disk ([`measure`]).
//!
//! **Batch stage**: [`OwnFormEngine::batch_baseline`] (β_p per template page over the batch,
//! shared core) then [`OwnFormEngine::decide`] (shared core: page level, group decisions,
//! erasure rule, hysteresis, Appendix A exceptions; ID grids never auto-accepted).
//!
//! Key-blind (R2): K_exam authenticates sheets and is not an answer key. No I/O, no threads
//! created here (R4; `kornia-apriltag` runs its internal `rayon` loops on the caller's pool).

pub mod code;
pub mod config;
pub mod measure;
pub mod refine;
pub mod stage_a;
pub mod streaks;
pub mod tags;
pub mod ticks;

pub use code::{CodeOutcome, CodeRegistry, CopyRead, CopyStatus, PageIdentity, RegisteredExam};
pub use config::{
    CodeConfig, OWNFORM_CONFIG_VERSION, OwnFormConfig, OwnMeasureConfig, StageAConfig, TagConfig,
    TickConfig, TriggerConfig, gray_decision,
};
pub use measure::BubbleFeatures;
pub use stage_a::{DpiSource, FitModel, StageA};
pub use streaks::{Streak, Streaks};
pub use tags::{KorniaDetector, TagDetector, TagHit, TagSearch};
pub use ticks::{TickField, TickHit};

use crate::RecogError;
use crate::baseline::{self, Baseline};
use crate::decide::{decide_group, decision_logit, option_logits, page_level};
use crate::exc;
use crate::legacy::{
    ChoiceRead, GridField, LegacyEngine, column_state, question_outcome, response_of,
};
use crate::measure::{BubbleMeasure, Unreadable};
use measure::BubbleSpec;
use omr_forms::pagecode::PageCode;
use omr_forms::{PageGeometry, Template};
use omr_imaging::{CaptureKind, GrayImage, Homography};
use omr_model::{
    ColumnState, Darkness, EngineManifest, Exception, IdColumnRead, IdGridRead, LogitMilli, PageNo,
    PointPx, QuestionOutcome, Response, Severity,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

/// Per-page inputs that are not in the image (key-blind: no key, no policy, no roster).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnPageRequest {
    /// Capture DPI from the capture agent or device profile (§8.2 rule 1), if known.
    pub device_dpi: Option<u32>,
    /// Header DPI (§8.2 rule 2: used only if standard and implying a 0.80–1.10 scale).
    pub header_dpi: Option<u32>,
    /// Capture classification (bitonal → CAP-003 `bitonal`, hysteresis doubled).
    pub capture: CaptureKind,
    /// High-stakes mode (erasure rule review-only, L_q = 9.2 nat).
    pub high_stakes: bool,
    /// Page identifier for exception targets.
    pub page: Option<String>,
}

impl Default for OwnPageRequest {
    fn default() -> Self {
        Self {
            device_dpi: None,
            header_dpi: None,
            capture: CaptureKind::Gray,
            high_stakes: false,
            page: None,
        }
    }
}

/// Where a page goes after identification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageRoute {
    /// A page of this engine's template with a MAC-verified code (measured; auto-gradable
    /// unless a gate failed).
    ThisTemplate,
    /// A MAC-valid page of another registered exam or revision: hand it to that engine.
    OtherTemplate {
        /// 40-bit exam ID.
        exam_id40: u64,
        /// Template revision.
        revision: u16,
    },
    /// Tags of another tag set (headers, calibration, proofs; §6.4).
    OtherTagSet(u16),
    /// No verified code, or no tags. When the MAC-valid copies agree on one of this
    /// template's pages, or the template has a single page, the page is still measured
    /// provisionally so a reviewer sees proposals; nothing on it is auto-accepted.
    Unidentified,
}

/// What the page stage produces for one page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnPageMeasurement {
    /// The request it was measured under.
    pub request: OwnPageRequest,
    /// Corner-tag search.
    pub tags: TagSearch,
    /// Stage A, if at least two tags were usable.
    pub registration: Option<StageA>,
    /// How the capture DPI was established.
    pub dpi: Option<DpiSource>,
    /// Page code reads and outcome.
    pub identity: Option<PageIdentity>,
    /// Routing decision.
    pub route: PageRoute,
    /// The template page measured: from the verified code, or provisionally (see
    /// [`PageRoute::Unidentified`]).
    pub page: Option<PageNo>,
    /// Row-tick evidence.
    pub ticks: Option<TickField>,
    /// Scanner streaks masked from sampling (CAP-006).
    pub streaks: Vec<Streak>,
    /// One measurement per bubble of `page`: answers (question, option), then the ID grid
    /// (column, digit), then the form grid.
    pub bubbles: Vec<BubbleMeasure>,
    /// Paper level B per bubble (diagnostics).
    pub paper: Vec<u8>,
    /// Shape features per bubble (review triggers).
    pub features: Vec<BubbleFeatures>,
    /// Page-level exceptions: REG-001/002/003/004/005, IDN-001/002, COL-004, CAP-005/006.
    pub exceptions: Vec<Exception>,
    /// False when identity or registration forbids auto-acceptance (IDN-001 ERROR, IDN-002,
    /// COL-004, REG-003/004 ERROR, CAP-005 ERROR): decisions are proposals only.
    pub auto_accept_allowed: bool,
}

impl OwnPageMeasurement {
    /// The verified page code, if any.
    pub fn code(&self) -> Option<PageCode> {
        self.identity.as_ref().and_then(|i| i.code)
    }
}

/// Per-page baselines of a batch slice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnBaseline {
    /// β_p per template page (1-based page number).
    pub pages: BTreeMap<u16, Baseline>,
    /// Batch exceptions (MRK-016 `prior_model`).
    pub exceptions: Vec<Exception>,
}

/// Everything recognised on one own-form page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnPageResult {
    /// Routing.
    pub route: PageRoute,
    /// Template page.
    pub page: Option<PageNo>,
    /// Verified page code (form, page, booklet serial).
    pub code: Option<PageCode>,
    /// Stage A diagnostics.
    pub registration: Option<StageA>,
    /// Page mark level.
    pub level: Darkness,
    /// Questions on this page (0-based exam question numbers).
    pub questions: Vec<QuestionOutcome>,
    /// Student-ID grid (page 1): per-column reads and review flags for identity (§9.5); ID
    /// columns are never auto-accepted (§8.8).
    pub student_id: Option<GridField>,
    /// Form grid (page 1, if printed).
    pub form: Option<ChoiceRead>,
    /// Page-level exceptions (REG-*, IDN-*, COL-004, CAP-003/005/006, MRK-015).
    pub exceptions: Vec<Exception>,
}

/// One template page prepared for measurement.
#[derive(Clone, Debug)]
struct PageModel {
    specs: Vec<BubbleSpec>,
    questions: Vec<(u16, Range<usize>)>,
    id_columns: Vec<Range<usize>>,
    form: Option<(Range<usize>, Vec<String>)>,
    fields: Vec<Range<usize>>,
    prior: Vec<Darkness>,
}

impl PageModel {
    fn new(t: &Template, g: &PageGeometry, default_prior: u16) -> Self {
        let mut specs = Vec::new();
        let mut questions = Vec::new();
        let mut fields = Vec::new();
        let mm = |p: omr_model::PointMm| PointPx { x: p.x, y: p.y };
        for q in g.questions() {
            let start = specs.len();
            for b in t.question_bubbles(q) {
                specs.push(BubbleSpec {
                    centre: mm(b.centre_mm()),
                    radius: b.radius_mm(),
                    question: Some(q),
                });
            }
            questions.push((q, start..specs.len()));
        }
        if !specs.is_empty() {
            fields.push(0..specs.len());
        }
        let id_start = specs.len();
        let mut id_columns: Vec<Range<usize>> = Vec::new();
        let mut last_col: Option<u8> = None;
        for b in &g.id_bubbles {
            let i = specs.len();
            specs.push(BubbleSpec {
                centre: mm(b.centre_mm()),
                radius: b.radius_mm(),
                question: None,
            });
            match id_columns.last_mut() {
                Some(r) if last_col == Some(b.column) => r.end = i + 1,
                _ => {
                    id_columns.push(i..i + 1);
                    last_col = Some(b.column);
                }
            }
        }
        if specs.len() > id_start {
            fields.push(id_start..specs.len());
        }
        let form = if g.form_bubbles.is_empty() {
            None
        } else {
            let start = specs.len();
            for b in &g.form_bubbles {
                specs.push(BubbleSpec {
                    centre: mm(b.centre_mm()),
                    radius: b.radius_mm(),
                    question: None,
                });
            }
            fields.push(start..specs.len());
            Some((
                start..specs.len(),
                g.form_bubbles.iter().map(|b| b.label.clone()).collect(),
            ))
        };
        let prior = vec![Darkness(default_prior); specs.len()];
        Self {
            specs,
            questions,
            id_columns,
            form,
            fields,
            prior,
        }
    }
}

/// The image region searched for streaks: the columns of the A4 safe box's bounding box under
/// `h` (inset by 2 mm), and the rows its quad covers on every column (so the backing is never
/// sampled). A near-vertical paper edge, the only page feature that looks like a streak, lies
/// outside these columns.
fn printed_region(h: &Homography, img: &GrayImage) -> [usize; 4] {
    use omr_imaging::Warp;
    let sb = omr_forms::geometry::SAFE_BOX_A4.inflate(-2_000);
    let pts = [
        (sb.x, sb.y),
        (sb.right(), sb.y),
        (sb.right(), sb.bottom()),
        (sb.x, sb.bottom()),
    ]
    .map(|(x, y)| {
        h.map(PointPx {
            x: f64::from(x) / 1000.0,
            y: f64::from(y) / 1000.0,
        })
    });
    // Outer extremes for columns, inner extremes for rows.
    let mut xs: Vec<f64> = pts.iter().map(|p| p.x).collect();
    let mut ys: Vec<f64> = pts.iter().map(|p| p.y).collect();
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    let clamp = |v: f64, hi: usize| -> usize {
        if v.is_finite() {
            (v.max(0.0) as usize).min(hi)
        } else {
            0
        }
    };
    [
        clamp(xs[0].floor(), img.width()),
        clamp(ys[1].ceil(), img.height()),
        clamp(xs[3].ceil(), img.width()),
        clamp(ys[2].floor(), img.height()),
    ]
}

/// Why a trigger routed a decided group to review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trigger {
    /// Ink outside a marked bubble's outline (cross-out): MRK-004.
    CrossOut(u8),
    /// A stroke-like selected bubble (tick or X): MRK-005.
    Tick(u8),
    /// A resolved erasure whose residue is as dark as a light fill: MRK-003.
    Erasure(u8),
    /// `fill_cancels_check`: a fill cancels, a tick selects; the classical cascade cannot
    /// tell them apart, so every mark is reviewed: MRK-005.
    CheckMode,
}

impl Trigger {
    fn exception(self, page: Option<&str>, q: u16) -> Exception {
        match self {
            Trigger::CrossOut(i) => exc::item("MRK-004", Severity::Error, page, q).with_detail(
                format!("option {i}: ink outside the bubble (cross-out?); class cascade deferred"),
            ),
            Trigger::Tick(i) => exc::item("MRK-005", Severity::Error, page, q).with_detail(
                format!("option {i}: stroke-like mark (tick or X) instead of a fill"),
            ),
            Trigger::Erasure(i) => exc::item("MRK-003", Severity::Error, page, q).with_detail(
                format!("option {i}: residue as dark as a light fill; erasure not resolved"),
            ),
            Trigger::CheckMode => exc::item("MRK-005", Severity::Error, page, q).with_detail(
                "fill_cancels_check: ticks select and fills cancel; reviewed until the class cascade ships",
            ),
        }
    }
}

/// The classical router triggers (§8.7 step 3 stand-in) for a decided group.
fn trigger(
    cfg: &TriggerConfig,
    mode: omr_model::CancelMode,
    blank_max: i32,
    deltas: &[i32],
    feats: &[BubbleFeatures],
    inner: &[Darkness],
    dec: &crate::GroupDecision,
) -> Option<Trigger> {
    let kind = &dec.kind;
    for (i, (d, f)) in deltas.iter().zip(feats).enumerate() {
        if *d >= blank_max && f.annulus_ink_permille >= cfg.cross_annulus_permille {
            return Some(Trigger::CrossOut(i as u8));
        }
    }
    let selected: Vec<u8> = match kind {
        crate::GroupKind::Blank => Vec::new(),
        crate::GroupKind::Single(i) => vec![*i],
        crate::GroupKind::Multi(v) => v.clone(),
    };
    if mode == omr_model::CancelMode::FillCancelsCheck && !selected.is_empty() {
        return Some(Trigger::CheckMode);
    }
    if dec.resolved_erasure
        && let Some((i, _)) = deltas
            .iter()
            .enumerate()
            .filter(|(i, _)| !selected.contains(&(*i as u8)))
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0)))
            .filter(|(_, d)| **d >= i32::from(cfg.erasure_residue_max))
    {
        return Some(Trigger::Erasure(i as u8));
    }
    for &i in &selected {
        let j = usize::from(i);
        let (Some(d), Some(f), Some(m)) = (deltas.get(j), feats.get(j), inner.get(j)) else {
            continue;
        };
        let dmi = i32::from(f.dark_permille) * 1024 - i32::from(m.0) * 1000;
        let [lo, hi] = cfg
            .tick_dark_minus_inner_permille
            .map(|v| i32::from(v) * 1024);
        if *d <= i32::from(cfg.tick_max_delta)
            && (cfg.tick_dark_permille[0]..=cfg.tick_dark_permille[1]).contains(&f.dark_permille)
            && (lo..=hi).contains(&dmi)
        {
            return Some(Trigger::Tick(i));
        }
    }
    None
}

/// The engine for one own-form template revision and configuration.
#[derive(Clone)]
pub struct OwnFormEngine {
    template: Template,
    config: OwnFormConfig,
    detector: Arc<dyn TagDetector>,
    pages: Vec<PageModel>,
    template_sha384: String,
    config_sha384: String,
}

impl std::fmt::Debug for OwnFormEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnFormEngine")
            .field("template_sha384", &self.template_sha384)
            .field("config_sha384", &self.config_sha384)
            .field("detector", &self.detector.name())
            .finish_non_exhaustive()
    }
}

impl OwnFormEngine {
    /// Builds the engine for `template` with `kornia-apriltag` as the tag detector. Until
    /// [`OwnFormEngine::with_blank_page`] is called, the prior β_p⁰ is the configured constant.
    pub fn new(template: Template, config: OwnFormConfig) -> Self {
        let pages = template
            .pages()
            .iter()
            .map(|g| PageModel::new(&template, g, config.measure.default_prior))
            .collect();
        let template_sha384 = template.hash_hex();
        let config_sha384 = config.sha384_hex();
        Self {
            template,
            config,
            detector: Arc::new(KorniaDetector),
            pages,
            template_sha384,
            config_sha384,
        }
    }

    /// Replaces the tag detector (fallback detectors, tests).
    pub fn with_detector(mut self, detector: Arc<dyn TagDetector>) -> Self {
        self.detector = detector;
        self
    }

    /// Sets the prior β_p⁰ of `page` from an unmarked render of it (e.g.
    /// `omr_forms::render::rasterize_static` at `dpi`): the bubbles are measured exactly where
    /// the layout puts them (`x_px = x_mm · dpi / 25.4`), with K = 0 (100 % K print).
    pub fn with_blank_page(
        mut self,
        page: PageNo,
        blank: &GrayImage,
        dpi: f64,
    ) -> Result<Self, RecogError> {
        let idx = usize::from(page.0)
            .checked_sub(1)
            .filter(|&i| i < self.pages.len())
            .ok_or_else(|| RecogError::Template(format!("no page {}", page.0)))?;
        let s = dpi / 25.4;
        let h = Homography {
            h: [s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0],
        };
        let model = &self.pages[idx];
        let (m, _, _) =
            measure::measure_bubbles(blank, &h, None, &model.specs, 0, &[], &self.config.measure);
        if m.iter().any(|b| !b.readable()) {
            return Err(RecogError::Template(format!(
                "blank render of page {} does not cover every bubble",
                page.0
            )));
        }
        self.pages[idx].prior = m.iter().map(|b| b.inner).collect();
        Ok(self)
    }

    /// The template.
    pub fn template(&self) -> &Template {
        &self.template
    }

    /// The configuration.
    pub fn config(&self) -> &OwnFormConfig {
        &self.config
    }

    /// The tag detector's name.
    pub fn detector_name(&self) -> &'static str {
        self.detector.name()
    }

    /// SHA-384 (hex) of the template's layout manifest.
    pub fn template_sha384(&self) -> &str {
        &self.template_sha384
    }

    /// SHA-384 (hex) of the canonical configuration.
    pub fn config_sha384(&self) -> &str {
        &self.config_sha384
    }

    /// Stamps the template and configuration hashes into a manifest (§14.6).
    pub fn stamp(&self, manifest: &mut EngineManifest) {
        manifest.template_sha384 = self.template_sha384.clone();
        manifest.config_sha384 = self.config_sha384.clone();
    }

    /// The prior β_p⁰ of a page's bubbles (answers, ID grid, form grid order).
    pub fn prior(&self, page: PageNo) -> Option<&[Darkness]> {
        self.model(page).map(|m| m.prior.as_slice())
    }

    fn model(&self, page: PageNo) -> Option<&PageModel> {
        usize::from(page.0)
            .checked_sub(1)
            .and_then(|i| self.pages.get(i))
    }

    fn failed(
        &self,
        request: &OwnPageRequest,
        tags: TagSearch,
        route: PageRoute,
        exceptions: Vec<Exception>,
    ) -> OwnPageMeasurement {
        OwnPageMeasurement {
            request: request.clone(),
            tags,
            registration: None,
            dpi: None,
            identity: None,
            route,
            page: None,
            ticks: None,
            streaks: Vec::new(),
            bubbles: Vec::new(),
            paper: Vec::new(),
            features: Vec::new(),
            exceptions,
            auto_accept_allowed: false,
        }
    }

    /// Page stage: tags, Stage A, page code, ticks and bubble measurement (pure; safe to run
    /// in parallel per page). `registry` holds page-code secrets only (R2).
    pub fn measure_page(
        &self,
        gray: &GrayImage,
        request: &OwnPageRequest,
        registry: &CodeRegistry,
    ) -> OwnPageMeasurement {
        let label = request.page.as_deref();
        let pe = |code: &str, sev: Severity| exc::on_page(exc::page(code, sev), label);
        let cfg = &self.config;
        let search = tags::search(self.detector.as_ref(), gray, &cfg.tags);
        let Some(set) = search.set else {
            let e = pe("REG-001", Severity::Blocker).with_detail(format!(
                "fewer than 2 corner tags of one known set (found IDs {:?}); foreign-form path",
                search.all_ids
            ));
            return self.failed(request, search, PageRoute::Unidentified, vec![e]);
        };
        if set != cfg.tags.answer_tag_set {
            let e = pe("IDN-002", Severity::Blocker).with_detail(format!(
                "tag set {set} is not an answer page (header, calibration or proof); routed elsewhere"
            ));
            return self.failed(request, search, PageRoute::OtherTagSet(set), vec![e]);
        }
        let Some(first) = self.template.pages().first() else {
            return self.failed(request, search, PageRoute::Unidentified, Vec::new());
        };
        let mirrored = search.mirrored;
        let stage = match stage_a::fit(
            gray,
            first,
            &search.tags,
            mirrored,
            &cfg.registration,
            &cfg.tags,
        ) {
            Ok(a) => a,
            Err(_) => {
                let e = pe("REG-001", Severity::Blocker)
                    .with_detail("corner tags found but no usable fit (degenerate geometry)");
                return self.failed(request, search, PageRoute::Unidentified, vec![e]);
            }
        };
        let dpi = stage_a::capture_dpi(
            request.device_dpi,
            request.header_dpi,
            stage.dpi_eff(),
            &cfg.registration,
        );
        let (mut exceptions, reg_failed) = stage_a::gates(&stage, dpi, &cfg.registration, label);
        let mut allowed = !reg_failed;

        // Page code (§8.3 step 3).
        let identity = code::read_page(
            gray,
            &stage.h,
            &self.template,
            mirrored,
            &cfg.code,
            registry,
            label,
        );
        exceptions.extend(identity.exceptions.iter().cloned());
        let own = |c: &PageCode| {
            c.exam_id40 == self.template.exam_id40() && c.revision == self.template.spec().revision
        };
        let mut route = PageRoute::Unidentified;
        let page: Option<PageNo> = match (identity.verified(), identity.code) {
            (true, Some(c)) if !own(&c) => {
                route = PageRoute::OtherTemplate {
                    exam_id40: c.exam_id40,
                    revision: c.revision.0,
                };
                None
            }
            (true, Some(c)) => match code::page_of(&self.template, &c) {
                Some(_) => {
                    route = PageRoute::ThisTemplate;
                    Some(c.page)
                }
                None => {
                    exceptions.push(pe("IDN-002", Severity::Blocker).with_detail(format!(
                        "verified page code names page {} which this template revision lacks",
                        c.page.0
                    )));
                    allowed = false;
                    None
                }
            },
            _ => {
                // Not verified (IDN-001 ERROR, IDN-002, COL-004): never auto-graded. The page
                // is still measured provisionally, so a reviewer sees proposals, when the
                // MAC-valid copies agree on one of this template's pages, or the template has
                // a single page.
                allowed = false;
                let valid: Vec<PageCode> = identity.copies.iter().filter_map(|c| c.code).collect();
                match valid.first() {
                    Some(c)
                        if own(c)
                            && valid.iter().all(|v| v.page == c.page)
                            && code::page_of(&self.template, c).is_some() =>
                    {
                        Some(c.page)
                    }
                    _ => (self.template.page_count() == 1).then_some(PageNo(1)),
                }
            }
        };
        let Some(page_no) = page else {
            return OwnPageMeasurement {
                request: request.clone(),
                tags: search,
                registration: Some(stage),
                dpi: Some(dpi),
                identity: Some(identity),
                route,
                page: None,
                ticks: None,
                streaks: Vec::new(),
                bubbles: Vec::new(),
                paper: Vec::new(),
                features: Vec::new(),
                exceptions,
                auto_accept_allowed: false,
            };
        };
        let (Some(model), Some(geom)) = (self.model(page_no), self.template.page(page_no)) else {
            return self.failed(request, search, PageRoute::Unidentified, exceptions);
        };
        let ticks = ticks::locate(
            gray,
            &stage.h,
            geom,
            stage.black,
            stage.px_per_mm,
            &cfg.ticks,
            stage.tags_used.len() < 3,
        );
        let streaks = streaks::detect(
            gray,
            printed_region(&stage.h, gray),
            cfg.measure.streak_min_delta,
            cfg.measure.streak_max_width,
            cfg.measure.streak_row_step.max(1) as usize,
        );
        for b in &streaks.bands {
            exceptions.push(
                Exception::must("CAP-006", Severity::Warn, omr_model::Scope::Region)
                    .with_target(omr_model::exceptions::Target {
                        page: label.map(str::to_owned),
                        ..Default::default()
                    })
                    .with_detail(format!(
                        "scanner streak at columns {}–{} ({:+} gray levels): masked from sampling; clean the glass",
                        b.x,
                        b.x + b.width - 1,
                        b.delta
                    )),
            );
        }
        let (bubbles, paper, features) = measure::measure_bubbles(
            gray,
            &stage.h,
            Some(&ticks),
            &model.specs,
            stage.black,
            &streaks.mask,
            &cfg.measure,
        );
        // Contrast gate (§8.9: B − K < 80 WARN, < 50 sheet review).
        let mut ps: Vec<u8> = paper
            .iter()
            .zip(&bubbles)
            .filter(|(_, b)| b.readable())
            .map(|(p, _)| *p)
            .collect();
        ps.sort_unstable();
        if let Some(&b) = ps.get(ps.len() / 2) {
            let contrast = b.saturating_sub(stage.black);
            if contrast < cfg.measure.contrast_error {
                allowed = false;
                exceptions.push(pe("CAP-005", Severity::Error).with_detail(format!(
                    "contrast B − K = {contrast} < {}: sheet review",
                    cfg.measure.contrast_error
                )));
            } else if contrast < cfg.measure.contrast_warn {
                exceptions.push(pe("CAP-005", Severity::Warn).with_detail(format!(
                    "contrast B − K = {contrast} < {}",
                    cfg.measure.contrast_warn
                )));
            }
        }
        if identity.outcome == CodeOutcome::CopiesDisagree {
            allowed = false;
        }
        OwnPageMeasurement {
            request: request.clone(),
            tags: search,
            registration: Some(stage),
            dpi: Some(dpi),
            identity: Some(identity),
            route,
            page: Some(page_no),
            ticks: Some(ticks),
            streaks: streaks.bands,
            bubbles,
            paper,
            features,
            exceptions,
            auto_accept_allowed: allowed,
        }
    }

    /// Single-page baseline for `page`: the prior only.
    pub fn prior_baseline(&self, page: PageNo) -> Option<Baseline> {
        self.model(page)
            .map(|m| baseline::prior_baseline(&m.prior, &self.config.baseline))
    }

    /// Batch baselines (order-independent) over the measured pages of this template whose
    /// identity is verified; pages are grouped by template page.
    pub fn batch_baseline(&self, pages: &[&OwnPageMeasurement]) -> OwnBaseline {
        let mut out = OwnBaseline {
            pages: BTreeMap::new(),
            exceptions: Vec::new(),
        };
        for (i, model) in self.pages.iter().enumerate() {
            let no = (i + 1) as u16;
            let slices: Vec<&[BubbleMeasure]> = pages
                .iter()
                .filter(|m| {
                    m.page == Some(PageNo(no))
                        && m.route == PageRoute::ThisTemplate
                        && m.auto_accept_allowed
                        && m.bubbles.len() == model.specs.len()
                })
                .map(|m| m.bubbles.as_slice())
                .collect();
            let b = if slices.is_empty() {
                baseline::prior_baseline(&model.prior, &self.config.baseline)
            } else {
                baseline::batch_baseline(
                    &slices,
                    &model.prior,
                    &model.fields,
                    &self.config.baseline,
                )
            };
            for e in &b.exceptions {
                if !out.exceptions.contains(e) {
                    out.exceptions.push(e.clone());
                }
            }
            out.pages.insert(no, b);
        }
        out
    }

    /// Single-page convenience: measure, prior baseline, decide.
    pub fn recognize_single(
        &self,
        gray: &GrayImage,
        request: &OwnPageRequest,
        registry: &CodeRegistry,
    ) -> OwnPageResult {
        let m = self.measure_page(gray, request, registry);
        let b = self.batch_baseline(&[]);
        self.decide(&m, &b)
    }

    /// Batch stage for one page: level, questions, ID grid and form grid.
    pub fn decide(&self, m: &OwnPageMeasurement, baseline: &OwnBaseline) -> OwnPageResult {
        let req = &m.request;
        let label = req.page.as_deref();
        let mut exceptions = m.exceptions.clone();
        if req.capture.is_bitonal() {
            exceptions.push(
                exc::batch("CAP-003", Severity::Warn)
                    .with_sub_reason("bitonal")
                    .with_detail("bitonal capture: fill-fraction mode; rescan in 8-bit gray"),
            );
        }
        let empty = |exceptions| OwnPageResult {
            route: m.route,
            page: m.page,
            code: m.code(),
            registration: m.registration.clone(),
            level: Darkness(0),
            questions: Vec::new(),
            student_id: None,
            form: None,
            exceptions,
        };
        let Some(page) = m.page else {
            return empty(exceptions);
        };
        let Some(model) = self.model(page) else {
            return empty(exceptions);
        };
        // A measurement from another engine or a damaged record: nothing is decided.
        if m.bubbles.len() != model.specs.len() || m.features.len() != m.bubbles.len() {
            return empty(exceptions);
        }
        let prior;
        let beta: &Baseline = match baseline.pages.get(&page.0) {
            Some(b) if b.beta.len() == model.specs.len() => b,
            _ => {
                prior = baseline::prior_baseline(&model.prior, &self.config.baseline);
                &prior
            }
        };
        // Bitonal captures are their own slice (§8.5): the thresholds validated on real bitonal
        // sheets, hysteresis doubled (§8.7 step 7).
        let mut cfg = if req.capture.is_bitonal() {
            self.config.decision_bitonal.clone()
        } else {
            self.config.decision.clone()
        };
        if req.capture.is_bitonal() {
            cfg.hysteresis = cfg.hysteresis.saturating_mul(cfg.bitonal_hysteresis_factor);
        }
        let lq = if req.high_stakes {
            LogitMilli::HIGH_STAKES_LQ
        } else {
            LogitMilli::STANDARD_LQ
        };
        let deltas = |r: &Range<usize>| -> Vec<i32> {
            r.clone()
                .map(|j| i32::from(m.bubbles[j].inner.0) - i32::from(beta.beta[j].0))
                .collect()
        };
        let unread = |r: &Range<usize>| -> Vec<(u8, Unreadable)> {
            r.clone()
                .filter_map(|j| m.bubbles[j].unreadable.map(|c| ((j - r.start) as u8, c)))
                .collect()
        };
        let tops = model
            .questions
            .iter()
            .filter(|(_, r)| unread(r).is_empty())
            .map(|(_, r)| deltas(r).into_iter().max().unwrap_or(0));
        let level = page_level(tops, &cfg);

        let mut questions = Vec::with_capacity(model.questions.len());
        let mut all_blank = !model.questions.is_empty();
        for (q, r) in &model.questions {
            let u = unread(r);
            if !u.is_empty() {
                all_blank = false;
                questions.push(QuestionOutcome {
                    question: *q,
                    response: Response::Unreadable {
                        bubbles: u.iter().map(|(i, _)| *i).collect(),
                    },
                    auto_accepted: false,
                    logit: LogitMilli(0),
                    exceptions: LegacyEngine::unreadable_exceptions(
                        &u,
                        label,
                        Some(*q),
                        "question",
                    ),
                });
                continue;
            }
            let d = deltas(r);
            let dec = decide_group(&d, level, &cfg, req.high_stakes);
            if !(dec.accepted && dec.kind == crate::GroupKind::Blank) {
                all_blank = false;
            }
            let mut o = question_outcome(*q, &dec, lq, label);
            if dec.accepted
                && let Some(t) = trigger(
                    &self.config.triggers,
                    self.template.cancel_mode(),
                    i32::from(cfg.blank_max),
                    &d,
                    &m.features[r.clone()],
                    &m.bubbles[r.clone()]
                        .iter()
                        .map(|b| b.inner)
                        .collect::<Vec<_>>(),
                    &dec,
                )
            {
                o.auto_accepted = false;
                o.logit = LogitMilli(0);
                o.exceptions = vec![t.exception(label, *q)];
            }
            if !m.auto_accept_allowed {
                o.auto_accepted = false;
            }
            questions.push(o);
        }

        let student_id = (!model.id_columns.is_empty()).then(|| {
            let mut columns = Vec::new();
            let mut review = Vec::new();
            let mut ex = Vec::new();
            for (c, r) in model.id_columns.iter().enumerate() {
                let u = unread(r);
                if !u.is_empty() {
                    columns.push(IdColumnRead {
                        column: c as u8,
                        state: ColumnState::Unreadable,
                        marked: vec![],
                        logits: vec![LogitMilli(0); r.len()],
                    });
                    review.push(true);
                    ex.extend(LegacyEngine::unreadable_exceptions(
                        &u,
                        label,
                        None,
                        &format!("student ID column {c}"),
                    ));
                    continue;
                }
                let d = deltas(r);
                let dec = decide_group(&d, level, &cfg, req.high_stakes);
                let triggered = trigger(
                    &self.config.triggers,
                    omr_model::CancelMode::EraseOnly,
                    i32::from(cfg.blank_max),
                    &d,
                    &m.features[r.clone()],
                    &m.bubbles[r.clone()]
                        .iter()
                        .map(|b| b.inner)
                        .collect::<Vec<_>>(),
                    &dec,
                )
                .is_some();
                // The `IdColumnRead` contract (`ColumnState::Single` = a confident digit, which
                // `exact_digits` and omr-identity rely on): a column demoted by a trigger or an
                // unverified page is reported like any review column (`column_state`), with
                // per-digit logits inside (−L_q, L_q).
                let mut shown = dec.clone();
                shown.accepted = dec.accepted && !triggered && m.auto_accept_allowed;
                let (state, marked) = column_state(&shown);
                columns.push(IdColumnRead {
                    column: c as u8,
                    state,
                    marked,
                    logits: option_logits(&d, &shown, lq, &cfg),
                });
                review.push(!shown.accepted || state == ColumnState::Multi);
            }
            GridField {
                read: IdGridRead { columns },
                review,
                exceptions: ex,
            }
        });

        let form = model.form.as_ref().map(|(r, labels)| {
            let u = unread(r);
            if !u.is_empty() {
                return ChoiceRead {
                    labels: labels.clone(),
                    response: Response::Unreadable {
                        bubbles: u.iter().map(|(i, _)| *i).collect(),
                    },
                    confident: false,
                    logit: LogitMilli(0),
                    exceptions: LegacyEngine::unreadable_exceptions(&u, label, None, "form grid"),
                };
            }
            let d = deltas(r);
            let dec = decide_group(&d, level, &cfg, req.high_stakes);
            let triggered = trigger(
                &self.config.triggers,
                omr_model::CancelMode::EraseOnly,
                i32::from(cfg.blank_max),
                &d,
                &m.features[r.clone()],
                &m.bubbles[r.clone()]
                    .iter()
                    .map(|b| b.inner)
                    .collect::<Vec<_>>(),
                &dec,
            )
            .is_some();
            // As on the legacy path: a single-choice field with several marks is never a
            // confident value; nor is one a trigger demoted or on an unverified page.
            let confident = dec.accepted
                && !triggered
                && m.auto_accept_allowed
                && !matches!(dec.kind, crate::GroupKind::Multi(_));
            ChoiceRead {
                labels: labels.clone(),
                response: response_of(&dec.kind),
                confident,
                logit: if confident {
                    decision_logit(&dec, lq)
                } else {
                    LogitMilli(0)
                },
                exceptions: vec![],
            }
        });

        if all_blank {
            exceptions.push(
                exc::booklet("MRK-015", Severity::Error, label)
                    .with_detail("every question on the page is blank: never auto-scored 0"),
            );
        }
        OwnPageResult {
            route: m.route,
            page: m.page,
            code: m.code(),
            registration: m.registration.clone(),
            level: Darkness(level.clamp(0, 1024) as u16),
            questions,
            student_id,
            form,
            exceptions,
        }
    }
}
