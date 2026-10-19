//! The New-exam proposal and the survey behind it (spec G-3 §C; design-opus §6 S1–S6 and
//! P1–P18), synthetic data only: IAU sheets from the committed blank form with synthetic marks
//! (tests/common), own sheets from `omr-sim`, Remark workbooks from tests/remark_common. Each
//! proposal is checked as the New-exam card shows it (value, source, questions and notes) and,
//! where it gives inputs, by grading with them: the forms and scores are the hand count's.

mod common;
mod remark_common;

use common::{Marks, blank_page, bleed_through, bubble, iau, png, sheet, write};
use omr_app::define::{Sheet, define};
use omr_app::model::FormSource;
use omr_app::propose::{
    Answer, ChoiceAction, Context, FieldId, Given, Proposal, RecentExam, Source, propose,
};
use omr_app::survey::{Survey, SurveyRequest, survey};
use omr_app::{ExamProject, GradeOptions};
use omr_forms::legacy::LegacyField;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The key of version `v` (0 = A … 3 = D) at question `q`: even questions are the same on
/// every version, odd ones differ.
fn key(v: u8, q: u16) -> u8 {
    let base = ((q * 3) % 5) as u8;
    if q.is_multiple_of(2) {
        base
    } else {
        (base + v) % 5
    }
}

fn letter(v: u8) -> String {
    char::from(b'A' + v).to_string()
}

/// What a sheet's version bubble shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shows {
    /// A confident letter (0 = A).
    Letter(u8),
    /// Nothing bubbled.
    Blank,
}

/// One sheet of a stack.
#[derive(Clone, Debug)]
struct S {
    /// File stem.
    name: String,
    shows: Shows,
    /// The version whose key the sheet answers.
    answered: u8,
    /// Rows answered (1..=rows).
    rows: u16,
    /// The student number, or a key sheet's marker.
    id: String,
    /// Exam-type bubble (3 = Midterm).
    exam_type: Option<u8>,
    /// Further rows marked (0-based), e.g. a stray mark far below the questions.
    stray: Vec<u16>,
    /// A key sheet: the key exactly.
    key_sheet: bool,
}

/// The fixture student numbers (fit the IAU scheme `2YYNNNNNNN`).
fn student_id(k: usize) -> String {
    format!("2210345{k:03}")
}

impl S {
    /// Student `k`: a strong student of version `answered` (wrong on every seventh question).
    fn student(k: usize, shows: Shows, answered: u8, rows: u16) -> Self {
        Self {
            name: format!("s{k:03}"),
            shows,
            answered,
            rows,
            id: student_id(k),
            exam_type: None,
            stray: Vec::new(),
            key_sheet: false,
        }
    }

    /// A key sheet of version `v` marked with `marker` in every student-number column.
    fn key_sheet(name: &str, v: u8, rows: u16, marker: u8) -> Self {
        Self {
            name: name.into(),
            shows: Shows::Letter(v),
            answered: v,
            rows,
            id: marker.to_string().repeat(10),
            exam_type: None,
            stray: Vec::new(),
            key_sheet: true,
        }
    }

    fn answers(&self) -> Vec<Option<u8>> {
        (0..self.rows)
            .map(|q| {
                let k = key(self.answered, q);
                Some(if !self.key_sheet && q % 7 == 3 {
                    (k + 1) % 5
                } else {
                    k
                })
            })
            .collect()
    }

    fn marks(&self, e: &omr_recog::LegacyEngine) -> Marks {
        let mut m = Marks::new(&self.answers(), &self.id, None);
        if let Shows::Letter(l) = self.shows {
            m.version = Some(l);
        }
        if let Some(t) = self.exam_type {
            m.extra.push(bubble(e, LegacyField::Exam, 0, t));
        }
        for &q in &self.stray {
            m.extra.push(bubble(e, LegacyField::Answers, q, 0));
        }
        m
    }

    /// The sheet's score under version `v`'s key over `n` questions.
    fn score(&self, v: u8, n: u16) -> usize {
        self.answers()
            .iter()
            .take(usize::from(n))
            .enumerate()
            .filter(|(q, a)| **a == Some(key(v, *q as u16)))
            .count()
    }
}

/// Students `first..first + count` showing `shows`, answering version `answered` to `rows`.
fn students(first: usize, count: usize, shows: Shows, answered: u8, rows: u16) -> Vec<S> {
    (first..first + count)
        .map(|k| S::student(k, shows, answered, rows))
        .collect()
}

/// Renders the sheets into `dir` as PNG files (`<name>.png`), in parallel.
fn render(dir: &Path, sheets: &[S]) -> PathBuf {
    let e = iau();
    std::fs::create_dir_all(dir).unwrap();
    sheets.par_iter().enumerate().for_each(|(k, s)| {
        let img = sheet(&e, &s.marks(&e), 500 + k as u64);
        write(dir, &format!("{}.png", s.name), &png(&img));
    });
    dir.to_path_buf()
}

fn files_of(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

fn iau_sheet() -> Sheet {
    Sheet::Template("iau-ccsit-v1".into())
}

fn survey_of(scans: &[PathBuf], key_sheets: &[PathBuf], threads: usize) -> Survey {
    let t0 = Instant::now();
    let s = survey(
        &SurveyRequest {
            sheet: iau_sheet(),
            scans: scans.to_vec(),
            key_sheets: key_sheets.to_vec(),
            roster: None,
            threads,
            cancel: Default::default(),
        },
        &|_| {},
    )
    .unwrap();
    eprintln!(
        "survey: {} pages at {threads} threads in {} ms",
        s.pages,
        t0.elapsed().as_millis()
    );
    s
}

fn given(scans: &[PathBuf]) -> Given {
    Given::new(iau_sheet(), scans.to_vec())
}

fn ctx(workspace: &Path) -> Context {
    Context {
        workspace: workspace.to_path_buf(),
        now_utc: "2026-10-19T08:00:00Z".into(),
        recent: Vec::new(),
        thresholds: Default::default(),
    }
}

/// A key file: `header` then one row per question `1..=n` of the versions' keys (`None` for an
/// empty cell).
fn key_csv(
    dir: &Path,
    name: &str,
    header: &str,
    n: u16,
    cells: &dyn Fn(u16) -> Vec<String>,
) -> PathBuf {
    let mut t = format!("{header}\n");
    for q in 0..n {
        t.push_str(&format!("{},{}\n", q + 1, cells(q).join(",")));
    }
    write(dir, name, t.as_bytes())
}

/// `question,A,B,…` for `versions`.
fn per_version_key(dir: &Path, versions: &[u8], n: u16) -> PathBuf {
    let header = format!(
        "question,{}",
        versions
            .iter()
            .map(|v| letter(*v))
            .collect::<Vec<_>>()
            .join(",")
    );
    key_csv(dir, "key.csv", &header, n, &|q| {
        versions.iter().map(|v| letter(key(*v, q))).collect()
    })
}

/// `question,key`: version A's key for every version.
fn shared_key(dir: &Path, n: u16) -> PathBuf {
    key_csv(dir, "key.csv", "question,key", n, &|q| {
        vec![letter(key(0, q))]
    })
}

fn asks(p: &Proposal) -> Vec<&omr_app::propose::Issue> {
    p.asks().collect()
}

fn ask_on(p: &Proposal, f: FieldId) -> &omr_app::propose::Issue {
    p.asks()
        .find(|i| i.field == f)
        .unwrap_or_else(|| panic!("no question on {f:?}: {:#?}", p.issues))
}

fn notes_on(p: &Proposal, f: FieldId) -> Vec<String> {
    p.notes()
        .filter(|i| i.field == f)
        .map(|i| i.text.clone())
        .collect()
}

fn value(p: &Proposal, f: FieldId) -> (String, Source) {
    let v = p.field(f).unwrap();
    (v.value.clone(), v.source)
}

/// The proposal has no open question and its inputs define the exam.
fn ready(p: &Proposal) -> &omr_app::define::Inputs {
    assert!(asks(p).is_empty(), "open questions: {:#?}", p.issues);
    let i = p.inputs.as_ref().expect("inputs without an open question");
    define(i).expect("define accepts the proposal's inputs");
    i
}

fn has_answer(p: &Proposal, f: FieldId, a: &Answer) -> bool {
    p.asks().any(|i| {
        i.field == f
            && i.choices
                .iter()
                .any(|c| c.action == ChoiceAction::Answer(a.clone()))
    })
}

/// Grades the scans in memory with the proposal's inputs.
fn grade_with(p: &Proposal, scans: &[PathBuf]) -> ExamProject {
    let (def, _) = define(p.inputs.as_ref().expect("inputs")).unwrap();
    let opts = GradeOptions {
        threads: 2,
        project_dir: None,
        snapshot_utc: Some("2026-10-19T08:00:00Z".into()),
        ..GradeOptions::default()
    };
    ExamProject::grade_with_key_sheets(&def, scans, &p.key_sheets, &opts, &|_| {}).unwrap()
}

/// The fixture sheet of booklet `bi` (by its file name).
fn sheet_of<'a>(p: &ExamProject, bi: usize, sheets: &'a [S]) -> &'a S {
    let name = common::source_of(p, bi);
    sheets
        .iter()
        .find(|s| format!("{}.png", s.name) == name)
        .unwrap_or_else(|| panic!("{name}"))
}

/// A listing of every file below `dir` with its size.
fn listing(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut v: Vec<(PathBuf, u64)> = walk(dir)
        .into_iter()
        .map(|p| {
            let len = std::fs::metadata(&p).unwrap().len();
            (p, len)
        })
        .collect();
    v.sort();
    v
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        }
        out.push(p);
    }
    out
}

/// S1, S5: the survey counts versions, rows, key sheets and student numbers; it writes nothing
/// and neither it nor the proposal holds a student number.
#[test]
fn the_survey_counts_versions_rows_and_key_sheets_and_keeps_no_student_number() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 8, Shows::Letter(0), 0, 40);
    sheets.extend(students(8, 8, Shows::Letter(1), 1, 40));
    sheets.extend(students(16, 6, Shows::Letter(2), 2, 40));
    sheets.extend(students(22, 2, Shows::Blank, 0, 40));
    sheets.push(S::key_sheet("key-a", 0, 40, 0));
    sheets.push(S::key_sheet("key-b", 1, 40, 9));
    sheets.push(S::key_sheet("key-c", 2, 40, 0));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let before = listing(tmp.path());
    let s = survey_of(std::slice::from_ref(&scans), &[], 4);
    assert_eq!(listing(tmp.path()), before, "the survey writes nothing");
    assert_eq!(
        (s.files, s.pages, s.booklets, s.blank_booklets),
        (27, 27, 24, 0)
    );
    assert_eq!(
        s.versions.by_letter,
        [("A", 8), ("B", 8), ("C", 6)]
            .into_iter()
            .map(|(l, c)| (l.to_string(), c))
            .collect()
    );
    assert_eq!((s.versions.blank, s.versions.unclear), (2, 0));
    assert_eq!(s.rows.len(), 100);
    assert!(
        s.rows[..40].iter().all(|r| r.marked == 24),
        "{:?}",
        &s.rows[..40]
    );
    assert!(s.rows[40..].iter().all(|r| r.marked == 0));
    let e = s.extent(&Default::default());
    assert_eq!((e.n50, e.last, e.high), (40, 40, true));
    assert_eq!(s.last_rows.get(&40), Some(&24));
    assert_eq!(s.key_sheets.len(), 3);
    for k in &s.key_sheets {
        assert!(!k.named && k.problem.is_none() && k.last_row == 40, "{k:?}");
        assert!(
            k.label.starts_with("key-") && k.label.ends_with(".png page 1"),
            "{k:?}"
        );
    }
    let mut kv: Vec<(&str, Option<u8>)> = s
        .key_sheets
        .iter()
        .map(|k| (k.version.as_str(), k.marker))
        .collect();
    kv.sort();
    assert_eq!(kv, [("A", Some(0)), ("B", Some(9)), ("C", Some(0))]);
    assert_eq!(s.ids.complete, 24);
    assert_eq!(s.ids.fit.get("iau"), Some(&24));
    // The proposal: key sheets give the key, versions A–C and 40 questions.
    let p = propose(&given(&[scans]), Some(&s), &ctx(tmp.path()));
    let i = ready(&p);
    assert_eq!(
        value(&p, FieldId::Versions),
        ("A, B, C".into(), Source::KeySheets)
    );
    assert_eq!(
        value(&p, FieldId::Questions),
        ("40".into(), Source::KeySheets)
    );
    assert_eq!(value(&p, FieldId::Key).1, Source::KeySheets);
    assert_eq!(i.forms, Some(vec!["A".into(), "B".into(), "C".into()]));
    assert!(!i.infer_key && i.key.is_none());
    assert!(
        notes_on(&p, FieldId::Versions)
            .iter()
            .any(|n| n.contains("2 sheets leave the version blank")),
        "{:#?}",
        p.issues
    );
    // Counts only: no student number in either, nor in the survey's JSON.
    let shown = format!("{s:?}{p:?}{}", serde_json::to_string(&s).unwrap());
    for k in 0..24 {
        assert!(!shown.contains(&student_id(k)), "student {k} leaked");
    }
    assert!(
        !shown.contains("2210345"),
        "a student number's prefix leaked"
    );
}

/// S2: two students' stray marks on row 77 (5 % of the sheets) neither move the extent nor
/// raise a question.
#[test]
fn stray_marks_far_below_the_questions_do_not_move_the_extent() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 40, Shows::Blank, 0, 30);
    sheets[0].stray.push(76);
    sheets[1].stray.push(76);
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let e = s.extent(&Default::default());
    assert_eq!((e.n50, e.last, e.high), (30, 77, true), "{e:?}");
    assert_eq!(s.rows[76].marked, 2);
    assert_eq!(s.last_rows.get(&77), Some(&2));
    let mut g = given(&[scans]);
    g.key = Some(shared_key(tmp.path(), 30));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    ready(&p);
    assert_eq!(
        value(&p, FieldId::Questions),
        ("30".into(), Source::KeyFile)
    );
    assert_eq!(value(&p, FieldId::Versions), ("A".into(), Source::Default));
}

/// S3: blank backs, bleed-through and a page that is no IAU sheet are counted apart; the
/// foreign page is named by its file and page, masked: a file named by a student number never
/// shows it.
#[test]
fn pages_that_are_not_the_sheet_are_counted_and_named() {
    let tmp = tempfile::tempdir().unwrap();
    let sheets = students(0, 4, Shows::Blank, 0, 20);
    let scans = render(&tmp.path().join("scans"), &sheets);
    let e = iau();
    let front = sheet(&e, &sheets[0].marks(&e), 500);
    write(&scans, "s000-back.png", &png(&bleed_through(&front, 3)));
    write(
        &scans,
        "s001-back.png",
        &png(&blank_page(front.width(), front.height(), 4)),
    );
    // A page of noise, saved under a student's number: no IAU sheet registers on it.
    let mut rng = omr_imaging::SplitMix64::new(9);
    let noise = omr_imaging::GrayImage::from_fn(front.width(), front.height(), |_, _| {
        (rng.next_u64() % 256) as u8
    });
    let named = student_id(99);
    write(&scans, &format!("{named}.png"), &png(&noise));
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    assert_eq!((s.pages, s.booklets, s.blank_pages), (7, 4, 2), "{s:#?}");
    assert_eq!(s.not_this_sheet.len(), 1, "{:?}", s.not_this_sheet);
    assert_eq!(s.not_this_sheet[0].label, "•••••••099.png page 1");
    let mut g = given(&[scans]);
    g.key = Some(shared_key(tmp.path(), 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    ready(&p);
    assert!(
        notes_on(&p, FieldId::Sheet)
            .iter()
            .any(|n| n.contains("•••••••099.png page 1")),
        "{:#?}",
        p.issues
    );
    let shown = format!("{s:?}{p:?}{}", serde_json::to_string(&s).unwrap());
    assert!(!shown.contains(&named), "{shown}");
}

/// S4, P18: the same files in another order, at 1 or 4 threads, give the same survey and the
/// same proposal.
#[test]
fn the_survey_and_the_proposal_do_not_depend_on_input_order_or_threads() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 4, Shows::Letter(0), 0, 25);
    sheets.extend(students(4, 4, Shows::Letter(1), 1, 25));
    sheets.extend(students(8, 1, Shows::Blank, 1, 25));
    sheets.push(S::key_sheet("key-a", 0, 25, 0));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let files = files_of(&scans);
    let mut shuffled = files.clone();
    shuffled.reverse();
    shuffled.swap(1, 6);
    let a = survey_of(&files, &[], 1);
    let b = survey_of(&shuffled, &[], 4);
    assert_eq!(a, b);
    let key = per_version_key(tmp.path(), &[0, 1], 25);
    let proposal = |scans: &[PathBuf], s: &Survey| {
        let mut g = given(scans);
        g.key = Some(key.clone());
        format!("{:?}", propose(&g, Some(s), &ctx(tmp.path())))
    };
    let p = proposal(&files, &a);
    assert_eq!(p, proposal(&shuffled, &b));
    assert_eq!(p, proposal(&files, &a), "two calls give equal proposals");
}

/// P1: one version and a key with one answer column: version A, nothing to ask, and grading
/// queues no version review.
#[test]
fn p1_one_version_with_one_answer_column_grades_without_version_review() {
    let tmp = tempfile::tempdir().unwrap();
    let sheets = students(0, 8, Shows::Blank, 0, 20);
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(shared_key(tmp.path(), 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    let i = ready(&p);
    assert_eq!(i.forms, Some(vec!["A".to_string()]));
    assert_eq!(
        value(&p, FieldId::Questions),
        ("20".into(), Source::KeyFile)
    );
    let x = grade_with(&p, &[scans]);
    let sc = x.scoring.as_ref().unwrap();
    for (bi, b) in x.session.booklets.iter().enumerate() {
        assert!(b.exceptions.iter().all(|e| e.code != "SCO-005"));
        let row = sc.row_of[bi].expect("scored");
        let want = sheet_of(&x, bi, &sheets).score(0, 20);
        assert_eq!(
            sc.run.as_ref().unwrap().students[row].raw.to_string(),
            want.to_string()
        );
    }
    assert!(x.queue(None).iter().all(|i| i.code != "SCO-005"));
}

/// P2: two versions named by the key's header; the blank bubbles are a note, and grading
/// infers exactly their two versions.
#[test]
fn p2_two_versions_from_the_key_header_and_blank_bubbles_noted() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 4, Shows::Letter(0), 0, 20);
    sheets.extend(students(4, 4, Shows::Letter(1), 1, 20));
    sheets.extend(students(8, 2, Shows::Blank, 1, 20));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(per_version_key(tmp.path(), &[0, 1], 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    ready(&p);
    assert_eq!(
        value(&p, FieldId::Versions),
        ("A, B".into(), Source::KeyFile)
    );
    let detail = &p.field(FieldId::Versions).unwrap().detail;
    assert!(detail.contains("A 4 · B 4 · blank 2"), "{detail}");
    assert!(
        notes_on(&p, FieldId::Versions)
            .iter()
            .any(|n| n.starts_with("2 sheets leave the version blank")),
        "{:#?}",
        p.issues
    );
    let x = grade_with(&p, &[scans]);
    let inferred = x
        .session
        .booklets
        .iter()
        .filter(|b| {
            b.exceptions
                .iter()
                .any(|e| e.code == "SCO-005" && e.sub_reason.as_deref() == Some("inferred"))
        })
        .count();
    assert_eq!(inferred, 2);
}

/// P3: four versions; each sheet is scored under its own version's key.
#[test]
fn p3_four_versions_score_each_sheet_under_its_own_key() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = Vec::new();
    for v in 0..4u8 {
        sheets.extend(students(usize::from(v) * 2, 2, Shows::Letter(v), v, 20));
    }
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(per_version_key(tmp.path(), &[0, 1, 2, 3], 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    ready(&p);
    assert_eq!(value(&p, FieldId::Versions).0, "A, B, C, D");
    let x = grade_with(&p, &[scans]);
    let sc = x.scoring.as_ref().unwrap();
    for (bi, b) in x.session.booklets.iter().enumerate() {
        let st = sheet_of(&x, bi, &sheets);
        assert_eq!(b.form, Some(usize::from(st.answered)), "{}", st.name);
        assert_eq!(b.form_source, FormSource::Bubbled);
        let row = sc.row_of[bi].unwrap();
        assert_eq!(
            sc.run.as_ref().unwrap().students[row].raw.to_string(),
            st.score(st.answered, 20).to_string()
        );
    }
}

/// P4, P5 (C1, C2): six sheets bubble C on an A,B key: a question; one bubbles D: a note.
/// Held after the answer, never scored; a key version without sheets may be read as C.
#[test]
fn p4_p5_a_version_the_key_lacks_is_asked_and_a_stray_one_noted_then_held() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 3, Shows::Letter(0), 0, 20);
    sheets.extend(students(3, 3, Shows::Letter(1), 1, 20));
    sheets.extend(students(6, 6, Shows::Letter(2), 2, 20));
    sheets.extend(students(12, 1, Shows::Letter(3), 3, 20));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(per_version_key(tmp.path(), &[0, 1], 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    assert!(p.inputs.is_none());
    let a = ask_on(&p, FieldId::Versions);
    assert!(
        a.text.contains("6 sheets bubble version C") && a.text.contains("key.csv has A, B"),
        "{}",
        a.text
    );
    let hold = Answer::HoldVersions(vec!["C".into()]);
    assert!(has_answer(&p, FieldId::Versions, &hold));
    assert!(
        a.choices
            .iter()
            .all(|c| !c.label.starts_with("Read key version")),
        "both key versions have sheets: {:?}",
        a.choices
    );
    assert_eq!(asks(&p).len(), 1, "{:#?}", p.issues);
    assert!(
        notes_on(&p, FieldId::Versions)
            .iter()
            .any(|n| n.starts_with("1 sheet bubbles version D")),
        "{:#?}",
        p.issues
    );
    g.answers.push(hold);
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    ready(&p);
    let x = grade_with(&p, std::slice::from_ref(&scans));
    let sc = x.scoring.as_ref().unwrap();
    let mut held = 0;
    for (bi, b) in x.session.booklets.iter().enumerate() {
        let st = sheet_of(&x, bi, &sheets);
        if st.answered >= 2 {
            held += 1;
            assert!(b.undeclared_version(), "{}", st.name);
            assert_eq!(
                (b.form, sc.row_of[bi]),
                (None, None),
                "{}: never scored",
                st.name
            );
        } else {
            let row = sc.row_of[bi].unwrap();
            assert_eq!(
                sc.run.as_ref().unwrap().students[row].raw.to_string(),
                st.score(st.answered, 20).to_string()
            );
        }
    }
    assert_eq!(held, 7);
    // Without B's sheets, key version B has no sheet: it may be read as C.
    let no_b: Vec<PathBuf> = files_of(&scans)
        .into_iter()
        .filter(|f| {
            let n = f.file_stem().unwrap().to_string_lossy().into_owned();
            !(3..6).any(|k| n == format!("s{k:03}"))
        })
        .collect();
    let s = survey_of(&no_b, &[], 2);
    let mut g = given(&no_b);
    g.key = Some(per_version_key(tmp.path(), &[0, 1], 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    let read_as = Answer::KeyVersions(vec!["A".into(), "C".into()]);
    assert!(
        has_answer(&p, FieldId::Versions, &read_as),
        "{:#?}",
        p.issues
    );
    g.answers.push(read_as);
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    let i = ready(&p);
    assert_eq!(i.forms, Some(vec!["A".to_string(), "C".into()]));
    let (def, _) = define(i).unwrap();
    let omr_app::KeySource::Provided { forms } = &def.key else {
        panic!("a provided key")
    };
    assert_eq!(forms[1].form, "C");
    assert_eq!(
        forms[1].answers[1],
        letter(key(1, 1)),
        "C is the file's column B"
    );
}

/// P6: one answer column while two versions are bubbled on many sheets: asked; the same key
/// for both, and sheets without a version are scored under it with no review (CR-16).
#[test]
fn p6_one_answer_column_with_two_bubbled_versions_asks_then_shares_the_key() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 4, Shows::Letter(0), 0, 20);
    sheets.extend(students(4, 4, Shows::Letter(1), 0, 20));
    sheets.extend(students(8, 2, Shows::Blank, 0, 20));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(shared_key(tmp.path(), 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    assert!(p.inputs.is_none());
    let a = ask_on(&p, FieldId::Versions);
    assert!(a.text.contains("one answer column"), "{}", a.text);
    assert!(has_answer(
        &p,
        FieldId::Versions,
        &Answer::SameKeyForEveryVersion
    ));
    g.answers.push(Answer::SameKeyForEveryVersion);
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    let i = ready(&p);
    assert_eq!(i.forms, Some(vec!["A".to_string(), "B".into()]));
    assert!(
        notes_on(&p, FieldId::Versions)
            .iter()
            .all(|n| !n.contains("blank"))
    );
    let x = grade_with(&p, &[scans]);
    let sc = x.scoring.as_ref().unwrap();
    for (bi, b) in x.session.booklets.iter().enumerate() {
        let st = sheet_of(&x, bi, &sheets);
        if st.shows == Shows::Blank {
            assert_eq!(b.form_source, FormSource::Shared, "{}", st.name);
        }
        let row = sc.row_of[bi].unwrap();
        assert_eq!(
            sc.run.as_ref().unwrap().students[row].raw.to_string(),
            st.score(0, 20).to_string()
        );
    }
    assert!(x.queue(None).iter().all(|i| i.code != "SCO-005"));
}

/// A scans folder holding one tiny image: enough for the files-only proposals (no survey).
fn dummy_scans(dir: &Path) -> PathBuf {
    let d = dir.join("scans");
    write(&d, "page.png", &png(&blank_page(8, 8, 1)));
    d
}

/// P7, P9 (Q1, Q4, Q5): the count is the key's last answered question; a typed count that
/// drops key rows, a count past the key, gaps and questions beyond the sheet are asked.
#[test]
fn p7_p9_the_count_and_the_gaps_come_from_the_key_file() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = dummy_scans(tmp.path());
    // Rows 1–40 answered, then the template's empty rows 41–100.
    let k40 = key_csv(tmp.path(), "key40.csv", "question,A,B", 100, &|q| {
        if q < 40 {
            vec![letter(key(0, q)), letter(key(1, q))]
        } else {
            vec![String::new(), String::new()]
        }
    });
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(k40);
    let c = ctx(tmp.path());
    let p = propose(&g, None, &c);
    ready(&p);
    let (q, src) = value(&p, FieldId::Questions);
    assert_eq!((q.as_str(), src), ("40", Source::KeyFile));
    assert!(
        p.field(FieldId::Questions)
            .unwrap()
            .detail
            .contains("rows to 100 are empty")
    );
    // Typed 30: never replaced, asked.
    g.typed.questions = Some("30".into());
    let p = propose(&g, None, &c);
    assert_eq!(value(&p, FieldId::Questions), ("30".into(), Source::Typed));
    let a = ask_on(&p, FieldId::Questions);
    assert!(
        a.text.contains("key40.csv answers questions 1–40"),
        "{}",
        a.text
    );
    assert!(has_answer(
        &p,
        FieldId::Questions,
        &Answer::UseQuestions(40)
    ));
    assert!(has_answer(
        &p,
        FieldId::Questions,
        &Answer::KeepQuestions(30)
    ));
    g.answers.push(Answer::KeepQuestions(30));
    let p = propose(&g, None, &c);
    assert_eq!(ready(&p).questions, 30);
    assert!(
        notes_on(&p, FieldId::Key)
            .iter()
            .any(|n| n.contains("questions 31–40 have an answer")),
        "{:#?}",
        p.issues
    );
    // An answer that names the count wins over the typed one.
    g.answers = vec![Answer::UseQuestions(40)];
    let p = propose(&g, None, &c);
    assert_eq!(ready(&p).questions, 40);
    assert_eq!(value(&p, FieldId::Questions), ("40".into(), Source::Answer));
    // Typed past the key: the questions without an answer are asked.
    g.answers.clear();
    g.typed.questions = Some("45".into());
    let p = propose(&g, None, &c);
    let a = ask_on(&p, FieldId::Key);
    assert!(
        a.text
            .contains("version A question 41–45; version B question 41–45"),
        "{}",
        a.text
    );
    assert!(has_answer(&p, FieldId::Key, &Answer::UseQuestions(40)));
    g.answers.push(Answer::GradeWithKeyGaps);
    assert_eq!(ready(&propose(&g, None, &c)).questions, 45);
    // P9: version B has no answer for question 12.
    let gap = key_csv(tmp.path(), "gap.csv", "question,A,B", 20, &|q| {
        let b = if q == 11 {
            String::new()
        } else {
            letter(key(1, q))
        };
        vec![letter(key(0, q)), b]
    });
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(gap);
    let p = propose(&g, None, &c);
    assert!(p.inputs.is_none());
    let a = ask_on(&p, FieldId::Key);
    assert!(
        a.text
            .contains("gap.csv has no answer for version B question 12"),
        "{}",
        a.text
    );
    g.answers.push(Answer::GradeWithKeyGaps);
    ready(&propose(&g, None, &c));
    // Q5: the key answers a question the sheet does not print.
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(write(
        tmp.path(),
        "far.csv",
        b"question,key\n1,A\n2,B\n120,C\n",
    ));
    let p = propose(&g, None, &c);
    assert!(
        p.asks().any(|i| i
            .text
            .contains("far.csv answers questions 120, but the sheet prints 100")),
        "{:#?}",
        p.issues
    );
    // The key uses option E on an exam typed with 4 options.
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(write(tmp.path(), "e.csv", b"question,key\n1,A\n2,E\n"));
    g.typed.options = Some("4".into());
    let p = propose(&g, None, &c);
    let a = ask_on(&p, FieldId::Options);
    assert!(a.text.contains("e.csv uses option E"), "{}", a.text);
    let five = Answer::UseValue(FieldId::Options, "5".into());
    assert!(has_answer(&p, FieldId::Options, &five));
    g.answers.push(five);
    assert_eq!(ready(&propose(&g, None, &c)).options, Some(5));
}

/// Numbered versions (`Version 1`) and unlabelled keys are asked; the answer reads them as the
/// letters it names. Headerless rows that start with their version are read as labelled.
#[test]
fn numbered_and_unlabelled_key_versions_are_asked_and_read_as_answered() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = dummy_scans(tmp.path());
    let c = ctx(tmp.path());
    let row = |label: &str, v: u8| {
        let mut r = label.to_string();
        for q in 0..20 {
            r.push(',');
            r.push_str(&letter(key(v, q)));
        }
        r.push('\n');
        r
    };
    let mut t = String::from("version");
    for q in 1..=20 {
        t.push_str(&format!(",Q{q}"));
    }
    t.push('\n');
    t.push_str(&row("Version 1", 0));
    t.push_str(&row("Version 2", 1));
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(write(tmp.path(), "numbered.csv", t.as_bytes()));
    let p = propose(&g, None, &c);
    let a = ask_on(&p, FieldId::Versions);
    assert!(a.text.contains("names its versions 1, 2"), "{}", a.text);
    let ab = Answer::KeyVersions(vec!["A".into(), "B".into()]);
    assert!(has_answer(&p, FieldId::Versions, &ab));
    g.answers.push(ab.clone());
    let (def, _) = define(ready(&propose(&g, None, &c))).unwrap();
    assert_eq!(def.forms, ["A", "B"]);
    let omr_app::KeySource::Provided { forms } = &def.key else {
        panic!()
    };
    for (f, k) in forms.iter().enumerate() {
        let want: Vec<String> = (0..20).map(|q| letter(key(f as u8, q))).collect();
        assert_eq!(k.answers, want, "version {}", k.form);
    }
    // Headerless `1,B,C` rows: two keys without a label.
    let mut t = String::new();
    for q in 0..20 {
        t.push_str(&format!(
            "{},{},{}\n",
            q + 1,
            letter(key(0, q)),
            letter(key(1, q))
        ));
    }
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(write(tmp.path(), "unlabelled.csv", t.as_bytes()));
    let p = propose(&g, None, &c);
    let a = ask_on(&p, FieldId::Versions);
    assert!(
        a.text.contains("has 2 keys without a version label"),
        "{}",
        a.text
    );
    g.answers.push(ab);
    assert_eq!(
        define(ready(&propose(&g, None, &c))).unwrap().0.forms,
        ["A", "B"]
    );
    // Headerless rows that start with their version: A and C.
    let t = format!("{}{}", row("A", 0), row("C", 2));
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(write(tmp.path(), "rows.csv", t.as_bytes()));
    let p = propose(&g, None, &c);
    assert_eq!(
        value(&p, FieldId::Versions),
        ("A, C".into(), Source::KeyFile)
    );
    assert_eq!(
        value(&p, FieldId::Questions),
        ("20".into(), Source::KeyFile)
    );
    assert!(
        notes_on(&p, FieldId::Versions)
            .iter()
            .any(|n| n.contains("no header row")),
        "{:#?}",
        p.issues
    );
    let (def, _) = define(ready(&p)).unwrap();
    assert_eq!(def.forms, ["A", "C"]);
}

/// P8 (Q2): the sheets answer rows 31–40 but the key covers 30: asked.
#[test]
fn p8_students_answering_rows_beyond_the_key_is_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let sheets = students(0, 10, Shows::Blank, 0, 40);
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(&[scans]);
    g.key = Some(shared_key(tmp.path(), 30));
    let c = ctx(tmp.path());
    let p = propose(&g, Some(&s), &c);
    let a = ask_on(&p, FieldId::Questions);
    assert!(
        a.text
            .contains("rows 31–40 are answered on up to 100 % of the sheets")
            && a.text.contains("has 30 questions"),
        "{}",
        a.text
    );
    assert!(has_answer(
        &p,
        FieldId::Questions,
        &Answer::UseQuestions(40)
    ));
    let mut keep = g.clone();
    keep.answers.push(Answer::KeepQuestions(30));
    assert_eq!(ready(&propose(&keep, Some(&s), &c)).questions, 30);
    // Using 40 needs a key that covers it: the key's missing rows are asked next.
    g.answers.push(Answer::UseQuestions(40));
    let p = propose(&g, Some(&s), &c);
    let a = ask_on(&p, FieldId::Key);
    assert!(
        a.text.contains("no answer for question 31–40"),
        "{}",
        a.text
    );
}

/// P10: named key sheets give the versions and the count; the sheets' shorter extent is a
/// note; grading reads the key sheet and waits for its confirmation.
#[test]
fn p10_the_count_comes_from_the_key_sheets() {
    let tmp = tempfile::tempdir().unwrap();
    let sheets = students(0, 6, Shows::Letter(0), 0, 30);
    let scans = render(&tmp.path().join("scans"), &sheets);
    let keys = render(&tmp.path().join("keys"), &[S::key_sheet("key-a", 0, 35, 0)]);
    let named = files_of(&keys);
    let s = survey_of(std::slice::from_ref(&scans), &named, 2);
    assert_eq!(s.key_sheets.len(), 1);
    assert!(s.key_sheets[0].named && s.key_sheets[0].last_row == 35);
    let mut g = given(std::slice::from_ref(&scans));
    g.key_sheets = named;
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    ready(&p);
    assert_eq!(
        value(&p, FieldId::Questions),
        ("35".into(), Source::KeySheets)
    );
    assert_eq!(
        value(&p, FieldId::Versions),
        ("A".into(), Source::KeySheets)
    );
    assert!(
        notes_on(&p, FieldId::Questions)
            .iter()
            .any(|n| n.contains("answered up to row 30 only, of 35")),
        "{:#?}",
        p.issues
    );
    let x = grade_with(&p, &[scans]);
    assert_eq!(x.session.key_sheets.sheets.len(), 1);
    assert!(x.session.key_unconfirmed());
    assert_eq!(x.session.definition.questions, 35);
}

/// P11: no key is asked, never inferred by default; after the explicit answer the key is
/// inferred for the version with enough sheets and the stray one is held.
#[test]
fn p11_no_key_is_asked_and_a_provisional_key_is_only_chosen() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 10, Shows::Letter(0), 0, 20);
    sheets.extend(students(10, 2, Shows::Letter(1), 1, 20));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    let c = ctx(tmp.path());
    let p = propose(&g, Some(&s), &c);
    assert!(p.inputs.is_none());
    let a = ask_on(&p, FieldId::Key);
    assert!(a.text.starts_with("no answer key"), "{}", a.text);
    assert!(has_answer(&p, FieldId::Key, &Answer::InferKeyProvisionally));
    g.answers.push(Answer::InferKeyProvisionally);
    let p = propose(&g, Some(&s), &c);
    let i = ready(&p);
    assert!(i.infer_key);
    assert_eq!(i.forms, Some(vec!["A".to_string()]));
    assert_eq!(value(&p, FieldId::Questions), ("20".into(), Source::Sheets));
    assert!(
        notes_on(&p, FieldId::Versions)
            .iter()
            .any(|n| n.starts_with("2 sheets bubble version B")),
        "{:#?}",
        p.issues
    );
    let x = grade_with(&p, &[scans]);
    assert!(x.session.key.as_ref().is_some_and(|k| k.inferred));
    let held = x
        .session
        .booklets
        .iter()
        .filter(|b| b.undeclared_version())
        .count();
    assert_eq!(held, 2);
}

/// P12 (C6): a Remark import gives the versions and the count; a version it lacks is asked; a
/// typed count or a key file against it is asked too.
#[test]
fn p12_a_remark_import_gives_the_versions_and_the_count() {
    use remark_common::{Book, Student, xlsx};
    let tmp = tempfile::tempdir().unwrap();
    let n = 10u16;
    let book = |v: u8, first: u64| {
        let key: Vec<String> = (0..n).map(|q| letter(key(v, q))).collect();
        let students = (0..4)
            .map(|k| {
                let refs: Vec<&str> = key.iter().map(String::as_str).collect();
                Student::new(&(first + k).to_string(), &letter(v), &refs)
            })
            .collect();
        Book {
            sections: false,
            key,
            points: vec![1.0; usize::from(n)],
            groups: Vec::new(),
            students,
            footer: true,
        }
    };
    let wbs: Vec<_> = [(0u8, 2_210_000_100u64), (1, 2_210_000_200)]
        .iter()
        .map(|(v, first)| {
            omr_app::remark::read_workbook(
                &format!("{}_Graded.xlsx", letter(*v)),
                &xlsx(&book(*v, *first)),
            )
            .unwrap()
        })
        .collect();
    let given_points: omr_app::remark::GivenPoints =
        [(String::new(), vec![Some("1".to_string()); usize::from(n)])]
            .into_iter()
            .collect();
    let imp = omr_app::remark::import(&wbs, &given_points).unwrap();
    let mut sheets = students(0, 4, Shows::Letter(0), 0, n);
    sheets.extend(students(4, 4, Shows::Letter(1), 1, n));
    sheets.extend(students(8, 3, Shows::Letter(2), 2, n));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.remark = Some(imp.clone());
    let c = ctx(tmp.path());
    let p = propose(&g, Some(&s), &c);
    assert_eq!(
        value(&p, FieldId::Versions),
        ("A, B".into(), Source::RemarkImport)
    );
    assert_eq!(
        value(&p, FieldId::Questions),
        ("10".into(), Source::RemarkImport)
    );
    let a = ask_on(&p, FieldId::Versions);
    assert!(
        a.text
            .contains("3 sheets bubble version C, but the Remark export has A, B"),
        "{}",
        a.text
    );
    g.answers.push(Answer::HoldVersions(vec!["C".into()]));
    let p = propose(&g, Some(&s), &c);
    let (def, _) = define(ready(&p)).unwrap();
    assert_eq!(
        def.key,
        omr_app::KeySource::Provided {
            forms: imp.form_keys()
        }
    );
    // A typed count other than the export's, or a key file beside it, is asked.
    let mut typed = g.clone();
    typed.typed.questions = Some("12".into());
    let p = propose(&typed, Some(&s), &c);
    assert!(
        ask_on(&p, FieldId::Questions)
            .text
            .contains("the Remark export has 10 questions")
    );
    let mut both = g.clone();
    both.key = Some(shared_key(tmp.path(), n));
    let p = propose(&both, Some(&s), &c);
    let a = ask_on(&p, FieldId::Key);
    assert!(
        a.choices
            .iter()
            .any(|c| c.action == ChoiceAction::Clear(FieldId::Key)),
        "{a:?}"
    );
}

/// P14 (Q6): sheets that end by half the extent while most run on: the scans may hold two
/// exams.
#[test]
fn p14_two_exams_in_one_stack_are_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 8, Shows::Blank, 0, 40);
    for s in &mut sheets {
        s.exam_type = Some(3);
    }
    sheets.extend(students(8, 6, Shows::Blank, 0, 15));
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    assert_eq!(s.exam_types.get("Midterm"), Some(&8));
    let mut g = given(&[scans]);
    g.key = Some(shared_key(tmp.path(), 40));
    let c = ctx(tmp.path());
    let p = propose(&g, Some(&s), &c);
    let a = ask_on(&p, FieldId::Scans);
    assert!(
        a.text.contains("6 of 14 sheets end by row 20") && a.text.contains("two exams"),
        "{}",
        a.text
    );
    g.answers.push(Answer::OneExam);
    ready(&propose(&g, Some(&s), &c));
}

/// P15, P16: the title from the exam-type bubble, the course from the folder name, the term
/// from the last exam of the exams folder; typed values are never replaced, a typed value the
/// evidence contradicts is asked and stays shown as typed.
#[test]
fn p15_p16_labels_come_from_the_evidence_and_typed_values_stay() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sheets = students(0, 6, Shows::Blank, 0, 20);
    for s in sheets.iter_mut().take(5) {
        s.exam_type = Some(3);
    }
    let scans = render(&tmp.path().join("CS211 Section 3"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(shared_key(tmp.path(), 20));
    let mut c = ctx(&tmp.path().join("exams"));
    c.recent = vec![
        RecentExam {
            term: "1448-1".into(),
            course: "CS 211".into(),
            graded_utc: "2026-10-07T10:00:00Z".into(),
        },
        RecentExam {
            term: "1447-2".into(),
            course: "CS 210".into(),
            graded_utc: "2026-03-01T10:00:00Z".into(),
        },
    ];
    let p = propose(&g, Some(&s), &c);
    let i = ready(&p);
    assert_eq!(
        value(&p, FieldId::Title),
        ("Midterm".into(), Source::Sheets)
    );
    assert!(
        p.field(FieldId::Title)
            .unwrap()
            .detail
            .contains("on 5 of 6 sheets")
    );
    assert_eq!(
        value(&p, FieldId::Course),
        ("CS 211".into(), Source::Folder)
    );
    assert_eq!(
        value(&p, FieldId::Term),
        ("1448-1".into(), Source::Workspace)
    );
    assert_eq!(
        (i.title.as_deref(), i.course.as_deref(), i.term.as_deref()),
        (Some("Midterm"), Some("CS 211"), Some("1448-1"))
    );
    assert_eq!(p.folder, tmp.path().join("exams").join("cs-211-midterm"));
    // Without the sheets the folder name gives the title (the course taken out of it).
    let p = propose(&g, None, &c);
    assert_eq!(
        value(&p, FieldId::Title),
        ("Section 3".into(), Source::Folder)
    );
    // An exam graded long ago gives no term.
    c.recent.remove(0);
    assert_eq!(
        value(&propose(&g, Some(&s), &c), FieldId::Term).1,
        Source::Default
    );
    // Typed values stay; one the evidence contradicts is asked, still shown as typed.
    g.typed.title = Some("Final".into());
    g.typed.course = Some("MATH 101".into());
    g.typed.term = Some("T2".into());
    g.typed.questions = Some("15".into());
    g.typed.id_scheme = Some("pnu".into());
    let p = propose(&g, Some(&s), &c);
    for (f, v) in [
        (FieldId::Title, "Final"),
        (FieldId::Course, "MATH 101"),
        (FieldId::Term, "T2"),
        (FieldId::Questions, "15"),
        (FieldId::IdScheme, "pnu"),
    ] {
        assert_eq!(value(&p, f), (v.to_string(), Source::Typed), "{f:?}");
    }
    ask_on(&p, FieldId::Questions);
    let a = ask_on(&p, FieldId::IdScheme);
    assert!(
        a.text
            .contains("only 0 of 6 student numbers fit pnu; 6 fit iau"),
        "{}",
        a.text
    );
    g.answers.push(Answer::UseTypedValue(FieldId::IdScheme));
    g.answers.push(Answer::KeepQuestions(15));
    let p = propose(&g, Some(&s), &c);
    let i = ready(&p);
    assert_eq!((i.questions, i.id_scheme.as_deref()), (15, Some("pnu")));
    assert_eq!(i.title.as_deref(), Some("Final"));
}

/// P17: a survey of other inputs is ignored, and the scans are to be checked again.
#[test]
fn p17_a_survey_of_other_inputs_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let sheets = students(0, 4, Shows::Letter(0), 0, 20);
    let scans = render(&tmp.path().join("scans"), &sheets);
    let files = files_of(&scans);
    let s = survey_of(&files[..2], &[], 2);
    let mut g = given(&files);
    g.key = Some(shared_key(tmp.path(), 20));
    let c = ctx(tmp.path());
    let stale = |p: &Proposal| {
        assert!(p.inputs.is_none());
        let a = ask_on(p, FieldId::Scans);
        assert!(a.text.contains("check them again"), "{}", a.text);
        assert_eq!(a.choices[0].action, ChoiceAction::Survey);
        assert!(
            !p.field(FieldId::Versions)
                .unwrap()
                .detail
                .contains("sheets A")
        );
    };
    stale(&propose(&g, Some(&s), &c));
    let s = survey_of(&files, &[], 2);
    ready(&propose(&g, Some(&s), &c));
    // A scan replaced after the survey.
    let e = iau();
    write(
        &scans,
        "s000.png",
        &png(&sheet(&e, &sheets[1].marks(&e), 77)),
    );
    stale(&propose(&g, Some(&s), &c));
}

/// The setup records each value with its source and evidence, counts only.
#[test]
fn the_setup_records_where_each_value_came_from() {
    let tmp = tempfile::tempdir().unwrap();
    let sheets = students(0, 4, Shows::Letter(1), 1, 20);
    let scans = render(&tmp.path().join("scans"), &sheets);
    let s = survey_of(std::slice::from_ref(&scans), &[], 2);
    let mut g = given(std::slice::from_ref(&scans));
    g.key = Some(shared_key(tmp.path(), 20));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    let (def, _) = define(ready(&p)).unwrap();
    let field = |name: &str| {
        def.provenance
            .iter()
            .find(|f| f.field == name)
            .unwrap_or_else(|| panic!("{name}: {:?}", def.provenance))
            .clone()
    };
    assert_eq!(
        (field("versions").value, field("versions").source),
        ("B".to_string(), "the sheets".to_string())
    );
    assert_eq!(field("questions").source, "key file");
    let out = tmp.path().join("out");
    let opts = GradeOptions {
        threads: 2,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-19T08:00:00Z".into()),
        ..GradeOptions::default()
    };
    ExamProject::grade(&def, &[scans], &opts, &|_| {})
        .unwrap()
        .close()
        .unwrap();
    let x = ExamProject::open(&out.join("project.omrproj"), "principal:test").unwrap();
    assert_eq!(x.session.definition.provenance, def.provenance);
    let shown = format!("{:?}", def.provenance);
    assert!((0..4).all(|k| !shown.contains(&student_id(k))), "{shown}");
}

/// S6, P13: own sheets are surveyed by their verified page codes, under the spec's questions,
/// forms and cancel mode; a `cross_cancels` spec grades with nothing typed, and a spec without
/// a cancel mode is asked.
#[test]
fn s6_p13_own_sheets_are_surveyed_by_their_page_codes_with_the_specs_cancel_mode() {
    use omr_forms::{ExamKey, Template, TemplateSpec};
    const KEY: [u8; 32] = [0x6c; 32];
    let tmp = tempfile::tempdir().unwrap();
    let mut spec = TemplateSpec::standard(
        omr_model::ExamId(uuid::Uuid::from_u128(
            0x0190_aaaa_bbbb_7ccc_8ddd_0000_0000_0123,
        )),
        20,
        4,
        omr_model::CancelMode::CrossCancels,
    );
    spec.forms = 2;
    let spec_file = |name: &str, s: &TemplateSpec| {
        let json = serde_json::json!({"template": s, "exam_key_hex": hex::encode(KEY)});
        write(tmp.path(), name, &serde_json::to_vec(&json).unwrap())
    };
    let cross = spec_file("cross.json", &spec);
    let corpus = tmp.path().join("corpus");
    let mut cfg = omr_sim::CorpusConfig::standard(8, 41);
    cfg.roster = (0..8).map(|i| format!("4511{:05}", 100 + i * 7)).collect();
    cfg.threads = 2;
    omr_sim::corpus::write_corpus(
        &Template::build(spec.clone()).unwrap(),
        &ExamKey::from_bytes(KEY),
        &cfg,
        &corpus,
    )
    .unwrap();
    let truth = omr_sim::corpus::read_truth(&corpus.join("truth.jsonl")).unwrap();
    let images = corpus.join("images");
    let req = SurveyRequest {
        sheet: Sheet::Spec(cross.clone()),
        scans: vec![images.clone()],
        key_sheets: Vec::new(),
        roster: None,
        threads: 2,
        cancel: Default::default(),
    };
    let s = survey(&req, &|_| {}).unwrap();
    assert_eq!(s.booklets, 8);
    let mut want = std::collections::BTreeMap::new();
    for t in &truth {
        *want.entry(letter((t.form - 1) as u8)).or_insert(0u32) += 1;
    }
    assert_eq!(s.versions.by_letter, want, "the page codes' forms");
    let mut g = Given::new(Sheet::Spec(cross.clone()), vec![images.clone()]);
    g.key = Some(key_csv(tmp.path(), "key.csv", "question,A,B", 20, &|q| {
        vec![letter(key(0, q) % 4), letter(key(1, q) % 4)]
    }));
    let p = propose(&g, Some(&s), &ctx(tmp.path()));
    let i = ready(&p);
    assert_eq!(
        value(&p, FieldId::CancelMode),
        ("cross_cancels".into(), Source::SpecFile)
    );
    assert_eq!(
        value(&p, FieldId::IdScheme),
        ("digits:9".into(), Source::SpecFile)
    );
    assert_eq!(i.cancel_mode, None, "the spec's, nothing typed");
    let x = grade_with(&p, std::slice::from_ref(&images));
    assert_eq!(
        x.session.definition.cancel_mode,
        omr_model::CancelMode::CrossCancels
    );
    for b in &x.session.booklets {
        let t = truth.iter().find(|t| t.booklet == b.serial).unwrap();
        assert_eq!(
            b.form,
            Some(usize::from(t.form - 1)),
            "booklet {}",
            b.serial
        );
    }
    // A spec without a cancel mode: the survey cannot read the sheets, the proposal asks.
    spec.cancel_mode = None;
    let unset = spec_file("unset.json", &spec);
    let req = SurveyRequest {
        sheet: Sheet::Spec(unset.clone()),
        ..req
    };
    assert!(
        survey(&req, &|_| {})
            .unwrap_err()
            .to_string()
            .contains("cancel_mode")
    );
    let mut g = Given::new(Sheet::Spec(unset), vec![images]);
    g.key = Some(tmp.path().join("key.csv"));
    let p = propose(&g, None, &ctx(tmp.path()));
    assert!(p.inputs.is_none());
    let a = ask_on(&p, FieldId::CancelMode);
    assert!(
        a.text.contains("unset.json sets no cancel_mode"),
        "{}",
        a.text
    );
    assert_eq!(a.choices[0].action, ChoiceAction::Pick(FieldId::Sheet));
}
