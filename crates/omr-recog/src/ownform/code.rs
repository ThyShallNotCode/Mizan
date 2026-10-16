//! Page code (§6.4, §8.3 step 3): both QR copies are cropped where Stage A says they are,
//! decoded with `rxing` 0.9.3 (retry on a 2× upsample), parsed and verified with
//! `omr_forms::pagecode` under the per-exam page-code secret K_exam held in a [`CodeRegistry`].
//!
//! K_exam authenticates the sheet; it is **not** an answer key, so recognition stays key-blind
//! (R2). The registry never prints or logs a key (`ExamKey`'s `Debug` is redacted).
//!
//! Copy outcomes (§6.4, §8.3, Appendix A):
//!
//! | Copies | Outcome | Exception |
//! |---|---|---|
//! | all decoded copies MAC-valid and equal | [`CodeOutcome::Verified`] | — |
//! | MAC-valid copies that differ | [`CodeOutcome::CopiesDisagree`] | COL-004 `code_copies_disagree` (ERROR, Booklet) |
//! | exactly one MAC-valid, the other fails its MAC | [`CodeOutcome::OneCopyValid`] | IDN-001 `one_copy_invalid` (INFO) |
//! | exactly one MAC-valid, the other unreadable | [`CodeOutcome::OneCopyValid`] | IDN-001 (INFO) |
//! | MAC-valid but unknown template revision | [`CodeOutcome::UnknownRevision`] | IDN-002 (BLOCKER) |
//! | something decoded, nothing MAC-valid (forged, foreign, unknown exam, misread) | [`CodeOutcome::Invalid`] | IDN-002 (BLOCKER) |
//! | nothing decoded | [`CodeOutcome::NotDecoded`] | IDN-001 `unresolved` (ERROR) |

use super::config::CodeConfig;
use crate::exc;
use omr_forms::pagecode::{self, PageCode};
use omr_forms::{ExamKey, PageGeometry, Template};
use omr_imaging::{GrayImage, Homography, Warp, bilinear};
use omr_model::{Exception, PointPx, Scope, Severity, TemplateRevision, exceptions::Target};
use serde::{Deserialize, Serialize};

/// One exam's page-code secret and the template revisions it has issued.
#[derive(Clone, Debug)]
pub struct RegisteredExam {
    /// The 40-bit exam ID printed in the code (`Template::exam_id40`).
    pub exam_id40: u64,
    /// K_exam (redacted in `Debug`).
    pub key: ExamKey,
    /// Template revisions printed for this exam.
    pub revisions: Vec<TemplateRevision>,
}

/// Page-code secrets of the exams a project knows (§6.4: "decoding tests the tag under each
/// candidate exam", since 40-bit IDs can collide). Holds no answer key, policy or roster (R2).
#[derive(Clone, Debug, Default)]
pub struct CodeRegistry {
    exams: Vec<RegisteredExam>,
}

impl CodeRegistry {
    /// An empty registry (every code is then unknown: IDN-002).
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry holding one template's exam and revision.
    pub fn for_template(template: &Template, key: ExamKey) -> Self {
        let mut r = Self::new();
        r.register(template.exam_id40(), key, vec![template.spec().revision]);
        r
    }

    /// Adds an exam (or more revisions of an exam already registered under the same key).
    pub fn register(&mut self, exam_id40: u64, key: ExamKey, revisions: Vec<TemplateRevision>) {
        if let Some(e) = self
            .exams
            .iter_mut()
            .find(|e| e.exam_id40 == exam_id40 && e.key == key)
        {
            for r in revisions {
                if !e.revisions.contains(&r) {
                    e.revisions.push(r);
                }
            }
            return;
        }
        self.exams.push(RegisteredExam {
            exam_id40,
            key,
            revisions,
        });
    }

    /// Candidate exams for a 40-bit exam ID, in registration order.
    pub fn candidates(&self, exam_id40: u64) -> impl Iterator<Item = &RegisteredExam> {
        self.exams.iter().filter(move |e| e.exam_id40 == exam_id40)
    }

    /// Number of registered exams.
    pub fn len(&self) -> usize {
        self.exams.len()
    }

    /// True if nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.exams.is_empty()
    }
}

/// What happened to one printed copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CopyStatus {
    /// No QR symbol decoded (after the retry chain).
    NotDecoded,
    /// Decoded text that is not a well-formed payload (length, alphabet, scheme, pad).
    Malformed,
    /// Well-formed, but no registered exam has this 40-bit ID.
    UnknownExam,
    /// Well-formed, but the tag verifies under no candidate key (forged or misread).
    MacInvalid,
    /// The 96-bit tag verifies.
    Valid,
}

/// One copy's read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CopyRead {
    /// Copy index in the layout (0 = upper).
    pub copy: u8,
    /// Status.
    pub status: CopyStatus,
    /// Decoded text (no PII by design, §6.4), if any.
    pub payload: Option<String>,
    /// The verified fields (only when [`CopyStatus::Valid`]).
    pub code: Option<PageCode>,
    /// True if the template revision of a valid copy is registered.
    pub revision_known: bool,
    /// Decoder attempts used (1 = first try, 2 = 2× upsample).
    pub attempts: u8,
}

/// Page-level outcome of the copies (table in the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodeOutcome {
    /// Every decoded copy is MAC-valid and they agree.
    Verified,
    /// Exactly one MAC-valid copy, used (IDN-001 INFO).
    OneCopyValid,
    /// MAC-valid copies differ (COL-004).
    CopiesDisagree,
    /// Valid MAC, unknown template revision (IDN-002).
    UnknownRevision,
    /// Something decoded but nothing is MAC-valid (IDN-002).
    Invalid,
    /// Nothing decoded (IDN-001 `unresolved`).
    NotDecoded,
}

/// The page identity read from the codes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageIdentity {
    /// Outcome.
    pub outcome: CodeOutcome,
    /// The code used for routing: the (first) MAC-valid copy.
    pub code: Option<PageCode>,
    /// Per copy.
    pub copies: Vec<CopyRead>,
    /// IDN-001 / IDN-002 / COL-004.
    pub exceptions: Vec<Exception>,
}

impl PageIdentity {
    /// True if a MAC-valid code with a known revision routes the page (verified or one valid
    /// copy). Disagreeing copies are not verified: the booklet is split by a human (COL-004).
    pub fn verified(&self) -> bool {
        matches!(
            self.outcome,
            CodeOutcome::Verified | CodeOutcome::OneCopyValid
        )
    }
}

fn upsample2(img: &GrayImage) -> GrayImage {
    GrayImage::from_fn(img.width() * 2, img.height() * 2, |x, y| {
        bilinear(img, (x as f64 + 0.5) / 2.0, (y as f64 + 0.5) / 2.0, 255)
    })
}

fn rxing_qr(img: &GrayImage) -> Option<String> {
    if img.width() < 21 || img.height() < 21 {
        return None;
    }
    // A decoder panic on hostile input must not take the batch down (§22.1): treat it as
    // "not decoded".
    std::panic::catch_unwind(|| {
        rxing::helpers::detect_in_luma(
            img.as_raw().to_vec(),
            img.width() as u32,
            img.height() as u32,
            Some(rxing::BarcodeFormat::QR_CODE),
        )
        .ok()
        .map(|r| r.getText().to_owned())
    })
    .ok()
    .flatten()
}

/// Distinct code-slot rectangles over the template's pages, in (page, copy) order.
pub(crate) fn slots(template: &Template) -> Vec<(u8, omr_forms::RectUm)> {
    let mut out: Vec<(u8, omr_forms::RectUm)> = Vec::new();
    for page in template.pages() {
        for c in &page.codes {
            if !out.iter().any(|(_, r)| *r == c.rect) {
                out.push((c.copy, c.rect));
            }
        }
    }
    out
}

/// Crops and decodes one copy: first at native resolution, then (if configured) on a 2×
/// upsample. A mirrored page's crop is un-mirrored first.
pub(crate) fn read_copy(
    img: &GrayImage,
    h: &Homography,
    rect: omr_forms::RectUm,
    mirrored: bool,
    cfg: &CodeConfig,
) -> (Option<String>, u8) {
    let r = rect.inflate(cfg.crop_margin_um as i32);
    let corners = [
        (r.x, r.y),
        (r.right(), r.y),
        (r.right(), r.bottom()),
        (r.x, r.bottom()),
    ]
    .map(|(x, y)| {
        h.map(PointPx {
            x: f64::from(x) / 1000.0,
            y: f64::from(y) / 1000.0,
        })
    });
    let x0 = corners
        .iter()
        .map(|p| p.x)
        .fold(f64::INFINITY, f64::min)
        .floor();
    let y0 = corners
        .iter()
        .map(|p| p.y)
        .fold(f64::INFINITY, f64::min)
        .floor();
    let x1 = corners
        .iter()
        .map(|p| p.x)
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil();
    let y1 = corners
        .iter()
        .map(|p| p.y)
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil();
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return (None, 0);
    }
    let x0 = x0.max(0.0);
    let y0 = y0.max(0.0);
    let x1 = x1.min(img.width() as f64);
    let y1 = y1.min(img.height() as f64);
    if x1 - x0 < 21.0 || y1 - y0 < 21.0 {
        return (None, 0);
    }
    let mut crop = img.crop(
        x0 as isize,
        y0 as isize,
        (x1 - x0) as usize,
        (y1 - y0) as usize,
        255,
    );
    if mirrored {
        crop = super::tags::mirror(&crop);
    }
    if let Some(t) = rxing_qr(&crop) {
        return (Some(t), 1);
    }
    if cfg.retry_upsample
        && let Some(t) = rxing_qr(&upsample2(&crop))
    {
        return (Some(t), 2);
    }
    (None, if cfg.retry_upsample { 2 } else { 1 })
}

fn classify(copy: u8, payload: Option<String>, attempts: u8, reg: &CodeRegistry) -> CopyRead {
    let mut read = CopyRead {
        copy,
        status: CopyStatus::NotDecoded,
        payload: payload.clone(),
        code: None,
        revision_known: false,
        attempts,
    };
    let Some(text) = payload else {
        return read;
    };
    let Ok(parsed) = pagecode::parse(&text) else {
        read.status = CopyStatus::Malformed;
        return read;
    };
    let mut any = false;
    read.status = CopyStatus::UnknownExam;
    for cand in reg.candidates(parsed.code.exam_id40) {
        any = true;
        if let Ok(code) = parsed.verify(&cand.key) {
            read.status = CopyStatus::Valid;
            read.revision_known = cand.revisions.contains(&code.revision);
            read.code = Some(code);
            return read;
        }
    }
    if any {
        read.status = CopyStatus::MacInvalid;
    }
    read
}

/// Applies the §6.4 copy-outcome table to the per-copy reads.
pub(crate) fn identify(
    reads: Vec<(u8, Option<String>, u8)>,
    reg: &CodeRegistry,
    page: Option<&str>,
) -> PageIdentity {
    let copies: Vec<CopyRead> = reads
        .into_iter()
        .map(|(c, p, a)| classify(c, p, a, reg))
        .collect();
    let valid: Vec<&CopyRead> = copies
        .iter()
        .filter(|c| c.status == CopyStatus::Valid)
        .collect();
    let pe = |code: &str, sev: Severity| exc::on_page(exc::page(code, sev), page);
    let mut exceptions = Vec::new();
    let code = valid.first().and_then(|c| c.code);
    let outcome = if valid.len() >= 2 && valid.iter().any(|c| c.payload != valid[0].payload) {
        exceptions.push(
            Exception::must("COL-004", Severity::Error, Scope::Booklet)
                .with_sub_reason("code_copies_disagree")
                .with_target(Target {
                    page: page.map(str::to_owned),
                    booklet: code.map(|c| c.serial.0),
                    ..Target::default()
                })
                .with_detail(
                    "both page-code copies are MAC-valid but differ: split into provisional booklets",
                ),
        );
        CodeOutcome::CopiesDisagree
    } else if let Some(v) = valid.first() {
        if !v.revision_known {
            exceptions.push(pe("IDN-002", Severity::Blocker).with_detail(format!(
                "valid page-code MAC but unknown template revision {}: assign a matching revision; never auto-graded",
                v.code.map_or(0, |c| c.revision.0)
            )));
            CodeOutcome::UnknownRevision
        } else if valid.len() < copies.len() {
            let other_invalid = copies.iter().any(|c| {
                matches!(
                    c.status,
                    CopyStatus::Malformed | CopyStatus::MacInvalid | CopyStatus::UnknownExam
                )
            });
            let mut e = pe("IDN-001", Severity::Info);
            if other_invalid {
                e = e
                    .with_sub_reason("one_copy_invalid")
                    .with_detail("one page-code copy fails its MAC; the MAC-valid copy is used");
            } else {
                e = e.with_detail("one page-code copy is unreadable; the MAC-valid copy is used");
            }
            exceptions.push(e);
            CodeOutcome::OneCopyValid
        } else {
            CodeOutcome::Verified
        }
    } else if copies.iter().any(|c| c.status != CopyStatus::NotDecoded) {
        let why: Vec<&str> = copies
            .iter()
            .filter_map(|c| match c.status {
                CopyStatus::Malformed => Some("malformed payload"),
                CopyStatus::UnknownExam => Some("unknown exam"),
                CopyStatus::MacInvalid => Some("tag does not verify"),
                _ => None,
            })
            .collect();
        exceptions.push(pe("IDN-002", Severity::Blocker).with_detail(format!(
            "no MAC-valid page-code copy ({}): foreign, forged or old sheet; never auto-graded",
            why.join(", ")
        )));
        CodeOutcome::Invalid
    } else {
        exceptions.push(
            pe("IDN-001", Severity::Error)
                .with_sub_reason("unresolved")
                .with_detail(
                    "no page-code copy decoded (echo-line OCR not available): attach the page manually",
                ),
        );
        CodeOutcome::NotDecoded
    };
    PageIdentity {
        outcome,
        code: if outcome == CodeOutcome::CopiesDisagree {
            None
        } else {
            code
        },
        copies,
        exceptions,
    }
}

/// Reads every distinct code slot of `template` on the registered page.
pub(crate) fn read_page(
    img: &GrayImage,
    h: &Homography,
    template: &Template,
    mirrored: bool,
    cfg: &CodeConfig,
    reg: &CodeRegistry,
    page: Option<&str>,
) -> PageIdentity {
    let reads = slots(template)
        .into_iter()
        .map(|(copy, rect)| {
            let (p, a) = read_copy(img, h, rect, mirrored, cfg);
            (copy, p, a)
        })
        .collect();
    identify(reads, reg, page)
}

/// The geometry of the page a verified code names.
pub(crate) fn page_of<'t>(template: &'t Template, code: &PageCode) -> Option<&'t PageGeometry> {
    template.page(code.page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use omr_forms::TemplateSpec;
    use omr_model::{BookletSerial, CancelMode, ExamId, FormCode, PageNo};

    fn setup() -> (Template, ExamKey, CodeRegistry) {
        let t = Template::build(TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(7)),
            20,
            4,
            CancelMode::EraseOnly,
        ))
        .unwrap();
        let k = ExamKey::from_bytes([9; 32]);
        let reg = CodeRegistry::for_template(&t, k.clone());
        (t, k, reg)
    }

    fn payload(t: &Template, k: &ExamKey, serial: u32) -> String {
        t.page_code(FormCode(1), PageNo(1), BookletSerial(serial))
            .encode(k)
            .unwrap()
    }

    #[test]
    fn req_a7_copy_outcomes() {
        let (t, k, reg) = setup();
        let good = payload(&t, &k, 5);
        let other = payload(&t, &k, 6);
        let forged = payload(&t, &ExamKey::from_bytes([1; 32]), 5);
        let id = |a: Option<&str>, b: Option<&str>| {
            identify(
                vec![(0, a.map(str::to_owned), 1), (1, b.map(str::to_owned), 1)],
                &reg,
                Some("p1"),
            )
        };
        let r = id(Some(&good), Some(&good));
        assert_eq!(r.outcome, CodeOutcome::Verified);
        assert!(r.exceptions.is_empty() && r.verified());
        let r = id(Some(&good), Some(&forged));
        assert_eq!(r.outcome, CodeOutcome::OneCopyValid);
        assert_eq!(r.exceptions[0].code, "IDN-001");
        assert_eq!(r.exceptions[0].severity, Severity::Info);
        assert_eq!(
            r.exceptions[0].sub_reason.as_deref(),
            Some("one_copy_invalid")
        );
        let r = id(None, Some(&good));
        assert_eq!(r.outcome, CodeOutcome::OneCopyValid);
        assert_eq!(r.exceptions[0].sub_reason, None);
        let r = id(Some(&good), Some(&other));
        assert_eq!(r.outcome, CodeOutcome::CopiesDisagree);
        assert_eq!(r.exceptions[0].code, "COL-004");
        assert!(!r.verified());
        let r = id(Some(&forged), Some(&forged));
        assert_eq!(r.outcome, CodeOutcome::Invalid);
        assert_eq!(r.exceptions[0].code, "IDN-002");
        assert_eq!(r.exceptions[0].severity, Severity::Blocker);
        let r = id(Some("HELLO"), None);
        assert_eq!(r.outcome, CodeOutcome::Invalid);
        let r = id(None, None);
        assert_eq!(r.outcome, CodeOutcome::NotDecoded);
        assert_eq!(r.exceptions[0].severity, Severity::Error);
        // Unknown exam: empty registry.
        let r = identify(vec![(0, Some(good.clone()), 1)], &CodeRegistry::new(), None);
        assert_eq!(r.outcome, CodeOutcome::Invalid);
        assert_eq!(r.copies[0].status, CopyStatus::UnknownExam);
        // Valid MAC, unknown revision.
        let mut reg2 = CodeRegistry::new();
        reg2.register(t.exam_id40(), k.clone(), vec![TemplateRevision(9)]);
        let r = identify(vec![(0, Some(good.clone()), 1)], &reg2, None);
        assert_eq!(r.outcome, CodeOutcome::UnknownRevision);
        assert_eq!(r.exceptions[0].code, "IDN-002");
        assert!(format!("{reg2:?}").contains("redacted"));
    }
}
