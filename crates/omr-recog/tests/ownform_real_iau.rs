//! Real data (restricted: read from `OMR_IAU_DIR`, never copied; aggregate counts only): the
//! owner's IAU CCSIT sheets are legacy forms with no AprilTags and no page code, so the
//! own-form path must reject every one of them with REG-001 (the foreign/legacy-form route,
//! §8.3) instead of mis-registering a lattice of bubbles. Skips when `OMR_IAU_DIR` is unset.

use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId, Severity};
use omr_recog::ownform::{CodeRegistry, OwnFormConfig, OwnFormEngine, OwnPageRequest, PageRoute};
use std::path::PathBuf;
use std::time::Instant;

#[test]
fn ownform_path_rejects_legacy_iau_sheets() {
    let Ok(dir) = std::env::var("OMR_IAU_DIR") else {
        eprintln!("OMR_IAU_DIR is not set: skipping the real-data check");
        return;
    };
    let t = Template::build(TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(1)),
        100,
        5,
        CancelMode::EraseOnly,
    ))
    .expect("template");
    let reg = CodeRegistry::for_template(&t, ExamKey::from_bytes([1; 32]));
    let e = OwnFormEngine::new(t, OwnFormConfig::gray_v1());
    let mut files: Vec<PathBuf> = std::fs::read_dir(PathBuf::from(dir).join("pages"))
        .expect("pages directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    files.sort();
    let (mut rejected, mut ms) = (0usize, Vec::new());
    for f in &files {
        let bytes = std::fs::read(f).expect("read page");
        let page = omr_imaging::decode(&bytes).expect("decode");
        let t0 = Instant::now();
        let m = e.measure_page(&page.image, &OwnPageRequest::default(), &reg);
        ms.push(t0.elapsed().as_secs_f64() * 1e3);
        let r = e.decide(&m, &e.batch_baseline(&[]));
        if m.route == PageRoute::Unidentified
            && m.registration.is_none()
            && r.questions.is_empty()
            && r.exceptions
                .iter()
                .any(|x| x.code == "REG-001" && x.severity == Severity::Blocker)
        {
            rejected += 1;
        }
    }
    ms.sort_by(f64::total_cmp);
    eprintln!(
        "own-form path on {} legacy pages: {rejected} rejected with REG-001; median {:.1} ms",
        files.len(),
        ms.get(ms.len() / 2).copied().unwrap_or(0.0)
    );
    assert!(!files.is_empty());
    assert_eq!(rejected, files.len());
}
