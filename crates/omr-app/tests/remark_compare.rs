//! The parallel run with Remark (spec §23.7, §3.5) on a synthetic exam: IAU sheets rendered on
//! the committed blank form (invented students), graded with the key and points imported from
//! synthetic Remark workbooks, then compared with those workbooks: agreements, one answer
//! Remark read differently (with its crop), a booklet Remark has no row for, a Remark row with
//! no booklet, an ID Remark lists differently (paired by answers), Remark's ID equal to the
//! handwritten one, the key sheet Remark graded as a student (and its effect on the reported
//! statistics), another section's workbook (not compared), and the Restricted report.

mod common;
mod remark_common;

use common::{Marks, iau, legacy_def, options, sheet, write};
use omr_app::exam::read_key_file_full;
use omr_app::parallel::{CompareOptions, HandwrittenId, IdentityKind, RESTRICTED_BANNER};
use omr_app::remark::{GivenPoints, RemarkWorkbook, import, read_workbook};
use omr_app::{ExamProject, KeySource};
use omr_scoring::exact_text;
use remark_common::{Book, Student, letter, xlsx};
use std::path::PathBuf;

const N: usize = 12;

fn key(v: u8, q: usize) -> u8 {
    ((q as u8) * 3 + 2 * v) % 5
}

/// The synthetic students: ID, version (0 = A), wrong questions (0-based), blank questions.
fn roster() -> Vec<(&'static str, u8, Vec<usize>, Vec<usize>)> {
    vec![
        ("2210000001", 0, vec![3, 9], vec![]),
        ("2210000002", 0, vec![2, 7], vec![]),
        ("2210000003", 0, vec![1, 5, 10], vec![]),
        ("2210000004", 0, vec![0, 4], vec![]),
        ("2210000005", 1, vec![6], vec![11]),
        ("2210000006", 1, vec![8, 2], vec![]),
        ("2210000007", 1, vec![0, 3, 9], vec![]),
    ]
}

fn answers(v: u8, wrong: &[usize], blank: &[usize]) -> Vec<Option<u8>> {
    (0..N)
        .map(|q| {
            if blank.contains(&q) {
                None
            } else if wrong.contains(&q) {
                Some((key(v, q) + 1) % 5)
            } else {
                Some(key(v, q))
            }
        })
        .collect()
}

fn text(a: &[Option<u8>]) -> Vec<String> {
    a.iter()
        .map(|o| o.map(letter).unwrap_or_default())
        .collect()
}

fn student(id: &str, v: &str, a: &[String]) -> Student {
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    Student::new(id, v, &refs)
}

fn books() -> Vec<Book> {
    let ka: Vec<String> = (0..N).map(|q| letter(key(0, q))).collect();
    let kb: Vec<String> = (0..N).map(|q| letter(key(1, q))).collect();
    let pa: Vec<f64> = (0..N).map(|q| if q < 6 { 0.75 } else { 1.0 }).collect();
    let pb: Vec<f64> = (0..N).map(|q| if q < 4 { 1.0 } else { 0.75 }).collect();
    let ga = vec![("Q1".to_string(), 1, 6), ("Q2".to_string(), 7, 12)];
    let gb = vec![("Q1".to_string(), 1, 4), ("Q2".to_string(), 5, 12)];
    let r = roster();
    let row = |k: usize| {
        let (id, v, w, b) = &r[k];
        student(
            id,
            if *v == 0 { "A" } else { "B" },
            &text(&answers(*v, w, b)),
        )
    };
    // Section 1, version A: s0, s1 (question 6 read differently), s2 under another ID, a row
    // with no booklet, the key sheet. s3 has no row.
    let mut s1 = row(1);
    s1.answers[5] = letter((key(0, 5) + 2) % 5);
    let mut s2 = row(2);
    s2.id = remark_common::Id::Text("2210000093".into());
    let extra = student("2210000555", "A", &vec!["E".to_string(); N]);
    let ks = student("0000000000", "A", &ka);
    let a1 = vec![row(0), s1, s2, extra, ks];
    // Section 1, version B: s4, s5, s6 under the number written by hand.
    let mut s6 = row(6);
    s6.id = remark_common::Id::Text("2210000077".into());
    let b1 = vec![row(4), row(5), s6];
    // Section 2, version A: not scanned here.
    let other = vec![
        // (wrong on question 12, so that the totals tell questions 7 and 12 apart)
        student("2210000801", "A", &text(&answers(0, &[1, 2, 3, 11], &[]))),
        student("2210000802", "A", &text(&answers(0, &[4], &[]))),
    ];
    // Section 2, version B: not scanned here either; its rows tell every question's points
    // apart (each question wrong for a distinct subset of them), as a class of any size does.
    let other_b: Vec<Student> = (0..4)
        .map(|b| {
            let wrong: Vec<usize> = (0..N).filter(|q| ((q + 1) >> b) & 1 == 1).collect();
            student(
                &format!("22100009{b:02}"),
                "B",
                &text(&answers(1, &wrong, &[])),
            )
        })
        .collect();
    let mk = |k: &Vec<String>, p: &Vec<f64>, g: &Vec<(String, u16, u16)>, s| Book {
        sections: true,
        key: k.clone(),
        points: p.clone(),
        groups: g.clone(),
        students: s,
        footer: true,
    };
    vec![
        mk(&ka, &pa, &ga, a1),
        mk(&kb, &pb, &gb, b1),
        mk(&ka, &pa, &ga, other),
        mk(&kb, &pb, &gb, other_b),
    ]
}

fn workbooks() -> Vec<RemarkWorkbook> {
    books()
        .iter()
        .zip([
            "Sec1_A_Graded.xlsx",
            "Sec1_B_Graded.xlsx",
            "Sec2_A_Graded.xlsx",
            "Sec2_B_Graded.xlsx",
        ])
        .map(|(b, n)| read_workbook(n, &xlsx(b)).unwrap())
        .collect()
}

/// Grades the rendered sheets with the key and points imported from the workbooks.
fn graded(dir: &std::path::Path) -> ExamProject {
    graded_with(dir, true)
}

/// Grades the rendered sheets with the imported key, and with its points (`weighted`) or one
/// point per question.
fn graded_with(dir: &std::path::Path, weighted: bool) -> ExamProject {
    let imp = import(&workbooks(), &GivenPoints::new()).unwrap();
    let e = iau();
    let mut files: Vec<PathBuf> = Vec::new();
    for (k, (id, v, w, b)) in roster().iter().enumerate() {
        let mut m = Marks::new(&answers(*v, w, b), id, Some(*v));
        if k == 4 {
            // A light second mark on question 4: our answer waits for review.
            let o = (key(*v, 3) + 2) % 5;
            m.light.push(common::bubble(
                &e,
                omr_forms::legacy::LegacyField::Answers,
                3,
                o,
            ));
        }
        if k == 5 {
            // The last student-number column left blank: our read is incomplete.
            m.id[9].clear();
        }
        let img = sheet(&e, &m, 40 + k as u64);
        files.push(write(
            &dir.join("scans"),
            &format!("sheet-{k:02}.png"),
            &common::png(&img),
        ));
    }
    let mut def = legacy_def("Final", N as u16, &["A", "B"], KeySource::None);
    let kf = read_key_file_full(imp.key_csv().as_bytes(), &def.forms, N as u16, 5).unwrap();
    def.key = KeySource::Provided { forms: kf.forms };
    if weighted {
        def.set_key_points(&kf.points);
    }
    def.outcomes =
        Some(omr_app::exam::read_clo_csv(imp.clo_csv().unwrap().as_bytes(), 12).unwrap());
    def.course_code = "TST 101".into();
    def.validate().unwrap();
    let out = dir.join("exam");
    let mut p = ExamProject::grade(&def, &files, &options(&out, 2), &|_| {}).unwrap();
    p.export(&out).unwrap();
    p
}

#[test]
fn the_parallel_run_reports_agreements_differences_and_identities() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = graded(dir.path());
    assert_eq!(p.session.booklets.len(), 7);
    let wbs = workbooks();
    let opts = CompareOptions {
        given_points: GivenPoints::new(),
        handwritten: vec![HandwrittenId {
            file: "sheet-06.png".into(),
            page: 1,
            ids: vec!["2210000077".into()],
        }],
        crops: true,
    };
    let run = p.compare_remark(&wbs, &opts).unwrap();
    let c = &run.counts;
    assert_eq!(c.booklets, 7);
    assert_eq!(c.remark_rows, 14);
    assert_eq!(c.paired_by_id, 3, "s0, s1, s4");
    assert_eq!(
        c.paired_by_partial_id, 1,
        "s5: Remark's ID completes our read"
    );
    assert_eq!(c.paired_by_answers, 2, "s2 and s6");
    assert_eq!(c.key_differences, 0);
    // Another section's workbook: listed, not compared.
    assert!(run.workbooks[0].in_scope && run.workbooks[1].in_scope);
    assert!(!run.workbooks[2].in_scope && !run.workbooks[3].in_scope);
    assert_eq!(c.out_of_scope_rows, 6);
    // Identity differences: exactly these.
    let kinds: Vec<IdentityKind> = run.identity.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        vec![
            IdentityKind::NoRemarkRow,
            IdentityKind::NoBooklet,
            IdentityKind::DifferentId,
            IdentityKind::RemarkIdIsHandwritten,
        ],
        "{:#?}",
        run.identity
    );
    let diff = run
        .identity
        .iter()
        .find(|d| d.kind == IdentityKind::DifferentId)
        .unwrap();
    assert_eq!(
        (diff.our_id.as_str(), diff.remark_id.as_str()),
        ("2210000003", "2210000093")
    );
    assert!(
        diff.detail.contains("12 of 12 answers identical"),
        "{}",
        diff.detail
    );
    let hand = run
        .identity
        .iter()
        .find(|d| d.kind == IdentityKind::RemarkIdIsHandwritten)
        .unwrap();
    assert_eq!(hand.remark_id, "2210000077");
    assert!(hand.booklet.contains("sheet-06.png"));
    let none = &run.identity[0];
    assert_eq!(none.our_id, "2210000004");
    // Per question: one difference (s1, question 6), with its crop; everything else equal.
    assert_eq!(c.questions, 6 * N);
    assert_eq!(c.differ, 1);
    assert!(
        c.agree_pending + c.differ_pending == 1,
        "s4's question 4 waits"
    );
    assert_eq!(c.agree + c.agree_pending + c.differ_pending, 6 * N - 1);
    // The difference, and s4's question 4 in review with Remark's reading beside ours.
    assert_eq!(run.disagreements.len(), 2);
    assert!(
        run.disagreements
            .iter()
            .any(|d| d.question == 4 && d.status == "pending")
    );
    let d = run.disagreements.iter().find(|d| d.question == 6).unwrap();
    assert_eq!(d.question, 6);
    assert_eq!(d.student, "2210000002");
    assert_eq!(d.remark, letter((key(0, 5) + 2) % 5));
    assert_eq!(d.ours, letter(key(0, 5)));
    let crop = d.crop.as_ref().expect("bubble-row crop");
    assert!(crop.width > 100 && crop.height > 10 && crop.bubbles.len() == 5);
    // Per student: weighted totals, equal except s1 (Remark missed 0.75 on question 6).
    let st = |id: &str| run.students.iter().find(|s| s.our_id == id).unwrap();
    for id in ["2210000001", "2210000003", "2210000007"] {
        let s = st(id);
        assert!(s.settled);
        assert_eq!(Some(s.ours), s.remark, "{id}");
    }
    let s5 = run
        .students
        .iter()
        .find(|s| s.remark_id == "2210000006")
        .unwrap();
    assert!(
        s5.how.contains("incomplete") && s5.our_id.is_empty(),
        "{s5:?}"
    );
    assert_eq!(Some(s5.ours), s5.remark);
    // s4: provisional; Remark's total once question 4 is resolved as Remark read it.
    let s4 = st("2210000005");
    assert!(!s4.settled);
    assert_eq!(s4.pending, vec![4]);
    assert_eq!(Some(s4.ours_if_remark), s4.remark);
    assert_eq!(
        (c.totals_provisional, c.totals_provisional_equal_if_remark),
        (1, 1)
    );
    let s1 = st("2210000002");
    assert_eq!(
        exact_text(&(s1.ours - s1.remark.unwrap())),
        "0.75",
        "Remark did not credit question 6"
    );
    // s0 on version A: 0.75 × 5 + 1 × 5 (questions 4 and 10 wrong).
    assert_eq!(exact_text(&st("2210000001").ours), "8.75");
    assert_eq!(exact_text(&st("2210000001").max), "10.5");
    assert!(!st("2210000001").grade_ours.is_empty());
    assert_eq!(c.totals_settled, 5);
    assert_eq!(c.totals_settled_equal, 4);
    assert!(st("2210000004").remark.is_none());
    // The learning objectives (CLO results, §3.5 item 6): two per settled pair, s1's first
    // objective short of question 6 in Remark.
    assert_eq!(
        (c.objective_subtotals, c.objective_subtotals_equal),
        (10, 9)
    );
    // The scores of the exam itself use each version's own points.
    let sc = p.scoring.as_ref().unwrap();
    let run_scores = sc.run.as_ref().unwrap();
    let b0 = p
        .session
        .booklets
        .iter()
        .position(|b| b.student() == Some("2210000001"))
        .unwrap();
    let row = sc.row_of[b0].unwrap();
    assert_eq!(run_scores.students[row].raw, omr_scoring::exact(35, 4));
    // The key sheet Remark graded as a student, and what it did to its statistics.
    assert_eq!(run.key_sheets.len(), 1);
    let k = &run.key_sheets[0];
    assert_eq!(k.workbook, "Sec1_A_Graded.xlsx");
    assert_eq!(k.rows.len(), 1);
    let rep = k.reported.as_ref().unwrap();
    assert!((rep.mean.unwrap() - k.with.mean.unwrap()).abs() < 1e-9);
    assert!((rep.kr20.unwrap() - k.with.kr20.unwrap()).abs() < 1e-9);
    assert!((rep.alpha.unwrap() - k.with.alpha.unwrap()).abs() < 1e-9);
    assert_eq!(k.without.n, k.with.n - 1);
    assert_eq!(k.with.max_percent, Some(100.0));
    assert!(k.without.max_percent.unwrap() < 100.0);
    assert!(k.without.mean.unwrap() < k.with.mean.unwrap());
    // The report: written only where asked, Restricted, private files, signed, recorded.
    let out = dir.path().join("report");
    let sum = p.export_parallel_run(&run, &out).unwrap();
    assert_eq!(sum.files.len(), 6);
    for f in &sum.files {
        let bytes = std::fs::read(out.join(f)).unwrap();
        let t = String::from_utf8_lossy(&bytes);
        if f != "manifest.json" {
            assert!(t.contains(RESTRICTED_BANNER), "{f}");
        }
        if f.ends_with(".csv") {
            // The banner is the first line, after the UTF-8 BOM.
            let first = t
                .trim_start_matches('\u{feff}')
                .lines()
                .next()
                .unwrap_or("");
            assert!(
                first.trim_start_matches('"').starts_with("RESTRICTED"),
                "{f}"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(out.join(f)).unwrap().permissions().mode();
            assert_eq!(m & 0o077, 0, "{f} is private");
        }
    }
    let html = std::fs::read_to_string(out.join("parallel_run.html")).unwrap();
    assert!(html.contains("<svg class=\"crop\"") && html.contains("Identity differences"));
    assert!(html.contains("Key sheets graded as students"));
    let q = std::fs::read_to_string(out.join("parallel_run_questions.csv")).unwrap();
    assert_eq!(q.lines().count(), 2 + 6 * N);
    let exports = p.store().unwrap().reader().exports().unwrap();
    assert!(exports.iter().any(|e| e.purpose == "parallel-run" && e.pii));
    // A sync folder is refused (SYS-015).
    std::fs::create_dir_all(dir.path().join("Dropbox").join(".dropbox")).unwrap();
    let e = p
        .export_parallel_run(&run, &dir.path().join("Dropbox").join("report"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("SYS-015"), "{e}");
    p.close().unwrap();
}

#[test]
fn workbooks_of_another_exam_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = graded(dir.path());
    // Another exam: 5 questions.
    let b = Book {
        sections: false,
        key: vec!["A".into(); 5],
        points: vec![1.0; 5],
        groups: Vec::new(),
        students: vec![student("2210000001", "A", &vec!["A".to_string(); 5])],
        footer: false,
    };
    let w = read_workbook("other.xlsx", &xlsx(&b)).unwrap();
    let e = p
        .compare_remark(&[w], &CompareOptions::default())
        .unwrap_err()
        .to_string();
    assert!(e.contains("5 questions") && e.contains("12"), "{e}");
    p.close().unwrap();
}

fn read_books(books: &[Book]) -> Vec<RemarkWorkbook> {
    books
        .iter()
        .zip([
            "Sec1_A_Graded.xlsx",
            "Sec1_B_Graded.xlsx",
            "Sec2_A_Graded.xlsx",
            "Sec2_B_Graded.xlsx",
        ])
        .map(|(b, n)| read_workbook(n, &xlsx(b)).unwrap())
        .collect()
}

#[test]
fn a_booklet_with_another_students_number_is_paired_with_its_own_row() {
    // Regression: an ID match was taken whatever the answers. s2 bubbled 2210000003, which
    // here is also a student of section 2 (not scanned); Remark lists s2 under 2210000093.
    // The booklet was paired with the section-2 student's row by ID, which put that section in
    // scope and reported its other students as rows with no booklet.
    let dir = tempfile::tempdir().unwrap();
    let p = graded(dir.path());
    let mut books = books();
    books[2].students.push(student(
        "2210000003",
        "A",
        &text(&answers(0, &[0, 6, 8, 11], &[])),
    ));
    let wbs = read_books(&books);
    let run = p.compare_remark(&wbs, &CompareOptions::default()).unwrap();
    let c = &run.counts;
    assert!(!run.workbooks[2].in_scope, "{:?}", run.workbooks);
    assert_eq!(c.paired_by_answers, 2, "s2 and s6");
    let kinds: Vec<IdentityKind> = run.identity.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        vec![
            IdentityKind::NoRemarkRow,
            IdentityKind::NoBooklet,
            IdentityKind::DifferentId,
            IdentityKind::DifferentId,
        ],
        "{:#?}",
        run.identity
    );
    let d = run
        .identity
        .iter()
        .find(|d| d.our_id == "2210000003")
        .unwrap();
    assert_eq!(d.remark_id, "2210000093");
    // Without its own row, the booklet is still compared with the row under its number, and
    // the identity is flagged.
    books[0]
        .students
        .retain(|s| !matches!(&s.id, remark_common::Id::Text(t) if t == "2210000093"));
    let run = p
        .compare_remark(&read_books(&books), &CompareOptions::default())
        .unwrap();
    let d = run
        .identity
        .iter()
        .find(|d| d.kind == IdentityKind::SameIdAnswersDiffer)
        .expect("flagged");
    assert_eq!(
        (d.our_id.as_str(), d.remark_id.as_str()),
        ("2210000003", "2210000003")
    );
    assert!(
        d.detail.contains("5 of 12 answers identical"),
        "{}",
        d.detail
    );
    p.close().unwrap();
}

#[test]
fn workbooks_of_another_test_of_the_same_students_are_refused() {
    // The same students and question count, another test: every ID matches, no answers do.
    let dir = tempfile::tempdir().unwrap();
    let p = graded(dir.path());
    let shift = |a: &String| {
        if a.is_empty() {
            String::new()
        } else {
            letter((a.as_bytes()[0] - b'A' + 1) % 5)
        }
    };
    let mut books = books();
    for b in &mut books {
        b.key = b.key.iter().map(shift).collect();
        for s in &mut b.students {
            s.answers = s.answers.iter().map(shift).collect();
        }
    }
    let e = p
        .compare_remark(&read_books(&books), &CompareOptions::default())
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("none of the 7 booklets") && e.contains("not of this exam's sheets"),
        "{e}"
    );
    p.close().unwrap();
}

#[test]
fn a_mapping_cell_that_stands_for_several_versions_rows_is_not_edited_silently() {
    // The imported learning objectives differ between the versions (rows per version). A
    // Setup grid cell names a printed question and a CLO: editing it changed the first
    // matching row only, one version's, silently. Refused with the way to change them.
    let dir = tempfile::tempdir().unwrap();
    let mut p = graded(dir.path());
    assert!(p.session.definition.has_form_mapping());
    let before = p.session.definition.outcomes.clone();
    let e = p
        .set_mapping_weight(5, 0, Some(0.5))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("differ between versions") && e.contains("--clo"),
        "{e}"
    );
    assert!(p.set_mapping_note(5, 0, "parts a-b").is_err());
    assert_eq!(p.session.definition.outcomes, before);
    // F8: the grid labels a cell that stands for one version's rows only.
    let v = p.setup_vm();
    let row5 = v.mapping.iter().find(|r| r.question == 5).unwrap();
    assert!(
        row5.cells[0].1.starts_with("version A only: "),
        "{:?}",
        row5.cells
    );
    assert!(
        row5.cells[1].1.starts_with("version B only: "),
        "{:?}",
        row5.cells
    );
    p.close().unwrap();
}

#[test]
fn the_totals_are_the_exams_own_grades_not_a_recomputation_with_remarks_points() {
    // Regression (F1): "our" total was recomputed from the answers with Remark's imported
    // points, whatever points the exam was graded with: an exam graded at 1 point per question
    // was reported with every settled total equal to Remark's weighted one. The totals are now
    // the exam's score run; the points and the maximum that differ are reported.
    let dir = tempfile::tempdir().unwrap();
    let p = graded_with(dir.path(), false);
    let run = p
        .compare_remark(&workbooks(), &CompareOptions::default())
        .unwrap();
    let c = &run.counts;
    assert_eq!(c.totals_settled, 5);
    assert_eq!(c.totals_settled_equal, 0, "the exam's totals are out of 12");
    assert_eq!(c.totals_provisional_equal_if_remark, 0);
    // A check of the readings alone still agrees where the answers do.
    assert_eq!(c.answers_only_equal, 5, "every pair but s1 (question 6)");
    for s in run.students.iter().filter(|s| s.remark.is_some()) {
        assert!(s.graded);
        assert_eq!(exact_text(&s.max), "12", "{s:?}");
        assert!(!s.equals_remark());
        assert!(
            s.comparison().contains("maximum differs"),
            "{}",
            s.comparison()
        );
    }
    // Version A: questions 1-6 at 0.75 in Remark; version B: questions 5-12.
    assert_eq!(c.points_differences, 6 + 8);
    assert!(
        run.points_differences
            .iter()
            .all(|(_, _, ours, theirs)| ours == "1" && theirs == "0.75")
    );
    assert_eq!(
        run.maximum_differences,
        vec![
            ("A".to_string(), "12".to_string(), "10.5".to_string()),
            ("B".to_string(), "12".to_string(), "10".to_string()),
        ]
    );
    assert_eq!(c.maximum_differences, 2);
    let html = run.html();
    assert!(html.contains("maximum differs from Remark's total possible"));
    assert!(html.contains("carry other points in this exam than in Remark"));
    let csv = String::from_utf8(run.students_csv().unwrap()).unwrap();
    assert!(csv.contains("maximum differs"));
    // Graded with the imported points, the totals are the exam's and equal Remark's.
    let dir2 = tempfile::tempdir().unwrap();
    let q = graded(dir2.path());
    let run = q
        .compare_remark(&workbooks(), &CompareOptions::default())
        .unwrap();
    assert!(run.points_differences.is_empty() && run.maximum_differences.is_empty());
    assert_eq!(
        (run.counts.totals_settled, run.counts.totals_settled_equal),
        (5, 4)
    );
    assert!(
        run.html()
            .contains("This exam's points equal Remark's for every question")
    );
    p.close().unwrap();
    q.close().unwrap();
}

#[test]
fn settled_differences_queued_for_review_hold_release_and_become_field_labels() {
    // Regression (F2): every difference from an auto-accepted answer must be adjudicated by
    // the instructor before release (§23.7), but the comparison held nothing and the report
    // said otherwise. Queued, each is a Marks-lane MRK-001 item that holds release; each
    // decision is a field label.
    use omr_app::parallel::PARALLEL_SUB;
    use omr_app::review::{Action, Decision, ReviewReason};
    let dir = tempfile::tempdir().unwrap();
    let mut p = graded(dir.path());
    let wbs = workbooks();
    let mut run = p.compare_remark(&wbs, &CompareOptions::default()).unwrap();
    assert_eq!(run.counts.not_queued, 1, "s1's question 6, auto-accepted");
    assert_eq!(run.counts.held_in_review, 0);
    assert!(run.hold_text().contains("does not hold release"));
    assert!(run.html().contains("does not hold release"));
    let blocking = p.session.open_blocking();
    let s1 = p
        .session
        .booklets
        .iter()
        .position(|b| b.student() == Some("2210000002"))
        .unwrap();
    let raw_before = {
        let sc = p.scoring.as_ref().unwrap();
        sc.run.as_ref().unwrap().students[sc.row_of[s1].unwrap()].clone()
    };
    assert_eq!(raw_before.status, omr_scoring::AssessmentStatus::Scored);
    let n = p.queue_parallel_differences(&mut run).unwrap();
    assert_eq!(n, 1);
    assert_eq!(run.counts.held_in_review, 1);
    assert!(
        run.hold_text().contains("wait in Review"),
        "{}",
        run.hold_text()
    );
    assert_eq!(p.session.open_blocking(), blocking + 1);
    let it = p
        .session
        .review
        .items
        .iter()
        .find(|i| i.sub_reason.as_deref() == Some(PARALLEL_SUB))
        .cloned()
        .unwrap();
    assert_eq!(
        (it.code.as_str(), it.booklet, it.question),
        ("MRK-001", Some(s1), Some(5))
    );
    assert!(it.is_open() && it.crop.is_some());
    assert_eq!(it.suggestion, Some(Decision::Option(key(0, 5))));
    let inc = it.incumbent.as_ref().unwrap();
    assert_eq!(inc.decision, Some(Decision::Option((key(0, 5) + 2) % 5)));
    assert!(
        it.detail.contains("Remark Office OMR read"),
        "{}",
        it.detail
    );
    // The booklet's total is provisional until it is decided; release is held.
    {
        let sc = p.scoring.as_ref().unwrap();
        let r = &sc.run.as_ref().unwrap().students[sc.row_of[s1].unwrap()];
        assert_eq!(r.status, omr_scoring::AssessmentStatus::Provisional);
    }
    assert!(p.session.release_blocked().is_some());
    // Queuing again adds nothing.
    let mut again = p.compare_remark(&wbs, &CompareOptions::default()).unwrap();
    assert_eq!(
        (again.counts.not_queued, again.counts.held_in_review),
        (0, 1)
    );
    assert_eq!(p.queue_parallel_differences(&mut again).unwrap(), 0);
    // Choosing Remark's reading overrides the engine: a reason is needed.
    let remark = Decision::Option((key(0, 5) + 2) % 5);
    let resolve = |d: &Decision| Action::Resolve {
        decision: d.clone(),
        accepted_suggestion: false,
        note: String::new(),
    };
    assert!(p.apply(it.id, resolve(&remark)).is_err());
    p.apply_with_reason(
        it.id,
        resolve(&remark),
        Some(ReviewReason::code("AMBIGUOUS_MARK")),
    )
    .unwrap();
    let labels: Vec<serde_json::Value> = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "FieldLabelRecorded")
        .map(|e| e.payload)
        .collect();
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0]["engine_loss"], true);
    assert_eq!(labels[0]["incumbent_loss"], false);
    assert!(
        p.store()
            .unwrap()
            .events()
            .unwrap()
            .iter()
            .any(|e| e.event_type == "ParallelRunCompared")
    );
    {
        let sc = p.scoring.as_ref().unwrap();
        let r = &sc.run.as_ref().unwrap().students[sc.row_of[s1].unwrap()];
        assert_eq!(r.status, omr_scoring::AssessmentStatus::Scored);
        assert_eq!(r.raw, raw_before.raw - omr_scoring::exact(3, 4));
    }
    assert_eq!(p.session.open_blocking(), blocking);
    // Undo re-opens it; keeping this exam's reading is an incumbent loss.
    p.undo().unwrap();
    assert_eq!(p.session.open_blocking(), blocking + 1);
    p.apply(
        it.id,
        Action::Resolve {
            decision: Decision::Acknowledge,
            accepted_suggestion: true,
            note: String::new(),
        },
    )
    .unwrap();
    let last = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .rfind(|e| e.event_type == "FieldLabelRecorded")
        .unwrap()
        .payload;
    assert_eq!(
        (last["engine_loss"].clone(), last["incumbent_loss"].clone()),
        (false.into(), true.into())
    );
    // The project reopens with the same queue (the audit checks pass).
    let root = p.root().unwrap();
    p.close().unwrap();
    let q = ExamProject::open(&root, "principal:test").unwrap();
    assert!(
        q.session
            .review
            .items
            .iter()
            .any(|i| i.sub_reason.as_deref() == Some(PARALLEL_SUB) && !i.is_open())
    );
    q.close().unwrap();
}

#[test]
fn a_queued_parallel_difference_survives_a_later_batch() {
    // The queue is rebuilt when a batch is added: an open parallel-run item must stay open
    // (its answer still waits for review), never vanish with the answer auto-accepted again.
    use omr_app::parallel::PARALLEL_SUB;
    let dir = tempfile::tempdir().unwrap();
    let mut p = graded(dir.path());
    let mut run = p
        .compare_remark(&workbooks(), &CompareOptions::default())
        .unwrap();
    assert_eq!(p.queue_parallel_differences(&mut run).unwrap(), 1);
    let e = iau();
    let m = Marks::new(&answers(0, &[1], &[]), "2210000008", Some(0));
    let img = sheet(&e, &m, 99);
    let f = write(
        &dir.path().join("scans2"),
        "sheet-07.png",
        &common::png(&img),
    );
    p.add_scans(&[f], &options(&dir.path().join("exam"), 2), &|_| {})
        .unwrap();
    assert_eq!(p.session.booklets.len(), 8);
    let it = p
        .session
        .review
        .items
        .iter()
        .find(|i| i.sub_reason.as_deref() == Some(PARALLEL_SUB))
        .expect("carried over");
    assert!(it.is_open() && it.incumbent.is_some());
    let (b, q) = (it.booklet.unwrap(), usize::from(it.question.unwrap()));
    assert_eq!(p.session.booklets[b].student(), Some("2210000002"));
    assert_eq!(
        p.session.booklets[b].questions[q].status,
        omr_app::model::ResponseStatus::Pending
    );
    let run = p
        .compare_remark(&workbooks(), &CompareOptions::default())
        .unwrap();
    assert_eq!((run.counts.not_queued, run.counts.held_in_review), (0, 1));
    p.close().unwrap();
}

#[test]
fn imported_learning_objectives_are_approved_or_replaced_without_grading_again() {
    // Regression (F5): imported rows kept the TP-154 tables in draft with no audited way to
    // approve them or to replace a graded exam's mapping; the import did not say that the
    // versions' objectives differ, nor that every CLO was placed in domain K.
    let imp = import(&workbooks(), &GivenPoints::new()).unwrap();
    assert!(
        imp.notes.iter().any(
            |n| n.contains("learning objectives differ between the versions")
                && n.contains(
                    "Q1: 4.5 points over 6 questions in version A, 4 points over 4 \
                               questions in version B"
                )
        ),
        "{:?}",
        imp.notes
    );
    assert!(imp.notes.iter().any(|n| n.contains("domain K")));
    assert!(
        imp.clo_csv()
            .unwrap()
            .contains("Q1 in version B: 4 points over 4 questions")
    );
    let dir = tempfile::tempdir().unwrap();
    let mut p = graded(dir.path());
    let draft = |p: &ExamProject| p.outcomes().unwrap().b1.draft_watermark;
    assert!(draft(&p), "imported rows are proposed");
    let raw = |p: &ExamProject| p.scoring.as_ref().unwrap().run.as_ref().unwrap().students[0].raw;
    let before = raw(&p);
    assert!(
        p.approve_mappings("  ", "").is_err(),
        "an approver is named"
    );
    let n = p
        .approve_mappings(
            "Dr. Test Coordinator",
            "rows checked against the exam paper",
        )
        .unwrap();
    assert_eq!(
        n,
        p.session
            .definition
            .outcomes
            .as_ref()
            .unwrap()
            .mapping
            .len()
    );
    assert!(!draft(&p), "approved: no draft watermark");
    assert_eq!(raw(&p), before, "nothing graded again");
    let ev: Vec<serde_json::Value> = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "MappingApproved")
        .map(|e| e.payload)
        .collect();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["approver"], "Dr. Test Coordinator");
    assert!(
        p.approve_mappings("Dr. Test Coordinator", "").is_err(),
        "nothing left"
    );
    // A corrected CLO file replaces the mapping: imported rows are proposed again.
    let mut csv = imp.clo_csv().unwrap();
    csv = csv.replace(",K,", ",S,");
    let (clos, rows) = p.replace_outcomes(csv.as_bytes(), "clo.csv").unwrap();
    assert_eq!(clos, 2);
    assert!(rows >= N);
    assert!(draft(&p));
    assert!(
        p.session
            .definition
            .outcomes
            .as_ref()
            .unwrap()
            .clos
            .iter()
            .all(|c| c.domain == omr_app::exam::DomainLetter::S)
    );
    assert!(
        p.store()
            .unwrap()
            .events()
            .unwrap()
            .iter()
            .any(|e| e.event_type == "MappingReplaced")
    );
    // A version the exam does not have is refused naming the fix.
    let bad = imp
        .clo_csv()
        .unwrap()
        .replace(",A,\"imported", ",C,\"imported");
    let e = p
        .replace_outcomes(bad.as_bytes(), "clo.csv")
        .unwrap_err()
        .to_string();
    assert!(e.contains("version C") && e.contains("form column"), "{e}");
    let root = p.root().unwrap();
    p.close().unwrap();
    let q = ExamProject::open(&root, "principal:test").unwrap();
    assert!(q.outcomes().unwrap().b1.draft_watermark);
    q.close().unwrap();
}

#[test]
fn a_perfect_score_without_a_booklet_is_a_possible_key_sheet_never_counted_as_one() {
    // Regression (F10): a row with every answer right under a student-like ID and no booklet
    // (another section's student, or one absent here) was reported as "Remark key sheet graded
    // as a student" and removed from the "without key sheets" statistics, so a real student's
    // perfect score was said to inflate Remark's statistics.
    let dir = tempfile::tempdir().unwrap();
    let p = graded(dir.path());
    let mut books = books();
    let ka: Vec<String> = (0..N).map(|q| letter(key(0, q))).collect();
    books[2].students.push(student("2210000803", "A", &ka));
    let run = p
        .compare_remark(&read_books(&books), &CompareOptions::default())
        .unwrap();
    assert_eq!(run.counts.key_sheet_rows, 1, "only the repeated-digit row");
    assert_eq!(run.counts.possible_key_sheet_rows, 1);
    let other = run
        .key_sheets
        .iter()
        .find(|k| k.workbook == "Sec2_A_Graded.xlsx")
        .expect("listed");
    assert!(other.rows.is_empty());
    assert_eq!(other.possible.len(), 1);
    let wp = other.without_possible.as_ref().expect("both statistics");
    assert_eq!(
        other.without.n, other.with.n,
        "a possible key sheet is not removed"
    );
    assert_eq!(wp.n, other.with.n - 1);
    let csv = String::from_utf8(run.identity_csv().unwrap()).unwrap();
    assert!(csv.contains("possible key sheet or perfect score"));
    assert_eq!(
        csv.matches("Remark key sheet graded as a student").count(),
        1
    );
    assert!(run.html().contains("possible key sheets too"));
    p.close().unwrap();
}

#[test]
fn the_licence_review_is_stated_and_a_refusal_stops_the_comparison() {
    // F11: the Q-08 licence review (legal affairs) was not said to be open, and nothing let a
    // recorded decision stop a comparison.
    use omr_app::parallel::{LicenceReview, compare_under};
    let dir = tempfile::tempdir().unwrap();
    let p = graded(dir.path());
    let wbs = workbooks();
    let run = compare_under(&p, &wbs, &CompareOptions::default(), None).unwrap();
    assert!(
        run.licence.contains("not recorded, so open"),
        "{}",
        run.licence
    );
    assert!(run.html().contains("Licence review (Q-08)"));
    let cleared = LicenceReview {
        q08: "cleared".into(),
        by: "Legal affairs".into(),
        date: "2026-11-30".into(),
    };
    let run = compare_under(&p, &wbs, &CompareOptions::default(), Some(cleared)).unwrap();
    assert!(run.licence.contains("cleared (Legal affairs, 2026-11-30)"));
    let refused = LicenceReview {
        q08: "refused".into(),
        ..LicenceReview::default()
    };
    let e = compare_under(&p, &wbs, &CompareOptions::default(), Some(refused))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("does not permit") && e.contains("licence_review.json"),
        "{e}"
    );
    p.close().unwrap();
}

/// A sheet without a version on an exam whose versions share one key is scored under the
/// shared key and declares no version: the parallel run pairs it by its answers with a row of
/// any version's workbook and reports that workbook's version.
#[test]
fn a_sheet_scored_under_the_shared_key_pairs_with_any_versions_row() {
    let dir = tempfile::tempdir().unwrap();
    let ka: Vec<String> = (0..N).map(|q| letter(key(0, q))).collect();
    // s0 bubbles A, s1 bubbles B, s2 no version; every version has A's key.
    let people: [(&str, Option<u8>, Vec<usize>); 3] = [
        ("2210000001", Some(0), vec![3, 9]),
        ("2210000005", Some(1), vec![6]),
        ("2210000003", None, vec![1, 5, 10]),
    ];
    let e = iau();
    let mut files = Vec::new();
    for (k, (id, v, wrong)) in people.iter().enumerate() {
        let m = Marks::new(&answers(0, wrong, &[]), id, *v);
        files.push(write(
            &dir.path().join("scans"),
            &format!("sheet-{k:02}.png"),
            &common::png(&sheet(&e, &m, 60 + k as u64)),
        ));
    }
    let shared_key = ["A", "B"]
        .map(|f| omr_app::FormKey {
            form: f.into(),
            answers: ka.clone(),
        })
        .to_vec();
    let mut def = legacy_def(
        "Shared",
        N as u16,
        &["A", "B"],
        KeySource::Provided { forms: shared_key },
    );
    def.hold_undeclared_versions = true;
    let p =
        ExamProject::grade(&def, &files, &options(&dir.path().join("exam"), 2), &|_| {}).unwrap();
    let shared = p
        .session
        .booklets
        .iter()
        .position(|b| b.form_source == omr_app::model::FormSource::Shared)
        .expect("s2 is scored under the shared key");
    // Remark: A's workbook holds s0; B's holds s1, and s2 under an ID no booklet has.
    let row = |id: &str, v: &str, wrong: &[usize]| student(id, v, &text(&answers(0, wrong, &[])));
    let book = |students| Book {
        sections: false,
        key: ka.clone(),
        points: vec![1.0; N],
        groups: Vec::new(),
        students,
        footer: true,
    };
    let wbs: Vec<RemarkWorkbook> = [
        (book(vec![row("2210000001", "A", &[3, 9])]), "A_Graded.xlsx"),
        (
            book(vec![
                row("2210000005", "B", &[6]),
                row("2210000093", "B", &[1, 5, 10]),
            ]),
            "B_Graded.xlsx",
        ),
    ]
    .iter()
    .map(|(b, n)| read_workbook(n, &xlsx(b)).unwrap())
    .collect();
    let opts = CompareOptions {
        given_points: [(String::new(), vec![Some("1".to_string()); N])]
            .into_iter()
            .collect(),
        handwritten: Vec::new(),
        crops: false,
    };
    let run = p.compare_remark(&wbs, &opts).unwrap();
    assert_eq!(run.counts.paired_by_answers, 1, "{:#?}", run.students);
    let st = run.students.iter().find(|s| s.booklet == shared).unwrap();
    assert_eq!(st.remark_id, "2210000093", "paired by its answers: {st:?}");
    assert_eq!(st.form, "B", "the version of the workbook it pairs with");
}
