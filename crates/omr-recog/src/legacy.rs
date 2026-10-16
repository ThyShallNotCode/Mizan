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
};
use crate::exc;
use crate::measure::{self, BubbleMeasure, Unreadable};
use crate::register::{self, RefModel, Registration, RegistrationQuality};
use omr_forms::legacy::{IAU_CCSIT_V1_BLANK_PNG, LegacyField, LegacyTemplate};
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
    pub read: IdGridRead,
    /// Per column: true if the column decision is not confident (review / roster decoding).
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
    /// True if decided confidently.
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
        let rm = RefModel::new(
            points,
            template.bubble_radius_px(),
            &blank,
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

    /// The blank-form print coverage β_p⁰ per bubble (template order).
    pub fn prior(&self) -> &[Darkness] {
        &self.prior
    }

    /// The integer box factor that brings a scan of `width × height` px to the template's
    /// working resolution (e.g. 3 for a 600-dpi page against a 200-dpi template); never < 1.
    /// Header DPI is distrusted (§8.2), so the factor comes from the page size.
    pub fn working_factor(&self, width: usize, height: usize) -> usize {
        let (tw, th) = self.template.frame_size();
        let r = (width as f64 / f64::from(tw)).max(height as f64 / f64::from(th));
        (r.round() as usize).max(1)
    }

    /// The page at working resolution: area-reduced by [`Self::working_factor`] (bitonal input
    /// becomes coverage gray, §7.2 bitonal mode). For packed 1-bit data use
    /// `omr_imaging::reduce_bitmap` with the same factor.
    pub fn to_working_resolution(&self, page: &GrayImage) -> GrayImage {
        let f = self.working_factor(page.width(), page.height());
        omr_imaging::reduce_area(page, f)
    }

    /// Page stage: registration and measurement (pure; safe to run in parallel per page).
    pub fn measure_page(&self, page: &GrayImage) -> PageMeasurement {
        let reg = self.register_page(page);
        self.measure_registered(page, reg)
    }

    /// Page stage, first half: registration only.
    pub fn register_page(&self, page: &GrayImage) -> Registration {
        register::register(page, &self.rm, &self.config.registration)
    }

    /// Page stage, second half: snapping and measurement under a registration of `page`.
    pub fn measure_registered(&self, page: &GrayImage, reg: Registration) -> PageMeasurement {
        let bubbles = measure::measure(
            page,
            &reg,
            &self.rm,
            &self.config.registration,
            &self.config.measure,
        );
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

    fn unreadable_exceptions(
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
        let cfg = &effective;
        let page = req.page.as_deref();
        let lq = if req.high_stakes {
            LogitMilli::HIGH_STAKES_LQ
        } else {
            LogitMilli::STANDARD_LQ
        };
        let t = &self.template;
        let n_q = req.n_questions.min(t.question_count());
        let qgroups = t.groups(LegacyField::Answers);
        // Page level from readable questions.
        let mut tops = Vec::new();
        for r in qgroups.iter().take(usize::from(n_q)) {
            let r = r.clone();
            if self.unreadable_in(m, r.clone()).is_empty() {
                let d = self.deltas(m, baseline, r);
                tops.push(d.iter().copied().max().unwrap_or(0));
            }
        }
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
            let d = self.deltas(m, baseline, r);
            let dec = decide_group(&d, level, cfg, req.high_stakes);
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
                let (state, marked) = match &dec.kind {
                    GroupKind::Blank => (ColumnState::Blank, vec![]),
                    GroupKind::Single(i) => (ColumnState::Single, vec![*i]),
                    GroupKind::Multi(v) => (ColumnState::Multi, v.clone()),
                };
                columns.push(IdColumnRead {
                    column: c as u8,
                    state,
                    marked,
                    logits: option_logits(&d, &dec, lq, cfg),
                });
                review.push(!dec.accepted);
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
            ChoiceRead {
                labels,
                response: response_of(&dec.kind),
                confident: dec.accepted,
                logit: decision_logit(&dec, lq),
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
        }
    }
}

fn response_of(kind: &GroupKind) -> Response {
    match kind {
        GroupKind::Blank => Response::Blank,
        GroupKind::Single(i) => Response::Single(*i),
        GroupKind::Multi(v) => Response::Multi(v.clone()),
    }
}

fn question_outcome(
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
