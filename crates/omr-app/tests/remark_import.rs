//! Remark import (spec §3.5 item 3, PAR-09) on synthetic workbooks laid out as Remark Office
//! OMR's graded Excel export (invented IDs and names): keys per version, exact points from the
//! learning-objective groups or solved from the totals, every total checked, refusals that
//! name what to do (underdetermined points, keys that disagree, totals no points reproduce),
//! key-sheet rows, header variants, Excel-typed IDs, `BLANK` versions, the footer, and the
//! key, CLO and version-map files read back by the exam's own readers.

mod remark_common;

use omr_app::exam::{KeyFilePoints, read_clo_csv, read_key_file_full, read_points_file};
use omr_app::remark::{
    GivenPoints, KeySheetKind, PointsSource, RemarkImport, RemarkWorkbook, import, read_workbook,
};
use omr_scoring::{Exact, exact, exact_text};
use remark_common::{Book, Id, Student, letter, xlsx};

/// A deterministic generator (no rand dependency in the fixture).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn key(n: usize, shift: u8) -> Vec<String> {
    (0..n)
        .map(|q| letter(((q * 3 + usize::from(shift)) % 5) as u8))
        .collect()
}

/// `count` students answering `key` with about one wrong answer in `wrong_in`.
fn students(
    count: usize,
    key: &[String],
    version: &str,
    first_id: u64,
    seed: u64,
    wrong_in: u64,
) -> Vec<Student> {
    let mut g = Lcg(seed);
    (0..count)
        .map(|k| {
            let answers: Vec<String> = key
                .iter()
                .map(|a| {
                    let r = g.next();
                    if r.is_multiple_of(wrong_in) {
                        // A wrong letter, or blank now and then.
                        if r.is_multiple_of(7) {
                            String::new()
                        } else {
                            letter(((a.as_bytes()[0] - b'A') + 1 + (r % 4) as u8) % 5)
                        }
                    } else {
                        a.clone()
                    }
                })
                .collect();
            let refs: Vec<&str> = answers.iter().map(String::as_str).collect();
            let mut s = Student::new(&format!("{}", first_id + k as u64), version, &refs);
            s.name = format!("طالب {k}");
            s
        })
        .collect()
}

fn wb(name: &str, b: &Book) -> RemarkWorkbook {
    read_workbook(name, &xlsx(b)).expect("workbook")
}

fn pts(v: &[f64]) -> Vec<Exact> {
    v.iter()
        .map(|p| omr_scoring::exact_from_f64(*p).unwrap())
        .collect()
}

/// Version A: groups Q1 (1-4) 0.75 each, Q1 (5-6) 1 each, Q2 (7-10) 0.75 each; version B:
/// Q1 (1-4) 0.75, Q2 (5-8) 1, Q1 (9-10) 0.75. Two sections of A, one of B.
fn arti_like() -> (Vec<Book>, Vec<f64>, Vec<f64>) {
    let n = 10;
    let pa = vec![0.75, 0.75, 0.75, 0.75, 1.0, 1.0, 0.75, 0.75, 0.75, 0.75];
    let pb = vec![0.75, 0.75, 0.75, 0.75, 1.0, 1.0, 1.0, 1.0, 0.75, 0.75];
    let ka = key(n, 0);
    let kb = key(n, 2);
    let ga = vec![
        ("Q1".to_string(), 1, 4),
        ("Q1".to_string(), 5, 6),
        ("Q2".to_string(), 7, 10),
    ];
    let gb = vec![
        ("Q1".to_string(), 1, 4),
        ("Q2".to_string(), 5, 8),
        ("Q1".to_string(), 9, 10),
    ];
    let mut a1 = students(12, &ka, "A", 2_210_000_100, 11, 4);
    // An Excel-typed ID (a number cell) and a version left blank on the sheet.
    a1[3].id = Id::Number(2_210_000_103.0);
    a1[5].version = "BLANK".into();
    let mut a2 = students(10, &ka, "A", 2_210_000_200, 12, 3);
    // The instructor's key sheet graded as a student.
    let refs: Vec<&str> = ka.iter().map(String::as_str).collect();
    let mut ks = Student::new("0000000000", "A", &refs);
    ks.name = "مفتاح".into();
    a2.insert(0, ks);
    let b1 = students(14, &kb, "B", 2_210_000_300, 13, 4);
    let mk = |key: &Vec<String>, points: &Vec<f64>, groups: &Vec<(String, u16, u16)>, s| Book {
        sections: true,
        key: key.clone(),
        points: points.clone(),
        groups: groups.clone(),
        students: s,
        footer: true,
    };
    (
        vec![
            mk(&ka, &pa, &ga, a1),
            mk(&ka, &pa, &ga, a2),
            mk(&kb, &pb, &gb, b1),
        ],
        pa,
        pb,
    )
}

fn import_books(books: &[Book], given: &GivenPoints) -> Result<RemarkImport, String> {
    let wbs: Vec<RemarkWorkbook> = books
        .iter()
        .enumerate()
        .map(|(k, b)| wb(&format!("Section_{k}_Graded.xlsx"), b))
        .collect();
    import(&wbs, given).map_err(|e| e.to_string())
}

#[test]
fn learning_objective_groups_give_the_points_and_proposed_clo_mappings() {
    let (books, pa, pb) = arti_like();
    let imp = import_books(&books, &GivenPoints::new()).expect("import");
    assert_eq!(imp.forms(), vec!["A", "B"]);
    assert_eq!(imp.questions, 10);
    let a = imp.version("A").unwrap();
    let b = imp.version("B").unwrap();
    assert_eq!(a.points, pts(&pa));
    assert_eq!(b.points, pts(&pb));
    assert_eq!(a.points_source, PointsSource::LearningObjectives);
    assert_eq!(b.points_source, PointsSource::LearningObjectives);
    assert_eq!(a.workbooks.len(), 2, "two sections of version A merged");
    // Every row checked (key sheet included), every group's points too.
    assert_eq!((a.rows_checked, a.totals_reproduced), (23, 23));
    assert_eq!(a.group_points_checked, 23 * 3);
    assert_eq!(a.students, 22);
    assert_eq!(b.students, 14);
    assert_eq!(a.total_possible, exact(8, 1));
    assert_eq!(b.total_possible, exact(17, 2));
    // The key sheet: reported, never a student.
    assert_eq!(imp.key_sheet_rows.len(), 1);
    let k = &imp.key_sheet_rows[0];
    assert_eq!(k.kind, KeySheetKind::RepeatedDigitId);
    assert!(k.answers_equal_key && k.full_marks);
    // The summary: counts, no full student number, no name, no key letter run.
    let s = imp.summary();
    assert!(!s.contains("2210000"), "{s}");
    assert!(!s.contains("طالب") && !s.contains("مفتاح"), "{s}");
    assert!(s.contains("key sheets graded as students"), "{s}");
    // key.csv read back by the exam's reader: per-version keys and points.
    let kf = read_key_file_full(imp.key_csv().as_bytes(), &["A".into(), "B".into()], 10, 5)
        .expect("key.csv");
    assert_eq!(kf.forms[0].answers, books[0].key);
    assert_eq!(kf.forms[1].answers, books[2].key);
    assert_eq!(kf.points.all, None);
    let text = |v: &[f64]| -> Vec<String> { pts(v).iter().map(exact_text).collect() };
    assert_eq!(kf.points.per_form["A"], text(&pa));
    assert_eq!(kf.points.per_form["B"], text(&pb));
    // clo.csv read back: the groups as weight-1 mappings per version, imported (proposed).
    let clo = imp.clo_csv().expect("learning objectives");
    assert!(clo.contains("imported, needs review"), "{clo}");
    let o = read_clo_csv(clo.as_bytes(), 10).expect("clo.csv");
    assert_eq!(
        o.clos.iter().map(|c| c.code.as_str()).collect::<Vec<_>>(),
        vec!["Q1", "Q2"]
    );
    assert_eq!(o.mapping.len(), 20, "10 questions × 2 versions");
    assert!(
        o.mapping
            .iter()
            .all(|m| m.weight == "1" && m.imported == "Remark import")
    );
    let b6 = o
        .mapping
        .iter()
        .find(|m| m.form == "B" && m.question == 6)
        .unwrap();
    assert_eq!(b6.clo, "Q2");
    let a6 = o
        .mapping
        .iter()
        .find(|m| m.form == "A" && m.question == 6)
        .unwrap();
    assert_eq!(a6.clo, "Q1");
    let key_of = |m: &omr_app::CloMapping| (m.form.clone(), m.question, m.clo.clone());
    let mut x: Vec<_> = imp
        .outcome_setup()
        .unwrap()
        .mapping
        .iter()
        .map(key_of)
        .collect();
    let mut y: Vec<_> = o.mapping.iter().map(key_of).collect();
    x.sort();
    y.sort();
    assert_eq!(x, y);
    // The version map: one line per workbook.
    let vm = imp.version_map_csv();
    assert_eq!(vm.lines().count(), 4, "{vm}");
    assert!(vm.contains("Section_0_Graded.xlsx") && vm.contains("learning-objective"));
}

#[test]
fn points_are_solved_exactly_from_the_totals_without_groups() {
    // The CS-like layout: no Section No / Gender, no learning objectives; 1/2 and 3/4 points.
    let n = 9;
    let p = vec![0.5, 0.75, 0.5, 0.75, 0.75, 0.5, 0.5, 0.75, 0.5];
    let k = key(n, 1);
    let mut s = students(30, &k, "A", 2_220_000_000, 5, 3);
    s[0].id = Id::Number(2_220_000_000.0);
    let book = Book {
        sections: false,
        key: k.clone(),
        points: p.clone(),
        groups: Vec::new(),
        students: s,
        footer: true,
    };
    let imp = import_books(std::slice::from_ref(&book), &GivenPoints::new()).expect("import");
    let a = imp.version("A").unwrap();
    assert_eq!(a.points, pts(&p));
    assert_eq!(a.points_source, PointsSource::SolvedFromTotals);
    assert_eq!((a.totals_reproduced, a.rows_checked), (30, 30));
    assert!(imp.clo_csv().is_none());
    // One version: one points column.
    let kf = read_key_file_full(imp.key_csv().as_bytes(), &["A".into()], 9, 5).unwrap();
    assert_eq!(
        kf.points,
        KeyFilePoints {
            all: Some(pts(&p).iter().map(exact_text).collect()),
            per_form: Default::default(),
        }
    );
    assert!(!imp.workbooks[0].has_section && !imp.workbooks[0].has_gender);
    assert_eq!(imp.workbooks[0].footer_rows, 1);
}

#[test]
fn points_the_totals_cannot_tell_apart_are_refused_naming_the_questions() {
    let n = 7;
    let p = vec![0.5, 0.75, 1.0, 0.5, 0.75, 1.0, 0.5];
    let k = key(n, 3);
    let mut s = students(25, &k, "A", 2_230_000_000, 9, 3);
    // Every student answers questions 2 and 6 correctly.
    for st in &mut s {
        st.answers[1] = k[1].clone();
        st.answers[5] = k[5].clone();
    }
    let book = Book {
        sections: true,
        key: k,
        points: p.clone(),
        groups: Vec::new(),
        students: s,
        footer: false,
    };
    let e = import_books(std::slice::from_ref(&book), &GivenPoints::new()).unwrap_err();
    assert!(
        e.contains("do not determine the points of questions 2, 6"),
        "{e}"
    );
    assert!(
        e.contains("every student answered question 2 correctly") && e.contains("--points FILE"),
        "{e}"
    );
    // The instructor gives one of them: the other follows from the totals.
    let given = read_points_file(b"question,points\r\n2,0.75\r\n", 7).unwrap();
    let imp = import_books(std::slice::from_ref(&book), &given).expect("with given points");
    let a = imp.version("A").unwrap();
    assert_eq!(a.points, pts(&p));
    assert_eq!(a.points_source, PointsSource::Mixed);
    // Points that contradict the totals are refused.
    let wrong = read_points_file(b"question,points_A\n2,1\n6,1\n", 7).unwrap();
    let e = import_books(std::slice::from_ref(&book), &wrong).unwrap_err();
    assert!(e.contains("reproduce"), "{e}");
}

#[test]
fn a_version_determined_elsewhere_suggests_the_points_of_the_other() {
    // Version B carries version A's points in another order and is determined; in A two
    // questions are answered right by everyone.
    let n = 6;
    let pa = vec![0.5, 0.75, 0.75, 0.5, 0.75, 0.5];
    let pb = vec![0.75, 0.5, 0.5, 0.75, 0.5, 0.75];
    let (ka, kb) = (key(n, 0), key(n, 4));
    let mut sa = students(20, &ka, "A", 2_240_000_000, 21, 3);
    for st in &mut sa {
        st.answers[1] = ka[1].clone();
        st.answers[2] = ka[2].clone();
    }
    let sb = students(20, &kb, "B", 2_240_000_100, 22, 3);
    let books = [
        Book {
            sections: false,
            key: ka,
            points: pa,
            groups: Vec::new(),
            students: sa,
            footer: true,
        },
        Book {
            sections: false,
            key: kb,
            points: pb,
            groups: Vec::new(),
            students: sb,
            footer: true,
        },
    ];
    let e = import_books(&books, &GivenPoints::new()).unwrap_err();
    assert!(
        e.contains("version A") && e.contains("questions 2, 3"),
        "{e}"
    );
    assert!(
        e.contains("0.75 each") && e.contains("`2,0.75`, `3,0.75`"),
        "{e}"
    );
    // The suggestion as points the instructor gives: the import then reproduces every total.
    let wbs: Vec<RemarkWorkbook> = books
        .iter()
        .enumerate()
        .map(|(k, b)| wb(&format!("Section_{k}_Graded.xlsx"), b))
        .collect();
    let s = omr_app::remark::suggest_points(&wbs, &GivenPoints::new()).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s["A"][1].as_deref(), Some("0.75"));
    assert_eq!(s["A"][2].as_deref(), Some("0.75"));
    assert_eq!(s["A"].iter().flatten().count(), 2);
    let imp = import(&wbs, &s).unwrap();
    let a = imp.version("A").unwrap();
    assert_eq!(a.points_source, PointsSource::Mixed);
    assert_eq!(a.totals_reproduced, a.rows_checked);
}

#[test]
fn workbooks_of_one_version_that_disagree_are_refused_naming_the_files() {
    let (mut books, ..) = arti_like();
    books[1].key[2] = letter(((books[1].key[2].as_bytes()[0] - b'A') + 1) % 5);
    let e = import_books(&books, &GivenPoints::new()).unwrap_err();
    assert!(
        e.contains("Section_0_Graded.xlsx")
            && e.contains("Section_1_Graded.xlsx")
            && e.contains("questions 3"),
        "{e}"
    );
    // Different learning objectives within a version.
    let (mut books, ..) = arti_like();
    books[1].groups[0].0 = "LO9".into();
    let e = import_books(&books, &GivenPoints::new()).unwrap_err();
    assert!(
        e.contains("learning objectives") && e.contains("Section_1"),
        "{e}"
    );
}

#[test]
fn a_total_no_points_reproduce_is_refused_naming_the_row() {
    let n = 8;
    let p = vec![0.5, 0.75, 0.5, 0.75, 0.5, 0.75, 0.5, 0.75];
    let k = key(n, 2);
    let mut s = students(30, &k, "A", 2_250_000_000, 31, 3);
    s[4].total_override = Some(4.25); // a hand-edited total
    let book = Book {
        sections: false,
        key: k,
        points: p,
        groups: Vec::new(),
        students: s,
        footer: true,
    };
    let e = import_books(std::slice::from_ref(&book), &GivenPoints::new()).unwrap_err();
    assert!(e.contains("no points per question reproduce"), "{e}");
    // Worksheet row 12 holds the fifth student (header on row 6, key on row 7).
    assert!(e.contains("row 12"), "{e}");
}

#[test]
fn header_variants_typed_ids_blank_cells_and_the_footer_are_read() {
    let k = key(5, 0);
    let mut s = students(6, &k, "B", 2_260_000_000, 41, 2);
    s[1].id = Id::Number(2_260_000_001.0);
    s[2].id = Id::Text("2260000002.0".into());
    s[3].id = Id::Empty;
    s[4].answers[0] = String::new();
    let mut book = Book {
        sections: false,
        key: k,
        points: vec![1.0; 5],
        groups: Vec::new(),
        students: s,
        footer: true,
    };
    let w = wb("cs.xlsx", &book);
    assert_eq!(w.rows.len(), 6);
    assert_eq!(w.footer_rows, 1);
    assert_eq!(w.rows[1].student_id, "2260000001");
    assert!(w.rows[1].numeric_id);
    assert_eq!(w.rows[2].student_id, "2260000002");
    assert_eq!(w.rows[3].student_id, "");
    assert_eq!(w.rows[4].answers[0], omr_app::remark::RemarkAnswer::Blank);
    assert!(!w.has_section && !w.has_gender && w.has_version);
    // With the Section No and Gender columns.
    book.sections = true;
    let w2 = wb("arti.xlsx", &book);
    assert!(w2.has_section && w2.has_gender);
    assert_eq!(w2.rows[0].section, "1001");
    assert_eq!(
        w.rows.iter().map(|r| r.total).collect::<Vec<_>>(),
        w2.rows.iter().map(|r| r.total).collect::<Vec<_>>()
    );
    // Not a Remark export: refused with what to do.
    let e = read_workbook("x.xlsx", b"not a workbook")
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Excel .xlsx") && e.contains("Advanced Grade"),
        "{e}"
    );
    let e = omr_app::remark::read_workbook_files(&["Remark_OMR_Key_A.aky".into()])
        .unwrap_err()
        .to_string();
    assert!(e.contains("never opened"), "{e}");
}

#[test]
fn a_key_file_with_points_per_version_scores_each_version_with_its_own() {
    let (books, pa, pb) = arti_like();
    let imp = import_books(&books, &GivenPoints::new()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let files = imp.write(&dir.path().join("import")).unwrap();
    // key.csv, clo.csv, version_map.csv and the signed manifest over them (F13)
    assert_eq!(files.len(), 4);
    assert!(files.iter().any(|f| f.ends_with("manifest.json")));
    let mut i =
        omr_app::define::Inputs::new(omr_app::define::Sheet::Template("iau-ccsit-v1".into()), 10);
    i.key = Some(dir.path().join("import/key.csv"));
    i.clo = Some(dir.path().join("import/clo.csv"));
    let (def, _) = omr_app::define::define(&i).unwrap();
    assert_eq!(def.forms, vec!["A", "B"]);
    assert_eq!(def.form_points.len(), 2);
    let key = omr_app::pipeline::provided_key(
        &def,
        match &def.key {
            omr_app::KeySource::Provided { forms } => forms,
            _ => unreachable!(),
        },
        1,
    )
    .unwrap();
    assert!(
        !key.shared_items,
        "versions with other points never share items"
    );
    let fm = omr_app::score::form_map(&def, &key).unwrap();
    let got: Vec<Exact> = fm
        .items
        .iter()
        .map(|i| omr_scoring::widen(&i.max_points))
        .collect();
    let mut want = pts(&pa);
    want.extend(pts(&pb));
    assert_eq!(got, want);
    // The imported CLO rows map each version's own printed questions, proposed.
    let o = def.outcomes.as_ref().unwrap();
    let ms = omr_app::outcomes::mapping_set(&def, o, Some(&key), fm.items.len());
    assert_eq!(ms.rows.len(), 20);
    assert!(
        ms.rows
            .iter()
            .all(|r| r.status == omr_outcomes::MappingStatus::Proposed
                && r.source == omr_outcomes::MappingSource::RemarkImport)
    );
    // Version B's question 6 (master item 10 + 5) evidences Q2; version A's Q1.
    assert!(ms.rows.iter().any(|r| r.item == 15 && r.clo == "Q2"));
    assert!(ms.rows.iter().any(|r| r.item == 5 && r.clo == "Q1"));
}

#[test]
fn points_files_take_one_column_or_one_per_version() {
    let all = read_points_file(b"question,points\n1,0.5\n3,\n", 3).unwrap();
    assert_eq!(all[""], vec![Some("0.5".into()), None, None]);
    let per = read_points_file(
        "\u{feff}Q;points_A;points B\r\n1;0,75;1\r\n2;;0.5\r\n".as_bytes(),
        2,
    )
    .unwrap();
    assert_eq!(per["A"], vec![Some("0.75".into()), None]);
    assert_eq!(per["B"], vec![Some("1".into()), Some("0.5".into())]);
    // A key file with points works as it is.
    let k = read_points_file(b"question,A,B,points\n1,B,C,2\n", 1).unwrap();
    assert_eq!(k[""], vec![Some("2".into())]);
    assert!(read_points_file(b"question,A\n1,B\n", 1).is_err());
    assert!(read_points_file(b"question,points\n1,x\n", 1).is_err());
}

#[test]
fn groups_whose_points_are_not_split_equally_are_solved_with_their_points() {
    // Group Q1 (1-4) carries 3 points as 0.5, 1, 0.5, 1: the equal split (0.75) reproduces no
    // total, so the points are solved from the totals and each student's group points.
    let n = 8;
    let p = vec![0.5, 1.0, 0.5, 1.0, 0.75, 0.75, 0.75, 0.75];
    let k = key(n, 1);
    let book = Book {
        sections: true,
        key: k.clone(),
        points: p.clone(),
        groups: vec![("Q1".to_string(), 1, 4), ("Q2".to_string(), 5, 8)],
        students: students(24, &k, "A", 2_270_000_000, 51, 3),
        footer: true,
    };
    let imp = import_books(std::slice::from_ref(&book), &GivenPoints::new()).expect("import");
    let a = imp.version("A").unwrap();
    assert_eq!(a.points, pts(&p));
    assert_eq!(a.points_source, PointsSource::SolvedFromTotals);
    assert_eq!(a.totals_reproduced, a.rows_checked);
    assert!(
        imp.notes.iter().any(|n| n.contains("not split equally")),
        "{:?}",
        imp.notes
    );
}

#[test]
fn damaged_workbooks_are_refused_never_a_panic() {
    // A small mutation fuzz of a valid export (§16.3: every importer is fuzzed): flipped bytes,
    // truncation; each read is an import or a refusal, never a panic.
    let (books, ..) = arti_like();
    let good = xlsx(&books[0]);
    let mut g = Lcg(99);
    let mut refused = 0usize;
    for k in 0..300 {
        let mut b = good.clone();
        if k % 3 == 0 {
            b.truncate((g.next() as usize) % b.len());
        } else {
            for _ in 0..(1 + g.next() % 8) {
                let i = (g.next() as usize) % b.len();
                b[i] ^= (g.next() % 255 + 1) as u8;
            }
        }
        let r = std::panic::catch_unwind(|| read_workbook("damaged.xlsx", &b));
        let r = r.expect("no panic on a damaged workbook");
        refused += usize::from(r.is_err());
    }
    assert!(refused > 0);
}

/// Verification round (spec §3.5): rows the importer must read, and exports it must refuse.
fn plain(n: usize, students: Vec<Student>, points: Vec<f64>) -> Book {
    Book {
        sections: false,
        key: key(n, 1),
        points,
        groups: Vec::new(),
        students,
        footer: true,
    }
}

#[test]
fn a_student_row_whose_id_remark_could_not_read_does_not_end_the_rows() {
    // Regression: the first row whose ID cell had no digit was taken for the footer and every
    // row below it was skipped, so a student whose ID Remark wrote as a word (`ERROR`,
    // `BLANK`, an Excel `#N/A`) silently dropped the rest of the class.
    let n = 8;
    let k = key(n, 1);
    let mut s = students(20, &k, "A", 2_250_000_000, 31, 3);
    s[4].id = Id::Text("ERROR".into());
    s[9].id = Id::Text("#N/A".into());
    // A sheet left blank with no ID: no answers, a total of 0.
    s[13].id = Id::Text("BLANK".into());
    s[13].answers = vec![String::new(); n];
    let book = plain(n, s, vec![1.0; n]);
    let imp = import_books(std::slice::from_ref(&book), &GivenPoints::new()).expect("import");
    // Every row read (a student with every answer right counts as a possible key sheet).
    let w = &imp.workbooks[0];
    assert_eq!(w.students + w.key_sheets, 20, "{}", imp.summary());
    assert_eq!(imp.workbooks[0].footer_rows, 1, "the Mean row only");
    assert_eq!(imp.version("A").unwrap().rows_checked, 20);
    // A second Answer Key row: the report holds several tests; refused.
    let mut s = students(6, &k, "A", 2_250_000_100, 32, 3);
    s[3].id = Id::Text("Answer Key".into());
    let e = read_workbook("Two.xlsx", &xlsx(&plain(n, s, vec![1.0; n])))
        .unwrap_err()
        .to_string();
    assert!(e.contains("row 11: a second `Answer Key` row"), "{e}");
}

#[test]
fn a_hundred_question_export_is_solved_from_the_totals() {
    // Regression: the exact solver overflowed i128 fractions from about 60 questions, so an
    // export without learning objectives could not be imported.
    let n = 100;
    let p: Vec<f64> = (0..n).map(|q| [0.5, 0.75, 1.0][q % 3]).collect();
    let k = key(n, 2);
    let s = students(220, &k, "A", 2_260_000_000, 41, 3);
    let imp = import_books(&[plain(n, s, p.clone())], &GivenPoints::new()).expect("import");
    let a = imp.version("A").unwrap();
    assert_eq!(a.points, pts(&p));
    assert_eq!(a.points_source, PointsSource::SolvedFromTotals);
    assert_eq!((a.totals_reproduced, a.rows_checked), (220, 220));
}

#[test]
fn learning_objectives_of_another_test_or_with_gaps_are_named() {
    let (books, ..) = arti_like();
    // A group naming question 40 of a 10-question test: the reports are not of one test.
    let bytes = remark_common::xlsx_relabelled(&books[0], &|l| l.replace("(7-10)", "(7-40)"));
    let e = read_workbook("Mixed.xlsx", &bytes).unwrap_err().to_string();
    assert!(
        e.contains("Q2 (7-40)") && e.contains("not of one test"),
        "{e}"
    );
    // Questions in no objective, and questions in two: imported, and said.
    let n = 10;
    let k = key(n, 0);
    let mk = |groups: Vec<(String, u16, u16)>| Book {
        sections: true,
        key: k.clone(),
        points: vec![1.0; n],
        groups,
        students: students(16, &k, "A", 2_270_000_000, 51, 3),
        footer: true,
    };
    let gap = mk(vec![("Q1".into(), 1, 4), ("Q2".into(), 7, 10)]);
    let imp = import_books(&[gap], &GivenPoints::new()).expect("gap");
    assert_eq!(imp.version("A").unwrap().points, pts(&[1.0; 10]));
    assert!(
        imp.notes
            .iter()
            .any(|x| x.contains("questions 5, 6 are in no learning objective")),
        "{:?}",
        imp.notes
    );
    let overlap = mk(vec![("Q1".into(), 1, 6), ("Q2".into(), 5, 10)]);
    let imp = import_books(&[overlap], &GivenPoints::new()).expect("overlap");
    assert_eq!(imp.version("A").unwrap().points, pts(&[1.0; 10]));
    assert!(
        imp.notes
            .iter()
            .any(|x| x.contains("questions 5, 6 are in more than one learning objective")),
        "{:?}",
        imp.notes
    );
    let o = imp.outcome_setup().unwrap();
    assert_eq!(o.mapping.iter().filter(|m| m.question == 5).count(), 2);
}

#[test]
fn an_id_excel_turned_into_scientific_notation_is_said() {
    let n = 6;
    let k = key(n, 1);
    let mut s = students(12, &k, "A", 2_280_000_000, 61, 3);
    s[2].id = Id::Text("2.28E+09".into());
    // An ID Excel kept every digit of (`2.280000003E+09`) is the student number.
    s[3].id = Id::Text("2.280000003E+09".into());
    let imp = import_books(&[plain(n, s, vec![1.0; n])], &GivenPoints::new()).expect("import");
    assert!(
        imp.notes
            .iter()
            .any(|x| x.starts_with("1 row(s) have a student ID in scientific notation")),
        "{:?}",
        imp.notes
    );
    let w = wb(
        "Sci.xlsx",
        &plain(n, students(4, &k, "A", 2_280_000_000, 61, 3), vec![1.0; n]),
    );
    assert_eq!(w.rows[3].student_id, "2280000003");
}

#[test]
fn a_stray_value_at_the_far_corner_is_refused_not_an_abort() {
    // Regression: the sheet was read as a dense grid, so one value at XFD1048576 made the
    // reader allocate 2^34 cells (549 GB) and the process aborted: an 8 KB workbook killed the
    // desktop app. The same for a roster workbook.
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Student Response Report").unwrap();
    ws.write_string(5, 0, "Student ID").unwrap();
    ws.write_string(1_048_575, 16_383, "x").unwrap();
    let bytes = wb.save_to_buffer().unwrap();
    let e = read_workbook("Corner.xlsx", &bytes)
        .unwrap_err()
        .to_string();
    assert!(e.contains("cell XFD1048576"), "{e}");
    let e = omr_app::exam::read_roster_xlsx(&bytes)
        .unwrap_err()
        .to_string();
    assert!(e.contains("cell XFD1048576"), "{e}");
    // An empty formatted cell there is no value: the export reads as usual.
    let (books, ..) = arti_like();
    let fine = xlsx(&books[0]);
    assert!(read_workbook("Fine.xlsx", &fine).is_ok());
}

#[test]
fn a_workbook_whose_parts_inflate_beyond_any_export_is_refused() {
    // A ZIP whose directory declares 600 MB of decompressed parts (a decompression bomb) is
    // refused before it is opened.
    let (books, ..) = arti_like();
    let mut bytes = xlsx(&books[0]);
    // Patch the first central-directory entry's uncompressed size.
    let cd = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    bytes[cd + 24..cd + 28].copy_from_slice(&(600u32 << 20).to_le_bytes());
    let e = read_workbook("Bomb.xlsx", &bytes).unwrap_err().to_string();
    assert!(e.contains("once decompressed"), "{e}");
}

#[test]
fn a_workbook_given_twice_is_refused() {
    // Its rows would count twice: every student a duplicate, every total checked twice.
    let (books, ..) = arti_like();
    let w = wb("Sec1_A.xlsx", &books[0]);
    let mut again = w.clone();
    again.file = "Sec1_A (copy).xlsx".into();
    let e = import(&[w, again], &GivenPoints::new())
        .unwrap_err()
        .to_string();
    assert!(e.contains("are the same workbook"), "{e}");
}

#[test]
fn a_learning_objective_split_the_totals_cannot_pin_down_is_refused_not_guessed() {
    // Regression (F4): a group's points were split equally over its questions and only checked
    // against the totals. Questions 1 and 2 of `LO1 (1-2)` carry 1 and 0.5 points, and every
    // student answered both right or both wrong: any split of the group's 1.5 fits, and the
    // import wrote 0.75 each as "from the learning-objective groups". It is refused now, as
    // the totals' own gaps are, naming the group and the equal split as a hint only.
    let n = 8;
    let p = vec![1.0, 0.5, 0.75, 0.75, 0.75, 0.75, 0.75, 0.75];
    let k = key(n, 3);
    let mut st = students(30, &k, "A", 2_280_000_000, 61, 3);
    for s in &mut st {
        s.answers[1] = if s.answers[0] == k[0] {
            k[1].clone()
        } else {
            letter(((k[1].as_bytes()[0] - b'A') + 1) % 5)
        };
    }
    let book = Book {
        sections: true,
        key: k.clone(),
        points: p.clone(),
        groups: vec![("LO1".to_string(), 1, 2), ("LO2".to_string(), 3, 8)],
        students: st,
        footer: true,
    };
    let e = import_books(std::slice::from_ref(&book), &GivenPoints::new()).unwrap_err();
    assert!(
        e.contains("do not determine the points of questions 1, 2"),
        "{e}"
    );
    assert!(
        e.contains("`LO1 (1-2)` gives 1.5 points") && e.contains("0.75 each"),
        "{e}"
    );
    assert!(e.contains("--points"), "{e}");
    // Given by the instructor, the points are taken and checked against every total.
    let given = read_points_file(b"question,points\n1,1\n2,0.5\n", n as u16).unwrap();
    let imp = import_books(std::slice::from_ref(&book), &given).expect("import");
    let a = imp.version("A").unwrap();
    assert_eq!(a.points, pts(&p));
    assert_eq!(a.totals_reproduced, a.rows_checked);
    // Questions the students tell apart: the equal split is the only one, and is taken.
    let mut free = book.clone();
    free.points = vec![0.75; n];
    free.students = students(30, &k, "A", 2_280_000_100, 62, 3);
    let imp = import_books(&[free], &GivenPoints::new()).expect("import");
    let a = imp.version("A").unwrap();
    assert_eq!(a.points_source, PointsSource::LearningObjectives);
    assert_eq!(a.points, pts(&[0.75; 8]));
}
