//! Own-form page identification (§6.4, §8.3): orientation and mirroring from the tag IDs,
//! the page-code copy outcomes (IDN-001, IDN-002, COL-004) on rendered sheets, tag-set
//! routing, other exams, blank and cut-off pages. Synthetic sheets only.

use omr_forms::render::{px_per_mm, rasterize_sheet};
use omr_forms::{ExamKey, SheetInstance, Template, TemplateSpec};
use omr_imaging::GrayImage;
use omr_model::{BookletSerial, CancelMode, ExamId, FormCode, PageNo, Severity, TemplateRevision};
use omr_recog::ownform::{
    CodeOutcome, CodeRegistry, OwnFormConfig, OwnFormEngine, OwnPageMeasurement, OwnPageRequest,
    OwnPageResult, PageRoute,
};
use omr_sim::{CaptureParams, MarkRates, Profile, SheetScript, Simulator};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::sync::OnceLock;

const DPI: u32 = 200;

fn template_with(exam: u128, questions: u16) -> Template {
    let s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(exam)),
        questions,
        5,
        CancelMode::EraseOnly,
    );
    Template::build(s).expect("template")
}

fn template() -> &'static Template {
    static T: OnceLock<Template> = OnceLock::new();
    T.get_or_init(|| template_with(0x0190_0000_0000_7000_8000_0000_0000_0a07, 40))
}

fn key() -> ExamKey {
    ExamKey::from_bytes([0x33; 32])
}

fn engine() -> &'static OwnFormEngine {
    static E: OnceLock<OwnFormEngine> = OnceLock::new();
    E.get_or_init(|| OwnFormEngine::new(template().clone(), OwnFormConfig::gray_v1()))
}

fn registry() -> CodeRegistry {
    CodeRegistry::for_template(template(), key())
}

fn to_gray(img: &image::GrayImage) -> GrayImage {
    GrayImage::from_raw(
        img.width() as usize,
        img.height() as usize,
        img.as_raw().clone(),
    )
    .expect("dimensions")
}

/// A marked, lightly captured page (clean profile) and its script.
fn marked(serial: u32, seed: u64) -> (GrayImage, SheetScript) {
    let t = template();
    let k = key();
    let sim = Simulator::new(t, &k, DPI);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let script = SheetScript::random(
        t,
        1,
        serial,
        Some("221234567".into()),
        &MarkRates::clean(),
        &mut rng,
    );
    let mut img = sim
        .clean_page(FormCode(1), PageNo(1), BookletSerial(serial))
        .expect("render");
    sim.paint_page(&mut img, &script, PageNo(1), &mut rng)
        .expect("paint");
    let params = CaptureParams::sample(Profile::Clean, DPI, &mut rng);
    let cap = omr_sim::capture::capture(&img, &params, &mut rng);
    (to_gray(&cap.image), script)
}

fn run(img: &GrayImage, reg: &CodeRegistry) -> (OwnPageMeasurement, OwnPageResult) {
    let req = OwnPageRequest {
        device_dpi: Some(DPI),
        ..OwnPageRequest::default()
    };
    let e = engine();
    let m = e.measure_page(img, &req, reg);
    let r = e.decide(&m, &e.batch_baseline(&[]));
    (m, r)
}

fn codes(r: &OwnPageResult) -> Vec<(String, Severity, Option<String>)> {
    r.exceptions
        .iter()
        .map(|e| (e.code.clone(), e.severity, e.sub_reason.clone()))
        .collect()
}

fn has(r: &OwnPageResult, code: &str, sev: Severity) -> bool {
    r.exceptions
        .iter()
        .any(|e| e.code == code && e.severity == sev)
}

/// Every auto-accepted answer equals the script's intended response; returns how many.
fn check_answers(r: &OwnPageResult, s: &SheetScript) -> usize {
    let mut n = 0;
    for q in &r.questions {
        if q.auto_accepted {
            assert_eq!(
                q.response,
                s.questions[usize::from(q.question)].intended,
                "question {}",
                q.question
            );
            n += 1;
        }
    }
    n
}

fn rot90(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    // Clockwise: the output pixel (x', y') shows the input pixel (y', h − 1 − x').
    GrayImage::from_fn(h, w, |x, y| img.get(y, h - 1 - x))
}

fn rot180(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    GrayImage::from_fn(w, h, |x, y| img.get(w - 1 - x, h - 1 - y))
}

fn mirror(img: &GrayImage) -> GrayImage {
    let w = img.width();
    GrayImage::from_fn(w, img.height(), |x, y| img.get(w - 1 - x, y))
}

#[test]
fn upright_page_is_identified_and_read() {
    let (img, s) = marked(11, 1);
    let (m, r) = run(&img, &registry());
    assert_eq!(m.route, PageRoute::ThisTemplate);
    assert_eq!(m.identity.as_ref().unwrap().outcome, CodeOutcome::Verified);
    assert_eq!(r.code.unwrap().serial, BookletSerial(11));
    assert_eq!(r.page, Some(PageNo(1)));
    let a = m.registration.as_ref().unwrap();
    assert_eq!(a.tags_used.len(), 4);
    assert!(a.rms_um < 50.0, "{a:?}");
    assert!((a.dpi_eff() - 200.0).abs() < 2.0);
    assert!(r.exceptions.is_empty(), "{:?}", codes(&r));
    assert_eq!(check_answers(&r, &s), 40);
    let id = r.student_id.unwrap();
    assert_eq!(id.text(), "221234567");
}

#[test]
fn req_a7_rotated_pages_are_auto_rotated() {
    let (img, s) = marked(12, 2);
    let mut img90 = rot90(&img);
    for turns in 1..=3u8 {
        let (m, r) = run(&img90, &registry());
        let a = m.registration.as_ref().expect("registered");
        assert_eq!(a.quarter_turns, turns, "{turns}");
        assert!(!a.mirrored);
        assert!(has(&r, "REG-002", Severity::Info), "{:?}", codes(&r));
        assert_eq!(m.identity.as_ref().unwrap().outcome, CodeOutcome::Verified);
        assert_eq!(check_answers(&r, &s), 40, "{turns} quarter turns");
        img90 = rot90(&img90);
    }
    // 180° directly.
    let (m, r) = run(&rot180(&img), &registry());
    assert_eq!(m.registration.as_ref().unwrap().quarter_turns, 2);
    assert_eq!(check_answers(&r, &s), 40);
}

#[test]
fn req_a7_mirrored_page_is_unmirrored() {
    let (img, s) = marked(13, 3);
    for flipped in [mirror(&img), rot90(&mirror(&img))] {
        let (m, r) = run(&flipped, &registry());
        let a = m.registration.as_ref().expect("registered");
        assert!(a.mirrored);
        assert!(m.tags.mirrored);
        assert!(has(&r, "REG-005", Severity::Warn), "{:?}", codes(&r));
        assert_eq!(m.identity.as_ref().unwrap().outcome, CodeOutcome::Verified);
        assert_eq!(check_answers(&r, &s), 40);
    }
}

/// Pixel rectangle of a code slot (plus `margin_mm`) at the render resolution.
fn slot_px(copy: usize, margin_mm: f64) -> (u32, u32, u32, u32) {
    let k = px_per_mm(DPI as f32);
    let rect = template().page(PageNo(1)).unwrap().codes[copy].rect;
    let x0 = ((f64::from(rect.x) / 1000.0 - margin_mm) * k).floor() as u32;
    let y0 = ((f64::from(rect.y) / 1000.0 - margin_mm) * k).floor() as u32;
    let x1 = ((f64::from(rect.right()) / 1000.0 + margin_mm) * k).ceil() as u32;
    let y1 = ((f64::from(rect.bottom()) / 1000.0 + margin_mm) * k).ceil() as u32;
    (x0, y0, x1, y1)
}

fn render(t: &Template, k: &ExamKey, serial: u32) -> image::GrayImage {
    let inst = SheetInstance::new(t, k, FormCode(1), PageNo(1)).with_serial(BookletSerial(serial));
    rasterize_sheet(&inst, DPI as f32).expect("render")
}

/// Copies slot `copy` from `src` into `dst`, or whitens it when `src` is `None`.
fn patch(dst: &mut image::GrayImage, src: Option<&image::GrayImage>, copy: usize) {
    let (x0, y0, x1, y1) = slot_px(copy, 1.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let v = src.map_or(255, |s| s.get_pixel(x, y).0[0]);
            dst.put_pixel(x, y, image::Luma([v]));
        }
    }
}

#[test]
fn req_a7_page_code_copy_outcomes_on_rendered_sheets() {
    let t = template();
    let k = key();
    let good = render(t, &k, 21);
    let reg = registry();

    // Both copies fine.
    let (m, r) = run(&to_gray(&good), &reg);
    assert_eq!(m.identity.as_ref().unwrap().outcome, CodeOutcome::Verified);
    assert!(m.auto_accept_allowed && m.exceptions.is_empty());
    // An unmarked sheet is a blank booklet (MRK-015), never auto-scored 0.
    assert!(has(&r, "MRK-015", Severity::Error));

    // One copy unreadable: the other is used, IDN-001 INFO, still auto-gradable.
    let mut img = good.clone();
    patch(&mut img, None, 1);
    let (m, r) = run(&to_gray(&img), &reg);
    assert_eq!(
        m.identity.as_ref().unwrap().outcome,
        CodeOutcome::OneCopyValid
    );
    assert!(has(&r, "IDN-001", Severity::Info));
    assert!(m.auto_accept_allowed);
    assert_eq!(m.route, PageRoute::ThisTemplate);

    // One copy forged (another key): IDN-001 one_copy_invalid (INFO).
    let forged = render(t, &ExamKey::from_bytes([0x44; 32]), 21);
    let mut img = good.clone();
    patch(&mut img, Some(&forged), 0);
    let (m, r) = run(&to_gray(&img), &reg);
    assert_eq!(
        m.identity.as_ref().unwrap().outcome,
        CodeOutcome::OneCopyValid
    );
    assert!(
        r.exceptions
            .iter()
            .any(|e| e.code == "IDN-001" && e.sub_reason.as_deref() == Some("one_copy_invalid")),
        "{:?}",
        codes(&r)
    );

    // Copies from two booklets (photocopy paste-up): COL-004, not auto-gradable.
    let other = render(t, &k, 22);
    let mut img = good.clone();
    patch(&mut img, Some(&other), 1);
    let (m, r) = run(&to_gray(&img), &reg);
    assert_eq!(
        m.identity.as_ref().unwrap().outcome,
        CodeOutcome::CopiesDisagree
    );
    assert!(
        r.exceptions.iter().any(|e| e.code == "COL-004"
            && e.sub_reason.as_deref() == Some("code_copies_disagree")),
        "{:?}",
        codes(&r)
    );
    assert!(!m.auto_accept_allowed);
    assert!(r.questions.iter().all(|q| !q.auto_accepted));

    // Forged sheet (both copies under another key): IDN-002 BLOCKER, never auto-graded, but
    // the single-page template is measured provisionally.
    let (m, r) = run(&to_gray(&forged), &reg);
    assert_eq!(m.identity.as_ref().unwrap().outcome, CodeOutcome::Invalid);
    assert!(has(&r, "IDN-002", Severity::Blocker));
    assert_eq!(m.route, PageRoute::Unidentified);
    assert_eq!(r.questions.len(), 40);
    assert!(r.questions.iter().all(|q| !q.auto_accepted));

    // Unknown exam (empty registry) and unknown revision: IDN-002.
    let (m, r) = run(&to_gray(&good), &CodeRegistry::new());
    assert_eq!(m.identity.as_ref().unwrap().outcome, CodeOutcome::Invalid);
    assert!(has(&r, "IDN-002", Severity::Blocker));
    let mut old = CodeRegistry::new();
    old.register(t.exam_id40(), k.clone(), vec![TemplateRevision(7)]);
    let (m, r) = run(&to_gray(&good), &old);
    assert_eq!(
        m.identity.as_ref().unwrap().outcome,
        CodeOutcome::UnknownRevision
    );
    assert!(has(&r, "IDN-002", Severity::Blocker));

    // Nothing decodes: IDN-001 `unresolved` (ERROR).
    let mut img = good.clone();
    patch(&mut img, None, 0);
    patch(&mut img, None, 1);
    let (m, r) = run(&to_gray(&img), &reg);
    assert_eq!(
        m.identity.as_ref().unwrap().outcome,
        CodeOutcome::NotDecoded
    );
    assert!(
        r.exceptions
            .iter()
            .any(|e| e.code == "IDN-001" && e.sub_reason.as_deref() == Some("unresolved")),
        "{:?}",
        codes(&r)
    );
    assert!(r.questions.iter().all(|q| !q.auto_accepted));
}

#[test]
fn other_exams_and_tag_sets_are_routed() {
    let k = key();
    // A sheet of another registered exam goes to that exam's engine.
    let other_t = template_with(0x0190_0000_0000_7000_8000_0000_0000_0b08, 30);
    let mut reg = registry();
    reg.register(other_t.exam_id40(), k.clone(), vec![TemplateRevision(1)]);
    let (m, r) = run(&to_gray(&render(&other_t, &k, 5)), &reg);
    assert_eq!(
        m.route,
        PageRoute::OtherTemplate {
            exam_id40: other_t.exam_id40(),
            revision: 1
        }
    );
    assert!(r.questions.is_empty() && r.exceptions.is_empty());

    // Proof sheets carry tag set 3: never graded as answer pages.
    let mut inst = SheetInstance::new(template(), &k, FormCode(1), PageNo(1));
    inst.proof = true;
    let proof = rasterize_sheet(&inst, DPI as f32).expect("render");
    let (m, r) = run(&to_gray(&proof), &registry());
    assert_eq!(m.route, PageRoute::OtherTagSet(3));
    assert!(has(&r, "IDN-002", Severity::Blocker));
}

#[test]
fn req_a10_blank_and_cut_off_pages() {
    // A blank page: no tags, REG-001 BLOCKER, nothing decided.
    let blank = GrayImage::new(1700, 2400, 245);
    let (m, r) = run(&blank, &registry());
    assert_eq!(m.route, PageRoute::Unidentified);
    assert!(has(&r, "REG-001", Severity::Blocker), "{:?}", codes(&r));
    assert!(r.questions.is_empty());

    // A scan cut off half way down: the two top tags still register the page, but two tags are
    // never auto-graded (§8.4 tag-count rule; REG-003 ERROR, sheet review); bubbles beyond the
    // image edge are UNREADABLE with CAP-009, never Blank; the rest are correct proposals.
    let (img, s) = marked(31, 7);
    let g = template().page(PageNo(1)).unwrap();
    let mut ys: Vec<f64> = g.bubbles.iter().map(|b| b.centre_mm().y).collect();
    ys.sort_by(f64::total_cmp);
    let cut_mm = ys[ys.len() / 2] + 2.0;
    let cut_px = (cut_mm * px_per_mm(DPI as f32)) as usize;
    let cropped = img.crop(0, 0, img.width(), cut_px, 255);
    let (m, r) = run(&cropped, &registry());
    let a = m.registration.as_ref().expect("two tags register");
    assert_eq!(a.tags_used.len(), 2);
    assert!(has(&r, "REG-003", Severity::Error), "{:?}", codes(&r));
    assert!(!m.auto_accept_allowed);
    assert!(r.questions.iter().all(|q| !q.auto_accepted));
    let unread: Vec<_> = r
        .questions
        .iter()
        .filter(|q| q.response.is_unreadable())
        .collect();
    assert!(!unread.is_empty());
    for q in &unread {
        assert!(q.exceptions.iter().any(|e| e.code == "CAP-009"));
    }
    // Everything above the cut is proposed correctly for the reviewer.
    let proposed_ok = r
        .questions
        .iter()
        .filter(|q| q.response == s.questions[usize::from(q.question)].intended)
        .count();
    assert!(proposed_ok + unread.len() >= 36, "{:?}", codes(&r));
}
