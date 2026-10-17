//! Review findings turned into regression tests (synthetic sheets only, no real data): a
//! rescanned sheet (same verified serial, different bytes), forms declared in another order
//! than printed, a sheet of an older template revision, a blank booklet, and review-state and
//! key-editing invariants.

use omr_app::model::PageStatus;
use omr_app::{
    Action, Decision, ExamDefinition, ExamProject, FormKey, GradeOptions, IdSchemeChoice, ItemKind,
    KeySource, RosterStudent, TemplateChoice,
};
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId, TemplateRevision};
use omr_sim::{CorpusConfig, PageTruth};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const QUESTIONS: u16 = 20;
const OPTIONS: u8 = 4;
const KEY: [u8; 32] = [0x3c; 32];

fn spec(revision: u16) -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_1234_5678_7abc_8def_0000_0000_0042,
        )),
        QUESTIONS,
        OPTIONS,
        CancelMode::EraseOnly,
    );
    s.forms = 2;
    s.revision = TemplateRevision(revision);
    s.title = "Robustness quiz".into();
    s
}

fn roster_ids() -> Vec<String> {
    (0..12)
        .map(|i| format!("4211{:05}", 300 + i * 11))
        .collect()
}

fn key_letters(form: usize) -> Vec<String> {
    (0..QUESTIONS)
        .map(|q| {
            char::from(b'A' + ((q as usize * 3 + form) % usize::from(OPTIONS)) as u8).to_string()
        })
        .collect()
}

/// A corpus of `sheets` own-form sheets of template revision `revision` (all blank when
/// `blank`).
fn corpus(
    dir: &Path,
    sheets: u32,
    seed: u64,
    revision: u16,
    blank: bool,
) -> (PathBuf, BTreeMap<u32, PageTruth>) {
    let t = Template::build(spec(revision)).unwrap();
    let mut cfg = CorpusConfig::standard(sheets, seed);
    cfg.roster = roster_ids();
    cfg.threads = 2;
    if blank {
        cfg.rates.blank = 1.0;
        cfg.rates.multi = 0.0;
        cfg.rates.erasure = 0.0;
        cfg.rates.light = 0.0;
        cfg.rates.dot = 0.0;
        cfg.rates.check_or_x = 0.0;
        cfg.rates.crossed_out = 0.0;
        cfg.rates.stray_per_sheet = 0.0;
    }
    omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(KEY), &cfg, dir).unwrap();
    let truth = omr_sim::corpus::read_truth(&dir.join("truth.jsonl"))
        .unwrap()
        .into_iter()
        .map(|p| (p.booklet, p))
        .collect();
    (dir.join("images"), truth)
}

/// The exam definition, with the forms declared in `forms` order (keys follow the letters).
fn definition(forms: &[&str]) -> ExamDefinition {
    let mut d = ExamDefinition::new(
        "Robustness",
        TemplateChoice::Generated(Box::new(spec(1))),
        QUESTIONS,
        OPTIONS,
        forms.iter().map(|f| (*f).to_string()).collect(),
        CancelMode::EraseOnly,
    );
    d.page_code_key_hex = Some(hex::encode(KEY));
    d.key = KeySource::Provided {
        forms: forms
            .iter()
            .map(|f| FormKey {
                form: (*f).into(),
                answers: key_letters(usize::from(f.as_bytes()[0] - b'A')),
            })
            .collect(),
    };
    d.roster = Some(
        roster_ids()
            .into_iter()
            .enumerate()
            .map(|(i, id)| RosterStudent {
                id,
                name: format!("Student {i}"),
                name_ar: String::new(),
                section: "101".into(),
                numeric_cell: false,
            })
            .collect(),
    );
    d.id_scheme = IdSchemeChoice::Digits(9);
    d.exam_id = d.derived_exam_id();
    d.validate().unwrap();
    d
}

fn grade(def: &ExamDefinition, inputs: &[PathBuf], out: &Path) -> ExamProject {
    let opts = GradeOptions {
        threads: 2,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-17T09:00:00Z".into()),
        operator: "principal:test".into(),
        max_pages: None,
    };
    let mut p = ExamProject::grade(def, inputs, &opts, &|_| {}).unwrap();
    p.export(out).unwrap();
    p
}

fn files_of(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

#[test]
fn forms_declared_in_another_order_map_by_the_printed_letter() {
    let dir = tempfile::tempdir().unwrap();
    let (images, truth) = corpus(&dir.path().join("corpus"), 6, 11, 1, false);
    // The key file listed form B first: the definition's order is B, A.
    let def = definition(&["B", "A"]);
    let p = grade(&def, &[images], &dir.path().join("out"));
    let s = &p.session;
    assert_eq!(s.booklets.len(), 6);
    for b in &s.booklets {
        let printed = char::from(b'A' + (truth[&b.serial].form - 1) as u8).to_string();
        let f = b.form.expect("form from the verified page code");
        assert_eq!(s.definition.forms[f], printed, "booklet {}", b.serial);
    }
}

#[test]
fn a_rescanned_sheet_with_the_same_serial_is_col004_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (images, _) = corpus(&dir.path().join("corpus"), 3, 12, 1, false);
    let files = files_of(&images);
    // Re-encode one scan: same pixels (same verified serial), different bytes.
    let extra = dir.path().join("rescan");
    std::fs::create_dir_all(&extra).unwrap();
    let img = image::open(&files[0]).unwrap();
    img.save(extra.join("rescan.png")).unwrap();
    let def = definition(&["A", "B"]);
    let p = grade(&def, &[images, extra], &dir.path().join("out"));
    let s = &p.session;
    // One booklet per serial; the second copy is held out with COL-004 (never overwritten,
    // never a second booklet with the same serial).
    let mut serials: Vec<u32> = s.booklets.iter().map(|b| b.serial).collect();
    serials.sort_unstable();
    serials.dedup();
    assert_eq!(serials.len(), s.booklets.len());
    assert_eq!(s.booklets.len(), 3);
    assert!(
        p.queue(None).iter().any(|i| i.code == "COL-004"),
        "COL-004 queued"
    );
    assert!(s.release_blocked().is_some());
    let held = s
        .pages
        .iter()
        .filter(|pg| pg.status == PageStatus::Unattributed)
        .count();
    assert_eq!(held, 1);
}

#[test]
fn a_sheet_of_another_template_revision_is_queued_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (images, _) = corpus(&dir.path().join("corpus"), 2, 13, 1, false);
    let (old, _) = corpus(&dir.path().join("old"), 1, 14, 2, false);
    let def = definition(&["A", "B"]);
    let p = grade(&def, &[images, old], &dir.path().join("out"));
    let s = &p.session;
    assert_eq!(s.pages.len(), 3);
    for (pi, pg) in s.pages.iter().enumerate() {
        if pg.status == PageStatus::Read {
            continue;
        }
        assert!(
            p.session
                .review
                .items
                .iter()
                .any(|i| i.page == Some(pi) && i.severity >= omr_model::Severity::Error),
            "page {pi} ({:?}) has no blocking review item",
            pg.status
        );
    }
    assert!(s.release_blocked().is_some());
}

#[test]
fn a_blank_booklet_is_never_scored_zero_before_review() {
    let dir = tempfile::tempdir().unwrap();
    let (images, _) = corpus(&dir.path().join("corpus"), 2, 15, 1, true);
    let def = definition(&["A", "B"]);
    let mut p = grade(&def, &[images], &dir.path().join("out"));
    let blank: Vec<u64> = p
        .queue(None)
        .iter()
        .filter(|i| i.code == "MRK-015")
        .map(|i| i.id)
        .collect();
    assert!(!blank.is_empty(), "MRK-015 queued");
    let status = |p: &ExamProject, bi: usize| {
        let sc = p.scoring.as_ref().unwrap();
        let row = sc.row_of[bi].unwrap();
        sc.run.as_ref().unwrap().students[row].status
    };
    let bi = p.session.review.item(blank[0]).unwrap().booklet.unwrap();
    assert_eq!(
        status(&p, bi),
        omr_scoring::AssessmentStatus::Provisional,
        "an open MRK-015 holds the booklet's score"
    );
    p.apply(
        blank[0],
        Action::Resolve {
            decision: Decision::Acknowledge,
            accepted_suggestion: true,
            note: String::new(),
        },
    )
    .unwrap();
    assert_eq!(status(&p, bi), omr_scoring::AssessmentStatus::Scored);
    p.undo().unwrap();
    assert_eq!(status(&p, bi), omr_scoring::AssessmentStatus::Provisional);
}

#[test]
fn review_state_and_key_editing_invariants() {
    let dir = tempfile::tempdir().unwrap();
    let (images, _) = corpus(&dir.path().join("corpus"), 4, 16, 1, false);
    // Identical keys on both forms: shared master items.
    let mut def = definition(&["A", "B"]);
    def.key = KeySource::Provided {
        forms: ["A", "B"]
            .iter()
            .map(|f| FormKey {
                form: (*f).into(),
                answers: key_letters(0),
            })
            .collect(),
    };
    def.validate().unwrap();
    let mut p = grade(&def, &[images], &dir.path().join("out"));
    assert!(p.session.key.as_ref().unwrap().shared_items);
    // Key edits use 1-based question numbers (like the mapping and the shell) and keep every
    // form's copy of a shared key in step.
    p.set_key(1, 1, "D").unwrap();
    let k = p.session.key.as_ref().unwrap();
    assert_eq!(k.forms[0][0], omr_app::model::KeyCell::Options(vec![3]));
    assert_eq!(k.forms[1][0], omr_app::model::KeyCell::Options(vec![3]));
    assert!(p.set_key(0, 0, "A").is_err());
    assert!(p.set_key(0, u32::from(QUESTIONS) + 1, "A").is_err());
    // A resolved item stays resolved: defer and rescan are refused (only undo re-opens it).
    // Blank booklets always leave an MRK-015 notice to resolve.
    let (blank, _) = corpus(&dir.path().join("blank"), 1, 17, 1, true);
    let mut p = grade(&def, &[blank], &dir.path().join("out-blank"));
    let first = p
        .queue(None)
        .first()
        .map(|i| (i.id, i.kind))
        .expect("an MRK-015 notice");
    assert_eq!(first.1, ItemKind::Notice);
    p.apply(
        first.0,
        Action::Resolve {
            decision: Decision::Acknowledge,
            accepted_suggestion: false,
            note: String::new(),
        },
    )
    .unwrap();
    assert!(p.apply(first.0, Action::Defer).is_err());
    assert!(p.apply(first.0, Action::AddToRescan).is_err());
    assert!(!p.session.review.item(first.0).unwrap().is_open());
    assert_eq!(p.undo().unwrap(), Some(first.0));
    assert!(p.session.review.item(first.0).unwrap().is_open());
    p.apply(first.0, Action::Defer).unwrap();
}
