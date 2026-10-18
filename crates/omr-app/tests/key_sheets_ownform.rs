//! Key sheets on the university's own generated sheets (§10.1, C7): sheets rendered and
//! "scanned" by `omr-sim` (synthetic marks, a synthetic key), a 9-digit student-number grid
//! read under the DIGITS9 scheme. A key sheet named as such is read by the same key-blind
//! recognizer, holds the run, and once confirmed scores the exam exactly as the key file
//! does; the in-stack rule is refused under a scheme that accepts the marker (all 0 is a valid
//! DIGITS9 number), so an unnamed key sheet is graded as a student, and adding it afterwards
//! with `ExamProject::add_key_sheets` is the audited way: its original is stored, the run held,
//! and the confirmation publishes a key version whose source is the key sheet.

use omr_app::model::PageStatus;
use omr_app::{
    ExamDefinition, ExamProject, FormKey, GradeOptions, IdSchemeChoice, KeySource, TemplateChoice,
};
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId, Response};
use omr_scoring::AssessmentStatus;
use std::path::{Path, PathBuf};

const QUESTIONS: u16 = 12;
const OPTIONS: u8 = 4;
const SECRET: [u8; 32] = [0x6b; 32];

fn spec() -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_4b45_5953_7ee0_8f00_0000_0000_0c07,
        )),
        QUESTIONS,
        OPTIONS,
        CancelMode::EraseOnly,
    );
    s.forms = 1;
    s.title = "Own-form key sheet".into();
    s
}

/// The synthetic key (0-based options).
fn key() -> Vec<u8> {
    (0..QUESTIONS)
        .map(|q| ((q * 3 + 1) % u16::from(OPTIONS)) as u8)
        .collect()
}

/// What student `k` marks: the key on the questions below `k + 4`, a distractor after.
fn answers(k: u16) -> Vec<u8> {
    key()
        .into_iter()
        .enumerate()
        .map(|(q, a)| {
            if (q as u16) < k + 4 {
                a
            } else {
                (a + 1) % OPTIONS
            }
        })
        .collect()
}

fn student_id(k: u16) -> String {
    format!("4212{:05}", 100 + k * 13)
}

/// One rendered sheet (serial `serial`, number `id`, `marks`) written to `dir/name`.
fn sheet(dir: &Path, name: &str, serial: u32, id: &str, marks: &[u8], seed: u64) -> PathBuf {
    let t = Template::build(spec()).unwrap();
    let secret = ExamKey::from_bytes(SECRET);
    let sim = omr_sim::Simulator::new(&t, &secret, 200);
    let mut rng = <rand_chacha::ChaCha8Rng as rand::SeedableRng>::seed_from_u64(seed);
    let responses: Vec<Response> = marks.iter().map(|&o| Response::Single(o)).collect();
    let script = omr_sim::SheetScript::from_responses(
        &t,
        1,
        serial,
        Some(id.to_string()),
        &responses,
        &omr_sim::MarkRates::clean(),
        &mut rng,
    );
    let page = sim
        .simulate(&script, omr_sim::Profile::Standard, &mut rng)
        .unwrap()
        .remove(0);
    std::fs::create_dir_all(dir).unwrap();
    let f = dir.join(name);
    page.image.save(&f).unwrap();
    f
}

fn definition(key: KeySource) -> ExamDefinition {
    let mut d = ExamDefinition::new(
        "Own-form quiz",
        TemplateChoice::Generated(Box::new(spec())),
        QUESTIONS,
        OPTIONS,
        vec!["A".into()],
        CancelMode::EraseOnly,
    );
    d.page_code_key_hex = Some(hex::encode(SECRET));
    d.key = key;
    d.id_scheme = IdSchemeChoice::Digits(9);
    d.exam_id = d.derived_exam_id();
    d.validate().unwrap();
    d
}

fn key_file() -> KeySource {
    KeySource::Provided {
        forms: vec![FormKey {
            form: "A".into(),
            answers: key()
                .into_iter()
                .map(|o| char::from(b'A' + o).to_string())
                .collect(),
        }],
    }
}

fn options(out: &Path) -> GradeOptions {
    GradeOptions {
        threads: 2,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-17T09:00:00Z".into()),
        operator: "principal:test".into(),
        max_pages: None,
    }
}

/// (student number, raw score) of every scored booklet, sorted.
fn raw_scores(p: &ExamProject) -> Vec<(String, String)> {
    let sc = p.scoring.as_ref().unwrap();
    let run = sc.run.as_ref().unwrap();
    let mut v: Vec<(String, String)> = p
        .session
        .booklets
        .iter()
        .enumerate()
        .filter_map(|(bi, b)| {
            let r = sc.row_of.get(bi).copied().flatten()?;
            let s = &run.students[r];
            assert_eq!(s.status, AssessmentStatus::Scored, "{}", b.label());
            Some((b.identity.student.clone()?, s.raw.to_string()))
        })
        .collect();
    v.sort();
    v
}

struct Stack {
    students: Vec<PathBuf>,
    key_sheet: PathBuf,
}

fn stack(dir: &Path) -> Stack {
    let students = (0..4u16)
        .map(|k| {
            sheet(
                &dir.join("scans"),
                &format!("s{k}.png"),
                10 + u32::from(k),
                &student_id(k),
                &answers(k),
                40 + u64::from(k),
            )
        })
        .collect();
    let key_sheet = sheet(&dir.join("keys"), "key-A.png", 99, "000000000", &key(), 7);
    Stack {
        students,
        key_sheet,
    }
}

#[test]
fn own_form_key_sheet_named_holds_then_scores_like_the_key_file() {
    let tmp = tempfile::tempdir().unwrap();
    let s = stack(tmp.path());
    // The key file: the reference scores.
    let by_file = ExamProject::grade(
        &definition(key_file()),
        &s.students,
        &options(&tmp.path().join("file")),
        &|_| {},
    )
    .unwrap();
    let want = raw_scores(&by_file);
    assert_eq!(want.len(), 4);
    for (k, (id, raw)) in want.iter().enumerate() {
        assert_eq!(*id, student_id(k as u16));
        assert_eq!(*raw, (k + 4).to_string(), "hand count");
    }
    by_file.close().unwrap();
    // The key sheet named: read, held, never graded as a student.
    let mut p = ExamProject::grade_with_key_sheets(
        &definition(KeySource::None),
        &s.students,
        std::slice::from_ref(&s.key_sheet),
        &options(&tmp.path().join("sheet")),
        &|_| {},
    )
    .unwrap();
    let ks = &p.session.key_sheets;
    assert_eq!(ks.sheets.len(), 1);
    assert!(
        ks.sheets[0].declared && ks.sheets[0].usable(),
        "{:?}",
        ks.sheets[0]
    );
    assert!(p.session.key_unconfirmed());
    assert_eq!(p.session.booklets.len(), 4);
    assert!(
        p.scoring.is_none(),
        "nothing scored before the confirmation"
    );
    let sum = p.key_sheet_summary(false).unwrap();
    assert_eq!(sum.versions[0].open, 0, "every row one confident mark");
    let r = p.confirm_key_sheets(None, Vec::new(), None).unwrap();
    assert!(!r.held);
    assert_eq!(raw_scores(&p), want, "the key sheet scores as the key file");
    p.close().unwrap();
}

#[test]
fn the_in_stack_rule_is_refused_under_digits_and_add_key_sheets_is_the_audited_way() {
    let tmp = tempfile::tempdir().unwrap();
    let s = stack(tmp.path());
    let mut all = s.students.clone();
    all.push(s.key_sheet.clone());
    let out = tmp.path().join("exam");
    let mut p =
        ExamProject::grade(&definition(KeySource::None), &all, &options(&out), &|_| {}).unwrap();
    // All 0 is a valid DIGITS9 number: the marker is not used in the stack (said so), and
    // the unnamed key sheet is graded as a student.
    let ks = &p.session.key_sheets;
    assert!(ks.sheets.is_empty());
    assert!(
        ks.notes.iter().any(|n| n.contains("not used in the stack")),
        "{:?}",
        ks.notes
    );
    assert_eq!(p.session.booklets.len(), 5);
    // Added afterwards as a key sheet (the desktop's "Read key from answer sheet…"): its
    // original is the stored one, the page is now a key sheet, the run is held.
    let r = p
        .add_key_sheets(std::slice::from_ref(&s.key_sheet), &options(&out), &|_| {})
        .unwrap();
    assert_eq!(r.duplicates, 1, "the file was already stored");
    assert_eq!(p.session.booklets.len(), 4, "no longer graded as a student");
    let ks = &p.session.key_sheets;
    assert_eq!(ks.sheets.len(), 1);
    assert!(ks.sheets[0].declared);
    assert_eq!(
        p.session
            .pages
            .iter()
            .filter(|pg| pg.status == PageStatus::KeySheet)
            .count(),
        1
    );
    assert!(p.session.key_unconfirmed());
    assert!(
        p.session
            .release_blocked()
            .unwrap()
            .contains("not confirmed")
    );
    let r = p.confirm_key_sheets(None, Vec::new(), None).unwrap();
    let v = r.key_version.expect("published");
    assert!(!r.held);
    // The published key version names the key sheet as its source, with the scan's SHA-384.
    let sha = omr_store::jcs::sha384_hex(&std::fs::read(&s.key_sheet).unwrap());
    let ev: Vec<_> = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "KeyVersionPublished" && e.payload["source"] == "key_sheet")
        .collect();
    assert_eq!(ev.len(), 1, "key version {v}");
    assert!(
        ev[0].payload["scans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["sha384"] == sha.as_str()),
        "{}",
        ev[0].payload
    );
    let want: Vec<(String, String)> = (0..4u16)
        .map(|k| (student_id(k), (k + 4).to_string()))
        .collect();
    assert_eq!(raw_scores(&p), want);
    p.close().unwrap();
}
