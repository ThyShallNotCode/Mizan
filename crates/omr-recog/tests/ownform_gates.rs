//! Own-form safety gates that keep a decision out of auto-acceptance (§8.4 tag-count rule,
//! §8.5 bitonal slice, §8.8 ID grids / the `IdColumnRead` contract). Synthetic sheets only.

#[path = "support/ownform_eval.rs"]
mod ownform_eval;

use omr_imaging::CaptureKind;
use omr_model::{ColumnState, LogitMilli, PageNo, Response, Severity};
use omr_recog::ownform::{
    CodeRegistry, OwnFormConfig, OwnFormEngine, OwnPageMeasurement, OwnPageRequest, OwnPageResult,
};
use omr_sim::{CorpusConfig, MarkRates, Profile, Simulator};
use ownform_eval::{capture_sheet, engine, key, template};
use std::sync::OnceLock;

const DPI: u32 = 200;

fn setup() -> &'static (omr_forms::Template, OwnFormEngine) {
    static S: OnceLock<(omr_forms::Template, OwnFormEngine)> = OnceLock::new();
    S.get_or_init(|| {
        let t = template(100, 5, 2);
        let e = engine(&t, &key(), DPI, OwnFormConfig::gray_v1());
        (t, e)
    })
}

/// Sheet `i` of a small corpus, decoded.
fn page(profile: Profile, seed: u64, i: u32) -> (omr_imaging::GrayImage, omr_sim::PageTruth) {
    let (t, _) = setup();
    let k = key();
    let sim = Simulator::new(t, &k, DPI);
    let mut ccfg = CorpusConfig::standard(i + 1, seed);
    ccfg.profile = profile;
    ccfg.rates = match profile {
        Profile::Hard => MarkRates::hard(),
        _ => MarkRates::standard(),
    };
    let p = capture_sheet(&sim, t, &ccfg, i).remove(0);
    let img = omr_imaging::decode(&p.jpeg).expect("decode").image;
    (img, p.truth)
}

fn measure(img: &omr_imaging::GrayImage, reg: &CodeRegistry) -> OwnPageMeasurement {
    let (_, e) = setup();
    let req = OwnPageRequest {
        device_dpi: Some(DPI),
        ..OwnPageRequest::default()
    };
    e.measure_page(img, &req, reg)
}

fn decide(m: &OwnPageMeasurement) -> OwnPageResult {
    let (_, e) = setup();
    e.decide(m, &e.batch_baseline(&[]))
}

fn has(r: &OwnPageResult, code: &str, sev: Severity) -> bool {
    r.exceptions
        .iter()
        .any(|e| e.code == code && e.severity == sev)
}

/// A lone mark as dark as an erasure residue on a bitonal capture (0.30 above the baseline on
/// a solid-fill page) is reviewed: bitonal input uses the thresholds validated on real bitonal
/// sheets, not the gray ones (which accept it).
#[test]
fn req_a1_bitonal_capture_uses_bitonal_thresholds() {
    let (t, e) = setup();
    let reg = CodeRegistry::for_template(t, key());
    let (img, _) = page(Profile::Standard, 31_001, 0);
    let mut m = measure(&img, &reg);
    assert!(m.auto_accept_allowed, "{:?}", m.exceptions);
    let prior = e.prior(PageNo(1)).expect("prior").to_vec();
    // Question 0 (bubbles 0..5): only option 2 marked, δ = 310/1024 above β_p⁰.
    for (j, p) in prior.iter().enumerate().take(5) {
        m.bubbles[j].inner = omr_model::Darkness(p.0 + if j == 2 { 310 } else { 0 });
        m.features[j] = Default::default();
    }
    let gray = decide(&m);
    let q0 = |r: &OwnPageResult| r.questions.iter().find(|q| q.question == 0).cloned();
    let g = q0(&gray).expect("question 0");
    // At this page level δ = 310 clears the gray θ_sel = max(205, 0.30·level) + h but stays
    // below the bitonal θ_sel = max(205, 0.55·level) − 2h.
    assert!(
        (600..=1000).contains(&gray.level.0),
        "page level {}",
        gray.level.0
    );
    assert_eq!((g.response, g.auto_accepted), (Response::Single(2), true));

    m.request.capture = CaptureKind::Bitonal;
    let bitonal = decide(&m);
    let b = q0(&bitonal).expect("question 0");
    assert!(!b.auto_accepted, "{b:?}");
    assert!(b.exceptions.iter().any(|x| x.code == "MRK-001"), "{b:?}");
    assert!(
        bitonal
            .exceptions
            .iter()
            .any(|x| x.code == "CAP-003" && x.sub_reason.as_deref() == Some("bitonal"))
    );
}

/// §8.4: two tags need Stage B and Stage C; without them the two-tag affine extrapolates (no
/// perspective), so such a page is never auto-graded and its ID grid is never exact.
#[test]
fn req_a10_two_tag_pages_are_never_auto_graded() {
    let (t, _) = setup();
    let reg = CodeRegistry::for_template(t, key());
    let g = t.page(PageNo(1)).expect("page");
    for (i, drop) in [[2usize, 3], [0, 1]].into_iter().enumerate() {
        let (mut img, truth) = page(Profile::Hard, 31_002, i as u32);
        let h = truth.mm_to_px;
        for c in drop {
            let c0 = g.tags[c].centre_mm();
            let (u, v) = omr_sim::profile::apply_h(&h, c0.x, c0.y);
            let r = 5.5 * f64::from(DPI) / 25.4;
            for y in (v - r).max(0.0) as usize..((v + r) as usize).min(img.height()) {
                for x in (u - r).max(0.0) as usize..((u + r) as usize).min(img.width()) {
                    img.set(x, y, 245);
                }
            }
        }
        let m = measure(&img, &reg);
        let a = m.registration.as_ref().expect("two tags register");
        assert_eq!(a.tags_used.len(), 2, "{drop:?}");
        assert!(!m.auto_accept_allowed);
        let r = decide(&m);
        assert!(has(&r, "REG-003", Severity::Error), "{:?}", r.exceptions);
        assert!(r.questions.iter().all(|q| !q.auto_accepted));
        let id = r.student_id.as_ref().expect("ID grid");
        assert!(!id.confident() && id.read.exact_digits().is_none());
    }
}

/// An unverified page (no MAC-valid code: IDN-002) is measured provisionally, but no ID column
/// is reported as a confident digit (`ColumnState::Single`, `exact_digits`) and no per-digit
/// logit reaches +L_q: identity must not take a forged sheet's ID as exact.
#[test]
fn req_a6_unverified_page_id_is_never_exact() {
    let (t, _) = setup();
    let (img, truth) = page(Profile::Standard, 31_003, 0);
    // Verified: the ID reads exactly.
    let ok = decide(&measure(&img, &CodeRegistry::for_template(t, key())));
    let id = ok.student_id.as_ref().expect("ID grid");
    assert_eq!(id.read.exact_digits(), truth.student_id);
    // Unknown exam (empty registry): IDN-002, proposals only.
    let r = decide(&measure(&img, &CodeRegistry::new()));
    assert!(has(&r, "IDN-002", Severity::Blocker));
    assert!(r.questions.iter().all(|q| !q.auto_accepted));
    let id = r.student_id.as_ref().expect("ID grid (provisional)");
    assert_eq!(id.read.exact_digits(), None);
    assert!(id.review.iter().all(|&x| x));
    for c in &id.read.columns {
        assert_ne!(c.state, ColumnState::Single, "{c:?}");
        assert!(
            c.logits.iter().all(|l| l.0 < LogitMilli::STANDARD_LQ.0),
            "{c:?}"
        );
    }
    // The proposed digits are still there for the reviewer.
    let proposed: String = id
        .read
        .columns
        .iter()
        .map(|c| match c.marked.as_slice() {
            [d] => char::from(b'0' + d),
            _ => '?',
        })
        .collect();
    assert_eq!(Some(proposed), truth.student_id);
    if let Some(f) = &r.form {
        assert!(!f.confident && f.value().is_none());
    }
}
