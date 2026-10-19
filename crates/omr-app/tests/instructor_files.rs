//! Answer-key and roster files as instructors and SIS exports produce them (C5, C6; synthetic
//! values only): encodings (UTF-8 with or without BOM, UTF-16, Windows-1256), CRLF, comma,
//! semicolon or tab separators, headers or none, one row per question or per version, lowercase
//! and spaced entries, several accepted answers, voids, Arabic-Indic digits and Arabic headers;
//! roster IDs damaged by Excel are repaired when nothing was lost and refused otherwise, and
//! every refusal says what to change (R10).

use omr_app::exam::{self, FormKey, read_key_file, read_roster_csv, read_roster_xlsx};
use omr_app::{AppError, ExamDefinition, IdSchemeChoice, KeySource, TemplateChoice};
use omr_model::CancelMode;

fn forms(f: &[&str]) -> Vec<String> {
    f.iter().map(|s| (*s).to_string()).collect()
}

fn keys(bytes: &[u8], f: &[&str], n: u16) -> Vec<FormKey> {
    read_key_file(bytes, &forms(f), n, 5)
        .unwrap_or_else(|e| panic!("{e}"))
        .0
}

fn answers(k: &[FormKey]) -> Vec<Vec<String>> {
    k.iter().map(|f| f.answers.clone()).collect()
}

fn entries(k: &[FormKey], f: usize) -> Vec<exam::ParsedKey> {
    k[f].answers
        .iter()
        .map(|a| exam::parse_key_entry(a, 5).unwrap())
        .collect()
}

fn err(r: Result<impl std::fmt::Debug, AppError>) -> String {
    let e = r.expect_err("refused");
    assert_eq!(e.exit_code(), 3, "{e}");
    e.to_string()
}

#[test]
fn c5_key_files_in_every_common_layout_read_the_same_key() {
    use exam::ParsedKey::{Missing, Options, Void};
    let want_a = vec![Options(vec![1]), Options(vec![0, 3]), Void];
    let want_b = vec![Options(vec![2]), Options(vec![4]), Options(vec![0])];
    let two = ["A", "B"];
    let check = |what: &str, bytes: &[u8]| {
        let k = keys(bytes, &two, 3);
        assert_eq!(entries(&k, 0), want_a, "{what}");
        assert_eq!(entries(&k, 1), want_b, "{what}");
    };
    // One row per question with a header: BOM, CRLF, lowercase, spaces, Q-prefixed numbers.
    check(
        "long, header, BOM, CRLF",
        "\u{feff}Question,Version A,Version B\r\nQ1, b ,c\r\n2.,a|d,e\r\n3),*,a\r\n".as_bytes(),
    );
    // Headerless, one column per form in the exam's order.
    check("long, no header", b"1,B,C\n2,A/D,E\n3,void,A\n");
    // One row per version: header, or none (first cell = form letter), or unlabeled rows.
    check("wide, header", b"version,Q1,Q2,Q3\nB,C,E,A\nA,B,A D,*\n");
    check("wide, labelled", b"A,B,AD,*\nB,C,E,A\n");
    check("wide, unlabeled", b"B,A or D,VOID\nC,E,A\n");
    // Semicolon (Excel with a decimal comma) and tab separators; quoted multi-keys.
    check("semicolon", b"question;A;B\n1;B;C\n2;\"A,D\";E\n3;*;A\n");
    // UTF-16 (Excel "Unicode text", tab separated) with Arabic headers and Arabic-Indic digits.
    let text = "السؤال\tنموذج أ\tنموذج ب\r\n١\tب\tج\r\n٢\tأ|د\tهـ\r\n٣\tملغى\tا\r\n";
    let mut utf16 = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    check("UTF-16 Arabic", &utf16);
    // Windows-1256 (Saudi ANSI code page; it has no Arabic-Indic digits), saved by Notepad.
    let comma = text
        .replace('\t', ",")
        .replace('١', "1")
        .replace('٢', "2")
        .replace('٣', "3");
    let (cp1256, _, bad) = encoding_rs::WINDOWS_1256.encode(&comma);
    assert!(!bad);
    check("Windows-1256", &cp1256);
    // F6: `+` and `&` are Remark's AND syntax (all of these answers), never read as "either":
    // refused naming the line and what to write.
    for and in ["A+D", "A&D", "A and D"] {
        let e = err(read_key_file(
            format!("question,A,B\n1,B,C\n2,{and},E\n3,*,A\n").as_bytes(),
            &forms(&two),
            3,
            5,
        ));
        assert!(
            e.contains("line 3") && e.contains("AND key") && e.contains("A|C"),
            "{e}"
        );
    }
    // One key for every form; a single column of answers; points; rows beyond the exam.
    let k = keys(
        b"Question,Answer,Points\n1,b,2\n2,c,\n3,d,0.5\n4,e,1\n",
        &two,
        3,
    );
    assert_eq!(answers(&k), vec![vec!["b", "c", "d"]; 2]);
    let (_, p) = read_key_file(
        b"question;key;points\n1;B;2\n2;C;0,5\n3;D;1\n",
        &forms(&two),
        3,
        5,
    )
    .unwrap();
    assert_eq!(p.unwrap(), vec!["2", "0.5", "1"]);
    // An empty line between answers is a question without an entry (an empty cell).
    let k = keys(b"B\nc\n\n D\n", &["A"], 4);
    assert_eq!(
        entries(&k, 0),
        vec![
            Options(vec![1]),
            Options(vec![2]),
            Missing,
            Options(vec![3])
        ]
    );
    // A question left out is missing (SCO-003 when scored), never a guess.
    let k = keys(b"question,A\n1,B\n3,C\n", &["A"], 3);
    assert_eq!(entries(&k, 0)[1], Missing);
    // The forms a file names.
    assert_eq!(
        exam::key_file_forms(b"question,Form A,form b,points\n1,A,B,1\n"),
        Some(forms(&two))
    );
}

#[test]
fn c5_malformed_key_files_fail_with_the_line_and_what_to_change() {
    let two = forms(&["A", "B"]);
    let m = err(read_key_file(b"question,A,B\n1,B,C\n2,F,A\n", &two, 2, 5));
    assert!(
        m.contains("line 3") && m.contains("`F`") && m.contains("A|C"),
        "{m}"
    );
    let m = err(read_key_file(b"question,A,B\nx,B,C\n", &two, 2, 5));
    assert!(
        m.contains("line 2") && m.contains("not a question number"),
        "{m}"
    );
    let m = err(read_key_file(b"question,A,B\n1,B,C\n1,D,C\n", &two, 2, 5));
    assert!(m.contains("question 1 is listed again"), "{m}");
    let m = err(read_key_file(b"question,A,C\n1,B,C\n", &two, 1, 5));
    assert!(
        m.contains("no key for form B") && m.contains("--forms"),
        "{m}"
    );
    let m = err(read_key_file(b"", &two, 2, 5));
    assert!(m.contains("accepted layouts"), "{m}");
    let m = err(read_key_file(b"question,A,B\n1,B,C,D,E\n", &two, 1, 5));
    assert!(m.contains("question 1"), "{m}");
    // Options beyond the exam's count are refused by the definition, with the remedy.
    let mut d = ExamDefinition::new(
        "Quiz",
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        2,
        4,
        two.clone(),
        CancelMode::EraseOnly,
    );
    let (k, _) = exam::read_key_csv(b"question,A,B\n1,E,A\n2,B,C\n", &two, 2).unwrap();
    d.key = KeySource::Provided { forms: k };
    let e = d.validate().unwrap_err();
    assert!(
        e.to_string().contains("question 1") && e.to_string().contains("* to void"),
        "{e}"
    );
}

#[test]
fn a_key_labelled_with_one_version_is_never_copied_to_another() {
    let two = forms(&["A", "B"]);
    // A key that names no version is every version's (identical versions).
    for (what, bytes) in [
        ("question,key header", &b"question,key\n1,B\n2,C\n"[..]),
        ("answer header", b"Q,Answer\n1,B\n2,C\n"),
        ("headerless rows", b"1,B\n2,C\n"),
        ("single column", b"B\nC\n"),
        ("a * row", b"form,Q1,Q2\n*,B,C\n"),
    ] {
        let k = keys(bytes, &["A", "B"], 2);
        assert_eq!(answers(&k), vec![vec!["B", "C"]; 2], "{what}");
    }
    // A key for version C only, on an exam declaring A and B: refused, never copied to them.
    let c_only = b"question,C\n1,B\n2,C\n";
    let m = err(read_key_file(c_only, &two, 2, 5));
    assert!(
        m.contains("key for version C only") && m.contains("declares versions A, B"),
        "{m}"
    );
    // ... and the one version's key on an exam declaring that version.
    assert_eq!(answers(&keys(c_only, &["C"], 2)), vec![vec!["B", "C"]]);
    // Versions A–D on an exam declaring A and B: refused naming C and D, never dropped.
    let abcd = b"question,A,B,C,D\n1,A,B,C,D\n2,B,C,D,E\n";
    let m = err(read_key_file(abcd, &two, 2, 5));
    assert!(
        m.contains("version C, version D") && m.contains("does not declare"),
        "{m}"
    );
    let m = err(read_key_file(
        b"version,Q1,Q2\nA,A,B\nB,B,C\nC,C,D\n",
        &two,
        2,
        5,
    ));
    assert!(
        m.contains("version C") && m.contains("does not declare"),
        "{m}"
    );
    // A key file whose undeclared versions hold no answer (a template's spare columns) reads
    // as the declared versions' key; declaring every version reads them all.
    let spare = b"question,A,B,C,D\n1,A,B,,\n2,B,C,,\n";
    assert_eq!(
        answers(&keys(spare, &["A", "B"], 2)),
        vec![vec!["A", "B"], vec!["B", "C"]]
    );
    assert_eq!(keys(abcd, &["A", "B", "C", "D"], 2).len(), 4);
}

/// A one-column key saved by a spreadsheet with an empty cell (an empty line): that question has
/// no entry, and every later answer stays on its own question.
#[test]
fn an_empty_cell_of_a_one_column_key_is_a_gap_never_skipped() {
    let a = forms(&["A"]);
    for (what, bytes) in [
        ("CRLF", &b"B\r\nC\r\n\r\nA\r\nD\r\nE\r\n"[..]),
        ("LF", b"B\nC\n\nA\nD\nE\n"),
        ("a line of spaces", b"B\nC\n \nA\nD\nE\n"),
        ("a quoted empty cell", b"B\nC\n\"\"\nA\nD\nE\n"),
    ] {
        let k = exam::read_key_file_full(bytes, &a, 5, 5).unwrap();
        assert_eq!(answers(&k.forms)[0], vec!["B", "C", "", "A", "D"], "{what}");
        assert_eq!(k.ignored_questions, vec![6], "{what}");
        let k = exam::read_key_file_full(bytes, &a, 6, 5).unwrap();
        assert_eq!(
            answers(&k.forms)[0],
            vec!["B", "C", "", "A", "D", "E"],
            "{what}"
        );
        assert!(k.ignored_questions.is_empty(), "{what}");
        let s = exam::key_file_shape(bytes, 100, 5).unwrap();
        assert_eq!(s.versions, exam::KeyVersions::Shared, "{what}");
        assert_eq!((s.questions, s.listed), (6, 6), "{what}");
        assert_eq!(s.gaps, vec![(String::new(), 3)], "{what}");
    }
    // Empty lines before the first answer and after the last are no questions.
    let lead = b"\r\n\r\nB\r\nC\r\n\r\n\r\n";
    let k = exam::read_key_file_full(lead, &a, 3, 5).unwrap();
    assert_eq!(answers(&k.forms)[0], vec!["B", "C", ""]);
    let s = exam::key_file_shape(lead, 100, 5).unwrap();
    assert_eq!((s.questions, s.listed, s.gaps.len()), (2, 2, 0));
    // An entry after an empty line is refused naming its own line.
    for bytes in [&b"B\nC\n\nF\n"[..], b"B\r\nC\r\n\r\nF\r\n"] {
        let m = err(exam::read_key_file_full(bytes, &a, 4, 5));
        assert!(m.contains("line 4") && m.contains("question 4"), "{m}");
        let m = err(exam::key_file_shape(bytes, 100, 5));
        assert!(m.contains("line 4") && m.contains("question 4"), "{m}");
    }
}

#[test]
fn key_rows_beyond_the_question_count_are_named_in_every_layout() {
    let two = forms(&["A", "B"]);
    // Questions 1–5 with answers; the exam uses 3. Question 6 has no answer, question 7 only
    // points: neither is named.
    let long = b"question,A,B,points\n1,A,B,1\n2,B,C,1\n3,C,D,1\n4,D,E,1\n5,,A,1\n6,,,1\n7,,,2\n";
    let k = exam::read_key_file_full(long, &two, 3, 5).unwrap();
    assert_eq!(k.ignored_questions, vec![4, 5], "long");
    assert_eq!(answers(&k.forms)[0], vec!["A", "B", "C"]);
    // One row per version, with and without a header.
    let wide = b"version,Q1,Q2,Q3,Q4,Q5,Q6\nA,A,B,C,D,,\nB,B,C,D,*,E,\n";
    let k = exam::read_key_file_full(wide, &two, 3, 5).unwrap();
    assert_eq!(k.ignored_questions, vec![4, 5], "wide");
    let k = exam::read_key_file_full(b"C,A,B,D,E\nD,B,C,E,A\n", &two, 3, 5).unwrap();
    assert_eq!(k.ignored_questions, vec![4, 5], "wide, no header");
    assert_eq!(answers(&k.forms)[1], vec!["D", "B", "C"]);
    // A single column of answers, and headerless rows.
    let k = exam::read_key_file_full(b"A\nB\nC\nD\n-\nE\n", &forms(&["A"]), 3, 5).unwrap();
    assert_eq!(
        k.ignored_questions,
        vec![4, 6],
        "single column ('-' is no entry)"
    );
    let k = exam::read_key_file_full(b"1,A\n2,B\n3,C\n10,D\n", &two, 3, 5).unwrap();
    assert_eq!(k.ignored_questions, vec![10], "headerless rows");
    // Nothing beyond: nothing named, no warning.
    let k = exam::read_key_file_full(long, &two, 7, 5).unwrap();
    assert!(k.ignored_questions.is_empty());
    assert_eq!(k.ignored_warning(7), None);
    // The new-exam definition names them in a warning, with the count that grades them.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key.csv");
    std::fs::write(&path, long).unwrap();
    let mut i =
        omr_app::define::Inputs::new(omr_app::define::Sheet::Template("iau-ccsit-v1".into()), 3);
    i.key = Some(path);
    let (d, warnings) = omr_app::define::define(&i).unwrap();
    assert_eq!(d.questions, 3);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("questions 4–5 have an answer")
            && warnings[0].contains("3 questions")
            && warnings[0].contains("question count to 5"),
        "{}",
        warnings[0]
    );
}

#[test]
fn an_own_sheets_cancel_mode_comes_from_its_spec_and_the_key_sheet_rule_from_the_inputs() {
    use omr_app::define::{Inputs, Sheet, define};
    let dir = tempfile::tempdir().unwrap();
    let spec_file = |name: &str, cancel: Option<CancelMode>| {
        let mut s = omr_forms::TemplateSpec::standard(
            omr_model::ExamId(uuid::Uuid::from_u128(
                0x0190_aaaa_bbbb_7ccc_8ddd_0000_0000_0099,
            )),
            20,
            4,
            CancelMode::CrossCancels,
        );
        s.cancel_mode = cancel;
        let p = dir.path().join(name);
        let json = serde_json::json!({"template": s, "exam_key_hex": "6b".repeat(32)});
        std::fs::write(&p, serde_json::to_vec(&json).unwrap()).unwrap();
        p
    };
    // The sheets print cross_cancels: the exam marks with it, with nothing typed.
    let cross = Inputs::new(
        Sheet::Spec(spec_file("cross.json", Some(CancelMode::CrossCancels))),
        20,
    );
    let (d, _) = define(&cross).unwrap();
    assert_eq!(d.cancel_mode, CancelMode::CrossCancels);
    // A typed mode must agree with the printed one.
    let mut typed = cross.clone();
    typed.cancel_mode = Some("cross_cancels".into());
    assert_eq!(
        define(&typed).unwrap().0.cancel_mode,
        CancelMode::CrossCancels
    );
    typed.cancel_mode = Some("erase_only".into());
    let e = define(&typed).unwrap_err().to_string();
    assert!(e.contains("cancel mode"), "{e}");
    // A spec without one could not have printed sheets: refused, naming the file and field.
    let none = Inputs::new(Sheet::Spec(spec_file("unset.json", None)), 20);
    let e = define(&none).unwrap_err().to_string();
    assert!(e.contains("unset.json") && e.contains("cancel_mode"), "{e}");
    // The IAU sheet marks with erase_only.
    let iau = Inputs::new(Sheet::Template("iau-ccsit-v1".into()), 20);
    let (d, _) = define(&iau).unwrap();
    assert_eq!(d.cancel_mode, CancelMode::EraseOnly);
    assert_eq!(d.key_sheets, omr_app::KeySheetRule::default());
    // The key-sheet rule the CLI's --key-sheet-marker sets reaches the definition.
    let mut ruled = iau.clone();
    let rule = omr_app::KeySheetRule {
        detect_in_stack: false,
        markers: vec![9],
        declared: Vec::new(),
    };
    ruled.key_sheets = Some(rule.clone());
    assert_eq!(define(&ruled).unwrap().0.key_sheets, rule);
    ruled.key_sheets = Some(omr_app::KeySheetRule {
        markers: vec![9, 9],
        ..omr_app::KeySheetRule::default()
    });
    let e = define(&ruled).unwrap_err().to_string();
    assert!(e.contains("markers repeat"), "{e}");
}

fn roster_def() -> ExamDefinition {
    let mut d = ExamDefinition::new(
        "Quiz",
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        5,
        5,
        forms(&["A"]),
        CancelMode::EraseOnly,
    );
    d.id_scheme = IdSchemeChoice::Iau;
    d
}

#[test]
fn c6_roster_csv_with_bom_arabic_names_and_excel_damage() {
    // A Banner/Edugate export: title rows above the header, Arabic headers and names, BOM, CRLF.
    let csv = "\u{feff}Course: CS 522 Final\r\n\r\nالرقم الجامعي,الاسم,الاسم العربي,الشعبة\r\n\
               2210345601,Test One,طالب أول,101\r\n\
               2210345602.0,Test Two,طالب ثان,101\r\n\
               ٢٢١٠٣٤٥٦٠٣,Test Three,طالب ثالث,102\r\n\
               2.210345604E+09,Test Four,طالب رابع,102\r\n";
    let rows = read_roster_csv(csv.as_bytes()).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        ["2210345601", "2210345602", "2210345603", "2210345604"]
    );
    assert_eq!(rows[0].name_ar, "طالب أول");
    assert_eq!(rows[2].section, "102");
    assert!(!rows[0].numeric_cell && rows[1].numeric_cell && rows[3].numeric_cell);
    let mut d = roster_def();
    let warnings = d.import_roster(rows).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("was a number")),
        "{warnings:?}"
    );
    d.validate().unwrap();
    // Excel's scientific display lost digits: refused, with the fix.
    let m = err(read_roster_csv(
        b"student_id,name\n2210345601,A\n2.21E+09,B\n",
    ));
    assert!(m.contains("line 3") && m.contains("Text"), "{m}");
    // A decimal that is no ID, and a named row without an ID.
    let m = err(read_roster_csv(b"student_id,name\n2210345601.5,A\n,B\n"));
    assert!(
        m.contains("decimal number") && m.contains("without a student ID"),
        "{m}"
    );
    // No ID column at all.
    let m = err(read_roster_csv(b"name,section\nA,1\n"));
    assert!(m.contains("student_id"), "{m}");
    // Semicolon-separated, Windows-1256.
    let (cp, _, _) = encoding_rs::WINDOWS_1256.encode("الرقم الجامعي;الاسم\n2210345601;محمد\n");
    let rows = read_roster_csv(&cp).unwrap();
    assert_eq!(
        (rows[0].id.as_str(), rows[0].name.as_str()),
        ("2210345601", "محمد")
    );
}

#[test]
fn c6_duplicate_and_damaged_roster_ids_are_refused_with_the_fix() {
    let mut d = roster_def();
    let rows =
        read_roster_csv(b"student_id,name\n2210345601,A\n2210345602,B\n 2210345601 ,C\n").unwrap();
    let m = err(d.import_roster(rows.clone()));
    assert!(
        m.contains("2210345601 appears 2 times")
            && m.contains("roster row 1")
            && m.contains("roster row 3"),
        "{m}"
    );
    // A definition carrying such a roster is refused before any grading.
    d.roster = Some(rows);
    assert!(d.validate().is_err());
}

#[test]
fn c6_lost_leading_zeros_restored_only_when_the_scheme_makes_it_unambiguous() {
    // A 9-digit scheme: Excel turned 012345678 into 12345678.
    let mut d = roster_def();
    d.id_scheme = IdSchemeChoice::Digits(9);
    let rows = read_roster_csv(b"student_id,name\n12345678,A\n987654321,B\n").unwrap();
    let w = d.import_roster(rows).unwrap();
    let r = d.roster.as_ref().unwrap();
    assert_eq!(
        (r[0].id.as_str(), r[1].id.as_str()),
        ("012345678", "987654321")
    );
    assert!(
        w.iter().any(|w| w.contains("leading zeros restored")),
        "{w:?}"
    );
    // Under the IAU scheme (2 YY NNNNNNN) no zero is missing: nothing is padded.
    let mut d = roster_def();
    let rows = read_roster_csv(b"student_id,name\n221034560,A\n").unwrap();
    let w = d.import_roster(rows).unwrap();
    assert_eq!(d.roster.as_ref().unwrap()[0].id, "221034560");
    assert!(w.iter().any(|w| w.contains("does not fit")), "{w:?}");
}

#[test]
fn c6_roster_xlsx_with_numeric_id_cells_and_arabic_names() {
    use rust_xlsxwriter::Workbook;
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "Student ID").unwrap();
    ws.write_string(0, 1, "Name").unwrap();
    ws.write_string(0, 2, "الاسم العربي").unwrap();
    // Numeric cells (Excel's default for IDs), a text cell, and one shown as 2.2E+09 text.
    ws.write_number(1, 0, 2_210_345_601.0).unwrap();
    ws.write_string(1, 1, "Test One").unwrap();
    ws.write_string(1, 2, "طالب أول").unwrap();
    ws.write_string(2, 0, "2210345602").unwrap();
    ws.write_string(2, 1, "Test Two").unwrap();
    let bytes = wb.save_to_buffer().unwrap();
    let rows = read_roster_xlsx(&bytes).unwrap();
    assert_eq!(rows[0].id, "2210345601");
    assert!(rows[0].numeric_cell);
    assert_eq!(rows[0].name_ar, "طالب أول");
    let mut d = roster_def();
    d.import_roster(rows).unwrap();
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "student_id").unwrap();
    ws.write_string(1, 0, "2.2E+09").unwrap();
    let m = err(read_roster_xlsx(&wb.save_to_buffer().unwrap()));
    assert!(m.contains("scientific notation"), "{m}");
    assert!(err(read_roster_xlsx(b"not a workbook")).contains(".xlsx"));
}
