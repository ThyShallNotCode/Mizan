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
    let k = keys(b"B\nc\n\n D\n", &["A"], 3);
    assert_eq!(
        entries(&k, 0),
        vec![Options(vec![1]), Options(vec![2]), Options(vec![3])]
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
