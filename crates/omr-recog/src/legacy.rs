//! The legacy-form engine (§6.10 forms without fiducials; validated on the IAU CCSIT sheet).
//!
//! Two stages, as in §8.1:
//! - **page stage** [`LegacyEngine::measure_page`]: a pure function of the page image —
//!   registration, snapping, per-bubble inner/core darkness ([`PageMeasurement`]);
//! - **batch stage** [`LegacyEngine::batch_baseline`] then [`LegacyEngine::decide`]: β_p over
//!   the batch's pages (or [`LegacyEngine::prior_baseline`] for a single page), the page mark
//!   level and the group decisions ([`LegacyPageResult`]).
//!
//! Recognition is key-blind (R2): nothing here sees an answer key.

use crate::RecogError;
use crate::baseline::{self, Baseline};
use crate::config::LegacyConfig;
use crate::decide::{
    GroupDecision, GroupKind, decide_group, decision_logit, option_logits, page_level,
    photo_residue_level,
};
use crate::exc;
use crate::measure::{self, BubbleMeasure, Unreadable};
use crate::register::{self, RefModel, Registration, RegistrationQuality};
use omr_forms::legacy::{IAU_CCSIT_V1_BLANK_PNG, LegacyField, LegacyTemplate};
use omr_imaging::Warp;
use omr_imaging::{CaptureKind, GrayImage, QuadWarp};
use omr_model::{
    ColumnState, Darkness, EngineManifest, Exception, IdColumnRead, IdGridRead, LogitMilli,
    QuestionOutcome, Response, Severity,
};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// What the page stage produces for one page.
#[derive(Clone, Debug)]
pub struct PageMeasurement {
    /// Registration diagnostics.
    pub registration: RegistrationQuality,
    /// The fitted warp (template px → page px), if registration succeeded.
    pub warp: Option<QuadWarp>,
    /// Page-level registration exceptions (REG-001/003/004).
    pub registration_exceptions: Vec<Exception>,
    /// One measurement per template bubble, in template order.
    pub bubbles: Vec<BubbleMeasure>,
}

impl PageMeasurement {
    /// True if the page registered.
    pub fn registered(&self) -> bool {
        self.warp.is_some()
    }
}

/// Per-page decision inputs that are not in the image (key-blind: no key, no policy).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRequest {
    /// Questions in this exam (the form prints more).
    pub n_questions: u16,
    /// High-stakes mode (erasure rule review-only, multi review per policy, L_q = 9.2 nat).
    pub high_stakes: bool,
    /// Capture classification (bitonal → CAP-003 `bitonal`).
    pub capture: CaptureKind,
    /// Page identifier for exception targets.
    pub page: Option<String>,
}

/// One bubbled grid (student number, section, date) as read from a page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridField {
    /// The per-column reads (never auto-accepted here; identity decodes them, §8.8, §9.5).
    /// `ColumnState::Single` only for a confident digit; an unconfident single proposal is
    /// `Multi` with the candidate in `marked` (see `column_state`).
    pub read: IdGridRead,
    /// Per column: true if the column is not a confident single digit or confident blank
    /// (review / roster decoding): unconfident, multi-marked or unreadable.
    pub review: Vec<bool>,
    /// Unreadable causes (CAP-009, CAP-002, REG-003; Region).
    pub exceptions: Vec<Exception>,
}

impl GridField {
    /// True if every column was decided confidently.
    pub fn confident(&self) -> bool {
        !self.review.iter().any(|&r| r)
    }

    /// One character per column: the digit, `_` for a confident blank, `?` otherwise.
    pub fn text(&self) -> String {
        self.read
            .columns
            .iter()
            .zip(&self.review)
            .map(|(c, &rev)| match (rev, c.state, c.marked.as_slice()) {
                (false, ColumnState::Single, [d]) => char::from(b'0' + d),
                (false, ColumnState::Blank, _) => '_',
                _ => '?',
            })
            .collect()
    }
}

/// A single-choice field (version, exam type, gender).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceRead {
    /// Labels in printed order (option indices refer to these).
    pub labels: Vec<String>,
    /// Decided or proposed response.
    pub response: Response,
    /// True if decided confidently as a single label or blank (several marks never are).
    pub confident: bool,
    /// Margin score of the response.
    pub logit: LogitMilli,
    /// Unreadable causes (Region).
    pub exceptions: Vec<Exception>,
}

impl ChoiceRead {
    /// The label of a confident single choice.
    pub fn value(&self) -> Option<&str> {
        match (&self.response, self.confident) {
            (Response::Single(i), true) => self.labels.get(usize::from(*i)).map(String::as_str),
            _ => None,
        }
    }

    /// `Some("")` for a confident blank, the label for a confident single, else `None`.
    pub fn text(&self) -> Option<&str> {
        match (&self.response, self.confident) {
            (Response::Blank, true) => Some(""),
            _ => self.value(),
        }
    }
}

/// Everything recognised on one legacy page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyPageResult {
    /// Registration diagnostics.
    pub registration: RegistrationQuality,
    /// Page mark level (median clear top-1 δ, clamped).
    pub level: Darkness,
    /// Questions `0..n_questions`.
    pub questions: Vec<QuestionOutcome>,
    /// Student-number grid.
    pub student_number: GridField,
    /// Section grid.
    pub section: GridField,
    /// Date grid.
    pub date: GridField,
    /// Gender.
    pub gender: ChoiceRead,
    /// Version (form).
    pub version: ChoiceRead,
    /// Exam type.
    pub exam: ChoiceRead,
    /// Page-level exceptions: REG-*, CAP-003 `bitonal`, MRK-015.
    pub exceptions: Vec<Exception>,
    /// Decided as a phone photo (§8.10): a camera capture, or a page whose registration took
    /// the camera path, needed a one-pitch correction or found the axes' scales more than the
    /// anisotropy warning apart. The wider camera band applied; the phone slice is not
    /// calibrated (§8.10 step 7), so the application sends such pages' answers to review.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub photo: bool,
}

/// The engine for one legacy template revision and configuration.
#[derive(Clone, Debug)]
pub struct LegacyEngine {
    template: LegacyTemplate,
    blank: GrayImage,
    config: LegacyConfig,
    rm: RefModel,
    prior: Vec<Darkness>,
    fields: Vec<Range<usize>>,
    template_sha384: String,
    config_sha384: String,
}

impl LegacyEngine {
    /// Builds the engine from a template, the blank-form image bytes (PNG/JPEG/TIFF) and a
    /// configuration.
    pub fn new(
        template: LegacyTemplate,
        blank_image: &[u8],
        config: LegacyConfig,
    ) -> Result<Self, RecogError> {
        let blank = omr_imaging::decode(blank_image)
            .map_err(|e| RecogError::Template(format!("blank form: {e}")))?
            .image;
        let sha = template.revision_sha384_hex(blank_image);
        Self::with_blank(template, blank, sha, config)
    }

    /// The committed IAU CCSIT template with [`LegacyConfig::bitonal_v1`].
    pub fn iau_ccsit_v1() -> Result<Self, RecogError> {
        Self::new(
            LegacyTemplate::iau_ccsit_v1(),
            IAU_CCSIT_V1_BLANK_PNG,
            LegacyConfig::bitonal_v1(),
        )
    }

    /// Builds the engine from an already decoded blank form and its template hash.
    pub fn with_blank(
        template: LegacyTemplate,
        blank: GrayImage,
        template_sha384: String,
        config: LegacyConfig,
    ) -> Result<Self, RecogError> {
        let (w, h) = template.frame_size();
        if blank.width() != w as usize || blank.height() != h as usize {
            return Err(RecogError::Template(format!(
                "blank form is {}x{}, template frame is {w}x{h}",
                blank.width(),
                blank.height()
            )));
        }
        let points = template.bubbles().iter().map(|b| b.centre).collect();
        let (regions, row_labelled) = lattice_regions(&template);
        let rm = RefModel::new(
            points,
            template.bubble_radius_px(),
            &blank,
            &regions,
            &row_labelled,
            &config.registration,
        );
        let prior = measure::prior_inner(&blank, &rm, &config.measure);
        let fields = LegacyField::ALL
            .iter()
            .map(|&f| template.field_range(f))
            .collect();
        let config_sha384 = config.sha384_hex();
        Ok(Self {
            template,
            blank,
            config,
            rm,
            prior,
            fields,
            template_sha384,
            config_sha384,
        })
    }

    /// The template.
    pub fn template(&self) -> &LegacyTemplate {
        &self.template
    }

    /// The blank form.
    pub fn blank(&self) -> &GrayImage {
        &self.blank
    }

    /// The configuration.
    pub fn config(&self) -> &LegacyConfig {
        &self.config
    }

    /// SHA-384 (hex) of the template revision (JSON digest + blank image bytes).
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

    #[cfg(test)]
    pub(crate) fn rm_for_tests(&self) -> &RefModel {
        &self.rm
    }

    /// The blank-form print coverage β_p⁰ per bubble (template order).
    pub fn prior(&self) -> &[Darkness] {
        &self.prior
    }

    /// The nearest integer box factor for a scan of `width × height` px (e.g. 3 for a 600-dpi
    /// page against a 200-dpi template); never < 1. Header DPI is distrusted (§8.2), so the
    /// factor comes from the page size. Only exact for integer ratios: use
    /// [`Self::working_ratio`] / [`Self::to_working_resolution`] for any ratio (300 dpi = 2/3).
    pub fn working_factor(&self, width: usize, height: usize) -> usize {
        let (tw, th) = self.template.frame_size();
        let r = (width as f64 / f64::from(tw)).max(height as f64 / f64::from(th));
        (r.round() as usize).max(1)
    }

    /// The capture resolution implied by a page of `width × height` px (header DPI distrusted,
    /// §8.2): the page/frame ratio × the template dpi, snapped to a common scanner resolution
    /// when within 10 % (scanners auto-crop, so the page size is only approximate), else
    /// rounded to 10 dpi.
    pub fn estimated_dpi(&self, width: usize, height: usize) -> u32 {
        const COMMON: [u32; 13] = [
            100, 150, 200, 240, 250, 300, 360, 400, 500, 600, 720, 800, 1200,
        ];
        let (tw, th) = self.template.frame_size();
        let r = (width as f64 / f64::from(tw)).max(height as f64 / f64::from(th));
        let est = r * self.template.dpi();
        if !est.is_finite() || est < 1.0 {
            return self.template.dpi().round().max(1.0) as u32;
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
            .unwrap_or_else(|| ((est / 10.0).round() as u32).max(1) * 10)
    }

    /// The rational scale `(num, den)` (output px per input px, reduced) that brings a scan of
    /// `width × height` px to the template's working resolution: template dpi ÷
    /// [`Self::estimated_dpi`]. `(1, 3)` for a 600-dpi page against the 200-dpi IAU template,
    /// `(2, 3)` for 300 dpi, `(1, 2)` for 400 dpi; `(1, 1)` at the template resolution.
    pub fn working_ratio(&self, width: usize, height: usize) -> (u32, u32) {
        let dst = self.template.dpi().round().max(1.0) as u32;
        let src = self.estimated_dpi(width, height);
        let g = gcd_u32(dst, src);
        (dst / g, src / g)
    }

    /// The page at working resolution, by exact area coverage at [`Self::working_ratio`]
    /// (`omr_imaging::resample_area`; an integer ratio is the box reduction of
    /// [`Self::working_factor`], so 600 → 200 dpi is unchanged). Bitonal input becomes coverage
    /// gray (§7.2 bitonal mode). For packed 1-bit data at an integer ratio use
    /// `omr_imaging::reduce_bitmap`.
    pub fn to_working_resolution(&self, page: &GrayImage) -> GrayImage {
        let (num, den) = self.working_ratio(page.width(), page.height());
        omr_imaging::resample_area(page, num, den)
    }

    /// Page stage: registration and measurement (pure; safe to run in parallel per page).
    ///
    /// Bitonal captures (1-bit or its coverage reduction, told from a sampled histogram) are
    /// read in the §8.5 bitonal mode (`B = 255`, `K = 0`) exactly as validated on the real IAU
    /// scans. Gray captures (gray scans, phone photos) are read against their paper background
    /// `B` (closing + blur at 1/4), and their coarse correlation and verification levels are
    /// flattened by it, so shadows, vignetting and colour casts neither shift the alignment nor
    /// read as marks.
    pub fn measure_page(&self, page: &GrayImage) -> PageMeasurement {
        let pyr = self.pyramid(page);
        let reg = register::register_with(page, &pyr, &self.rm, &self.config.registration);
        self.measure_with(page, reg, pyr.background())
    }

    /// Page stage, first half: registration only.
    pub fn register_page(&self, page: &GrayImage) -> Registration {
        let pyr = self.pyramid(page);
        register::register_with(page, &pyr, &self.rm, &self.config.registration)
    }

    /// Page stage, second half: snapping and measurement under a registration of `page`.
    pub fn measure_registered(&self, page: &GrayImage, reg: Registration) -> PageMeasurement {
        if reg.quality.paper_normalised {
            let pyr = self.pyramid(page);
            self.measure_with(page, reg, pyr.background())
        } else {
            self.measure_with(page, reg, None)
        }
    }

    /// The registration pyramid: flattened by the paper background for gray captures.
    fn pyramid<'a>(&self, page: &'a GrayImage) -> register::Pyramid<'a> {
        let cfg = &self.config.registration;
        let kind = omr_imaging::classify_capture_sampled(page, cfg.capture_row_step as usize);
        if kind.is_bitonal() {
            register::Pyramid::new(page)
        } else {
            register::Pyramid::flattened(page, cfg.background_radius_px)
        }
    }

    fn measure_with(
        &self,
        page: &GrayImage,
        mut reg: Registration,
        background: Option<&GrayImage>,
    ) -> PageMeasurement {
        let bubbles = measure::measure(
            page,
            &reg,
            &self.rm,
            &self.config.registration,
            &self.config.measure,
            background,
        );
        if let (Some(bg), Some(w)) = (background, reg.warp.as_ref()) {
            let levels: Vec<u8> = self
                .rm
                .points
                .iter()
                .map(|t| {
                    let p = w.map(*t);
                    omr_imaging::bilinear(bg, p.x / 4.0, p.y / 4.0, 255)
                })
                .collect();
            reg.quality.illumination_cv = omr_imaging::illumination(&levels).cv;
        }
        PageMeasurement {
            registration: reg.quality,
            warp: reg.warp,
            registration_exceptions: reg.exceptions,
            bubbles,
        }
    }

    /// Single-page baseline: the blank-form prior only.
    pub fn prior_baseline(&self) -> Baseline {
        baseline::prior_baseline(&self.prior, &self.config.baseline)
    }

    /// Batch baseline over the measured pages (order-independent).
    pub fn batch_baseline(&self, pages: &[&PageMeasurement]) -> Baseline {
        let slices: Vec<&[BubbleMeasure]> = pages.iter().map(|p| p.bubbles.as_slice()).collect();
        baseline::batch_baseline(&slices, &self.prior, &self.fields, &self.config.baseline)
    }

    /// Single-page convenience: measure, prior baseline, decide.
    pub fn recognize_single(&self, page: &GrayImage, req: &PageRequest) -> LegacyPageResult {
        let m = self.measure_page(page);
        self.decide(&m, &self.prior_baseline(), req)
    }

    fn deltas(&self, m: &PageMeasurement, b: &Baseline, range: Range<usize>) -> Vec<i32> {
        range
            .map(|j| i32::from(m.bubbles[j].inner.0) - i32::from(b.beta[j].0))
            .collect()
    }

    fn unreadable_in(&self, m: &PageMeasurement, range: Range<usize>) -> Vec<(u8, Unreadable)> {
        range
            .clone()
            .filter_map(|j| {
                m.bubbles[j]
                    .unreadable
                    .map(|c| ((j - range.start) as u8, c))
            })
            .collect()
    }

    pub(crate) fn unreadable_exceptions(
        cause: &[(u8, Unreadable)],
        page: Option<&str>,
        q: Option<u16>,
        what: &str,
    ) -> Vec<Exception> {
        let mut codes: Vec<&'static str> = cause.iter().map(|(_, c)| c.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        codes
            .into_iter()
            .map(|code| {
                let bubbles: Vec<u8> = cause
                    .iter()
                    .filter(|(_, c)| c.code() == code)
                    .map(|(i, _)| *i)
                    .collect();
                let bubble = (bubbles.len() == 1).then(|| bubbles[0]);
                exc::region(code, Severity::Error, page, q, bubble).with_detail(format!(
                    "{what}: bubble(s) {bubbles:?} unreadable, never blank"
                ))
            })
            .collect()
    }

    /// Batch stage for one page: level, questions, grids and choices.
    pub fn decide(
        &self,
        m: &PageMeasurement,
        baseline: &Baseline,
        req: &PageRequest,
    ) -> LegacyPageResult {
        let mut effective = self.config.decision.clone();
        if req.capture.is_bitonal() {
            effective.hysteresis = effective
                .hysteresis
                .saturating_mul(effective.bitonal_hysteresis_factor);
        }
        // Phone geometry: the camera slice is not calibrated (§8.10 step 7), so the ambiguity
        // band widens. A photo is recognised by its capture (the application infers it from
        // the file: a colour JPEG below 200 ppi, a camera's EXIF) or by its geometry: the
        // camera path, a verified one-pitch correction, or anisotropy above the REG-004 warning.
        let camera = req.capture.is_camera()
            || m.registration.method.is_camera()
            || m.registration.method == register::RegistrationMethod::LatticeCorrected
            || m.registration.anisotropy.abs() * 1000.0
                > f64::from(self.config.registration.anisotropy_warn_permille);
        if camera {
            effective.hysteresis = effective
                .hysteresis
                .saturating_mul(effective.camera_hysteresis_factor);
        }
        let page = req.page.as_deref();
        let lq = if req.high_stakes {
            LogitMilli::HIGH_STAKES_LQ
        } else {
            LogitMilli::STANDARD_LQ
        };
        let t = &self.template;
        let n_q = req.n_questions.min(t.question_count());
        let qgroups = t.groups(LegacyField::Answers);
        // Page level from readable questions, and on a photographed page the distribution of
        // its unmarked bubbles (every row's options but the darkest).
        let photo = camera;
        let mut tops = Vec::new();
        let mut unmarked = Vec::new();
        for r in qgroups.iter().take(usize::from(n_q)) {
            let r = r.clone();
            if self.unreadable_in(m, r.clone()).is_empty() {
                let mut d = self.deltas(m, baseline, r);
                tops.push(d.iter().copied().max().unwrap_or(0));
                if photo {
                    d.sort_unstable();
                    d.pop();
                    unmarked.extend(d);
                }
            }
        }
        // C2: on a photo the absolute residue level (0.10) sits inside the unmarked bubbles'
        // own spread, so every answer would carry MRK-003 and the key summary would highlight
        // every row; a residue must stand out from the page's unmarked bubbles instead.
        if photo {
            effective.residue_log = photo_residue_level(&mut unmarked, &effective);
        }
        let cfg = &effective;
        let level = page_level(tops, cfg);

        let mut questions = Vec::with_capacity(usize::from(n_q));
        let mut all_blank = n_q > 0;
        for q in 0..n_q {
            let r = qgroups[usize::from(q)].clone();
            let unread = self.unreadable_in(m, r.clone());
            if !unread.is_empty() {
                all_blank = false;
                questions.push(QuestionOutcome {
                    question: q,
                    response: Response::Unreadable {
                        bubbles: unread.iter().map(|(i, _)| *i).collect(),
                    },
                    auto_accepted: false,
                    logit: LogitMilli(0),
                    exceptions: Self::unreadable_exceptions(&unread, page, Some(q), "question"),
                });
                continue;
            }
            let d = self.deltas(m, baseline, r.clone());
            let dec = decide_group(&d, level, cfg, req.high_stakes);
            // Ring visibility (§8.6): an unmarked bubble shows its printed ring. A row read as
            // blank where no ring of the row was seen is not on the page where the alignment
            // puts it (covered, cut or folded away): unreadable, never blank.
            if dec.kind == GroupKind::Blank
                && m.bubbles
                    .get(r.clone())
                    .is_some_and(|b| b.iter().all(|x| !x.snapped))
            {
                all_blank = false;
                questions.push(QuestionOutcome {
                    question: q,
                    response: Response::Unreadable {
                        bubbles: (0..r.len() as u8).collect(),
                    },
                    auto_accepted: false,
                    logit: LogitMilli(0),
                    exceptions: vec![
                        exc::region("REG-003", Severity::Error, page, Some(q), None).with_detail(
                            "question: no printed bubble ring of this row is visible where the \
                             alignment puts it (covered, cut or folded away); unreadable, never \
                             blank",
                        ),
                    ],
                });
                continue;
            }
            if !(dec.accepted && dec.kind == GroupKind::Blank) {
                all_blank = false;
            }
            questions.push(question_outcome(q, &dec, lq, page));
        }

        let grid = |field: LegacyField| -> GridField {
            let mut columns = Vec::new();
            let mut review = Vec::new();
            let mut exceptions = Vec::new();
            for (c, r) in t.groups(field).iter().enumerate() {
                let unread = self.unreadable_in(m, r.clone());
                if !unread.is_empty() {
                    columns.push(IdColumnRead {
                        column: c as u8,
                        state: ColumnState::Unreadable,
                        marked: vec![],
                        logits: vec![LogitMilli(0); r.len()],
                    });
                    review.push(true);
                    exceptions.extend(Self::unreadable_exceptions(
                        &unread,
                        page,
                        None,
                        &format!("{field:?} column {c}"),
                    ));
                    continue;
                }
                let d = self.deltas(m, baseline, r.clone());
                let dec = decide_group(&d, level, cfg, req.high_stakes);
                let (state, marked) = column_state(&dec);
                columns.push(IdColumnRead {
                    column: c as u8,
                    state,
                    marked,
                    logits: option_logits(&d, &dec, lq, cfg),
                });
                review.push(!dec.accepted || state == ColumnState::Multi);
            }
            GridField {
                read: IdGridRead { columns },
                review,
                exceptions,
            }
        };

        let choice = |field: LegacyField| -> ChoiceRead {
            let r = t.groups(field)[0].clone();
            let labels = t.choice_labels(field).to_vec();
            let unread = self.unreadable_in(m, r.clone());
            if !unread.is_empty() {
                return ChoiceRead {
                    labels,
                    response: Response::Unreadable {
                        bubbles: unread.iter().map(|(i, _)| *i).collect(),
                    },
                    confident: false,
                    logit: LogitMilli(0),
                    exceptions: Self::unreadable_exceptions(
                        &unread,
                        page,
                        None,
                        &format!("{field:?}"),
                    ),
                };
            }
            let d = self.deltas(m, baseline, r);
            let dec = decide_group(&d, level, cfg, req.high_stakes);
            // A single-choice field with several marks is never a confident value.
            let confident = dec.accepted && !matches!(dec.kind, GroupKind::Multi(_));
            ChoiceRead {
                labels,
                response: response_of(&dec.kind),
                confident,
                logit: if confident {
                    decision_logit(&dec, lq)
                } else {
                    LogitMilli(0)
                },
                exceptions: vec![],
            }
        };

        let mut exceptions: Vec<Exception> = m
            .registration_exceptions
            .iter()
            .cloned()
            .map(|e| exc::on_page(e, page))
            .collect();
        if req.capture.is_bitonal() {
            exceptions.push(
                exc::batch("CAP-003", Severity::Warn)
                    .with_sub_reason("bitonal")
                    .with_detail("bitonal capture: fill-fraction mode; rescan in 8-bit gray"),
            );
        }
        let cv = m.registration.illumination_cv;
        if m.registered()
            && cv * 1000.0 > f64::from(self.config.registration.illumination_cv_warn_permille)
        {
            exceptions.push(exc::page("CAP-005", Severity::Warn).with_detail(format!(
                "uneven light: the paper level under the bubbles varies by {:.0} % (CV); read \
                 against the paper background, but rescan or retake in even light",
                cv * 100.0
            )));
        }
        if all_blank {
            exceptions.push(
                exc::booklet("MRK-015", Severity::Error, page)
                    .with_detail("every question is blank: blank booklet, never auto-scored 0"),
            );
        }
        LegacyPageResult {
            registration: m.registration.clone(),
            level: Darkness(level.clamp(0, 1024) as u16),
            questions,
            student_number: grid(LegacyField::StudentNumber),
            section: grid(LegacyField::Section),
            date: grid(LegacyField::Date),
            gender: choice(LegacyField::Gender),
            version: choice(LegacyField::Version),
            exam: choice(LegacyField::Exam),
            exceptions,
            photo: camera,
        }
    }
}

/// The template's lattice regions for the shift-sensitive verification (§8.4): every field
/// but the answers is one region, and the answers split into their printed blocks (questions
/// whose first option lies within one option pitch horizontally of the previous question's).
/// Also, per region, whether its rows are labelled outside the bubbles (the answer blocks'
/// question numbers), which the row-by-row check reads; the header grids' rows are told apart
/// only inside their bubbles, under the students' marks and handwriting.
fn lattice_regions(t: &LegacyTemplate) -> (Vec<Vec<usize>>, Vec<bool>) {
    let mut regions = Vec::new();
    let groups = t.groups(LegacyField::Answers);
    let mut block: Vec<usize> = Vec::new();
    let mut last_x: Option<f64> = None;
    let pitch = groups
        .first()
        .and_then(|g| {
            let b = t.bubbles();
            (g.len() >= 2).then(|| (b[g.start + 1].centre.x - b[g.start].centre.x).abs())
        })
        .unwrap_or(50.0);
    for g in groups {
        let x = t.bubbles()[g.start].centre.x;
        if last_x.is_some_and(|lx| (x - lx).abs() > pitch) && !block.is_empty() {
            regions.push(std::mem::take(&mut block));
        }
        block.extend(g.clone());
        last_x = Some(x);
    }
    if !block.is_empty() {
        regions.push(block);
    }
    let answers = regions.len();
    for f in LegacyField::ALL {
        if f != LegacyField::Answers {
            let r = t.field_range(f);
            if !r.is_empty() {
                regions.push(r.collect());
            }
        }
    }
    let labelled = (0..regions.len()).map(|r| r < answers).collect();
    (regions, labelled)
}

fn gcd_u32(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// The `omr_model::IdColumnRead` state of an ID-grid column decision. The contract
/// (`ColumnState::Single` = "exactly one digit **confidently** marked", which
/// `IdGridRead::exact_digits` and rosterless identity rely on) forbids reporting an unconfident
/// proposal as `Single`: a single digit proposed for review is reported as `Multi` with that
/// candidate in `marked` (an ambiguous column; its per-digit logits stay inside (−L_q, L_q)).
/// Blank proposals stay `Blank` (a wildcard column, never an exact digit).
pub(crate) fn column_state(dec: &GroupDecision) -> (ColumnState, Vec<u8>) {
    match &dec.kind {
        GroupKind::Blank => (ColumnState::Blank, vec![]),
        GroupKind::Single(i) if dec.accepted => (ColumnState::Single, vec![*i]),
        GroupKind::Single(i) => (ColumnState::Multi, vec![*i]),
        GroupKind::Multi(v) => (ColumnState::Multi, v.clone()),
    }
}

pub(crate) fn response_of(kind: &GroupKind) -> Response {
    match kind {
        GroupKind::Blank => Response::Blank,
        GroupKind::Single(i) => Response::Single(*i),
        GroupKind::Multi(v) => Response::Multi(v.clone()),
    }
}

pub(crate) fn question_outcome(
    q: u16,
    dec: &GroupDecision,
    lq: LogitMilli,
    page: Option<&str>,
) -> QuestionOutcome {
    let mut exceptions = Vec::new();
    if dec.accepted {
        if dec.resolved_erasure {
            exceptions.push(
                exc::item("MRK-003", Severity::Info, page, q)
                    .with_sub_reason("resolved_erasure")
                    .with_detail("erasure residue next to a fill; the fill was accepted"),
            );
        }
        if matches!(dec.kind, GroupKind::Multi(_)) {
            exceptions.push(
                exc::item("MRK-002", Severity::Info, page, q)
                    .with_detail("confident multiple marks, auto-scored under multi_review"),
            );
        }
    } else if let Some(why) = dec.review {
        let (code, sub) = why.code();
        let mut e = exc::item(code, Severity::Error, page, q).with_detail(format!("{why:?}"));
        if let Some(s) = sub {
            e = e.with_sub_reason(s);
        }
        exceptions.push(e);
    }
    QuestionOutcome {
        question: q,
        response: response_of(&dec.kind),
        auto_accepted: dec.accepted,
        logit: decision_logit(dec, lq),
        exceptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DecisionConfig;

    /// The `IdColumnRead` contract: `Single` only for a confident digit, so
    /// `IdGridRead::exact_digits` never returns a digit that was only proposed for review.
    #[test]
    fn unconfident_id_digit_is_never_reported_as_single() {
        let cfg = DecisionConfig::default();
        // Confident fill of digit 3.
        let d = [0, 0, 0, 900, 0, 0, 0, 0, 0, 0];
        let ok = decide_group(&d, 900, &cfg, false);
        assert!(ok.accepted);
        assert_eq!(column_state(&ok), (ColumnState::Single, vec![3]));
        // A lone weak mark on digit 7 (review, proposed Single(7)).
        let d = [0, 0, 0, 0, 0, 0, 0, 300, 0, 0];
        let weak = decide_group(&d, 900, &cfg, false);
        assert!(!weak.accepted);
        assert_eq!(weak.kind, GroupKind::Single(7));
        let (state, marked) = column_state(&weak);
        assert_eq!((state, marked.as_slice()), (ColumnState::Multi, &[7u8][..]));
        let grid = IdGridRead {
            columns: vec![IdColumnRead {
                column: 0,
                state,
                marked,
                logits: option_logits(&d, &weak, LogitMilli::STANDARD_LQ, &cfg),
            }],
        };
        assert_eq!(grid.exact_digits(), None);
        assert!(grid.columns[0].logits[7].0.abs() < LogitMilli::STANDARD_LQ.0);
        // A review blank stays a (wildcard) blank, never a digit.
        let near = decide_group(&[78, 0, 0, 0, 0, 0, 0, 0, 0, 0], 900, &cfg, false);
        assert!(!near.accepted);
        assert_eq!(column_state(&near), (ColumnState::Blank, vec![]));
    }
}
