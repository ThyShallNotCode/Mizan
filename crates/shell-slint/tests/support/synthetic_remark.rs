//! Synthetic Remark Office OMR graded workbooks for the desktop's Remark import and parallel
//! run (spec §3.5, §23.7): the sheet names and header layout of Remark's Advanced Grade Excel
//! export (Student Response Report, Student Statistics Report with learning-objective groups,
//! Test Statistics Report), filled from the synthetic exam's own sheets (`synthetic_exam.rs`:
//! invented students, painted marks), never from a real export. No real data anywhere.
//!
//! What the midterm's export carries, so every state of the import and the parallel run shows:
//! learning-objective groups whose points differ per version (exact points 0.5, 0.75 and 1),
//! the instructor's key sheet graded as a student (`0000000000`, answers equal the key, full
//! marks), a clear mark Remark read otherwise (a settled difference with its crop), a light
//! fill Remark left blank (a pending question that differs), a booklet with no Remark row, a
//! Remark row with no booklet, a row whose ID Remark could not read (`ERROR`), the number the
//! student meant where the sheet bubbles another, an ID typed as an Excel number, and another
//! section's workbook (not scanned here).

#![allow(dead_code)]

use super::synthetic_exam::{self as synth, Spec};
use rust_xlsxwriter::{Workbook, Worksheet};
use std::path::{Path, PathBuf};

/// The sheet names of a Remark graded workbook, in Remark's order.
pub const SHEETS: [&str; 11] = [
    "Student Statistics Report",
    "Class Frequency Distribution Re",
    "Test Statistics Report",
    "Student Response Report",
    "Condensed Test Report",
    "Class Learning Objective Report",
    "Student Achievement Report",
    "Detailed Item Analysis Report",
    "Item Analysis Graph Report",
    "Condensed Item Analysis Report",
    "Test Item Statistics Report",
];

/// How an ID cell is written.
#[derive(Clone, Debug)]
pub enum Id {
    Text(String),
    /// An Excel-typed number.
    Number(f64),
}

/// One student row as Remark exports it.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: Id,
    /// Invented name (never read by the importer).
    pub name: String,
    pub version: String,
    /// Answers as Remark writes them (`B`; empty = blank).
    pub answers: Vec<String>,
}

/// A workbook to write.
#[derive(Clone, Debug)]
pub struct Book {
    /// `Section No` and `Gender` columns.
    pub sections: bool,
    pub key: Vec<String>,
    pub points: Vec<f64>,
    /// Learning-objective groups: name and 1-based question range (inclusive).
    pub groups: Vec<(String, u16, u16)>,
    pub rows: Vec<Row>,
}

impl Book {
    /// Remark's total of a row.
    pub fn total(&self, r: &Row) -> f64 {
        (0..self.key.len())
            .filter(|q| r.answers[*q] == self.key[*q])
            .map(|q| self.points[q])
            .sum()
    }

    fn possible(&self) -> f64 {
        self.points.iter().sum()
    }

    fn group_points(&self, r: &Row, a: u16, z: u16) -> (f64, f64) {
        let qs = usize::from(a - 1)..usize::from(z);
        let got = qs
            .clone()
            .filter(|q| r.answers[*q] == self.key[*q])
            .map(|q| self.points[q])
            .sum();
        (got, qs.map(|q| self.points[q]).sum())
    }

    /// The statistics as Remark reports them (SD ÷ N; KR-20 on the number right; α on the
    /// weighted item scores).
    fn stats(&self) -> (f64, f64, f64, f64, f64) {
        let n = self.rows.len() as f64;
        let k = self.key.len();
        let x: Vec<Vec<bool>> = self
            .rows
            .iter()
            .map(|r| (0..k).map(|q| r.answers[q] == self.key[q]).collect())
            .collect();
        let totals: Vec<f64> = self.rows.iter().map(|r| self.total(r)).collect();
        let pvar = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            v.iter().map(|a| (a - m).powi(2)).sum::<f64>() / v.len() as f64
        };
        let mean = totals.iter().sum::<f64>() / n;
        let max = totals.iter().cloned().fold(f64::MIN, f64::max);
        let nr: Vec<f64> = x
            .iter()
            .map(|r| r.iter().filter(|v| **v).count() as f64)
            .collect();
        let spq: f64 = (0..k)
            .map(|q| {
                let p = x.iter().filter(|r| r[q]).count() as f64 / n;
                p * (1.0 - p)
            })
            .sum();
        let kf = k as f64;
        let kr20 = kf / (kf - 1.0) * (1.0 - spq / pvar(&nr));
        let iv: f64 = (0..k)
            .map(|q| {
                pvar(
                    &x.iter()
                        .map(|r| if r[q] { self.points[q] } else { 0.0 })
                        .collect::<Vec<_>>(),
                )
            })
            .sum();
        let alpha = kf / (kf - 1.0) * (1.0 - iv / pvar(&totals));
        (mean, pvar(&totals).sqrt(), max, kr20, alpha)
    }
}

fn id_cell(ws: &mut Worksheet, r: u32, c: u16, id: &Id) {
    match id {
        Id::Text(s) => {
            ws.write_string(r, c, s).expect("cell");
        }
        Id::Number(f) => {
            ws.write_number(r, c, *f).expect("cell");
        }
    }
}

/// The workbook's bytes.
pub fn xlsx(b: &Book) -> Vec<u8> {
    let mut wb = Workbook::new();
    for name in SHEETS {
        let ws = wb.add_worksheet();
        ws.set_name(name).expect("sheet name");
        match name {
            "Student Response Report" => response(ws, b),
            "Student Statistics Report" => statistics(ws, b),
            "Test Statistics Report" => test_stats(ws, b),
            _ => {
                ws.write_string(1, 0, name).expect("cell");
            }
        }
    }
    wb.save_to_buffer().expect("workbook")
}

fn response(ws: &mut Worksheet, b: &Book) {
    let n = b.key.len();
    ws.write_string(1, 0, "Student Response Report")
        .expect("cell");
    ws.write_string(3, 0, "Generated By Remark® From Gravic, Inc.")
        .expect("cell");
    let mut header: Vec<String> = vec!["Student ID".into(), "Student Name".into()];
    if b.sections {
        header.push("Section No".into());
        header.push("Gender".into());
    }
    for h in ["Version1", "Total", "Percent", "Grade"] {
        header.push(h.into());
    }
    let a0 = header.len() as u16;
    for q in 1..=n {
        header.push(format!("Answers{q}"));
    }
    for (c, h) in header.iter().enumerate() {
        ws.write_string(5, c as u16, h).expect("cell");
    }
    let c_total = a0 - 3;
    ws.write_string(6, 0, "Answer Key").expect("cell");
    ws.write_number(6, c_total, b.possible()).expect("cell");
    for (q, k) in b.key.iter().enumerate() {
        ws.write_string(6, a0 + q as u16, k).expect("cell");
    }
    for (r, s) in (7u32..).zip(&b.rows) {
        id_cell(ws, r, 0, &s.id);
        ws.write_string(r, 1, &s.name).expect("cell");
        let mut c = 2u16;
        if b.sections {
            ws.write_string(r, 2, "171").expect("cell");
            ws.write_string(r, 3, "Female").expect("cell");
            c = 4;
        }
        ws.write_string(r, c, &s.version).expect("cell");
        let t = b.total(s);
        ws.write_number(r, c + 1, t).expect("cell");
        ws.write_number(r, c + 2, 100.0 * t / b.possible())
            .expect("cell");
        ws.write_string(r, c + 3, "C").expect("cell");
        for (q, a) in s.answers.iter().enumerate() {
            if !a.is_empty() {
                ws.write_string(r, a0 + q as u16, a).expect("cell");
            }
        }
    }
    let r = 7 + b.rows.len() as u32;
    ws.write_string(r, 0, "Mean").expect("cell");
    let mean = b.rows.iter().map(|s| b.total(s)).sum::<f64>() / b.rows.len().max(1) as f64;
    ws.write_number(r, c_total, mean).expect("cell");
}

fn statistics(ws: &mut Worksheet, b: &Book) {
    ws.write_string(1, 0, "Student Statistics Report")
        .expect("cell");
    ws.write_string(3, 0, "Generated By Remark® From Gravic, Inc.")
        .expect("cell");
    let base = if b.sections { 5u16 } else { 3 };
    ws.write_string(5, base + 4, "Benchmark: ").expect("cell");
    ws.write_number(5, base + 5, 60.0).expect("cell");
    ws.write_string(6, 0, "Overall").expect("cell");
    for (k, (g, a, z)) in b.groups.iter().enumerate() {
        let c = base + 6 * (k as u16 + 1);
        ws.write_string(6, c, format!("{g} ({a}-{z})"))
            .expect("cell");
    }
    let mut header = vec!["Student ID", "Student Name"];
    if b.sections {
        header.extend(["Section No", "Gender"]);
    }
    header.push("Version1");
    for (c, h) in header.iter().enumerate() {
        ws.write_string(7, c as u16, *h).expect("cell");
    }
    for k in 0..=b.groups.len() as u16 {
        let c = base + 6 * k;
        ws.write_string(7, c, "Grade").expect("cell");
        ws.write_string(7, c + 1, "Total").expect("cell");
        ws.write_string(7, c + 4, "Percent").expect("cell");
    }
    for (r, s) in (8u32..).zip(&b.rows) {
        id_cell(ws, r, 0, &s.id);
        ws.write_string(r, 1, &s.name).expect("cell");
        if b.sections {
            ws.write_string(r, 2, "171").expect("cell");
            ws.write_string(r, 3, "Female").expect("cell");
        }
        ws.write_string(r, header.len() as u16 - 1, &s.version)
            .expect("cell");
        let mut blocks = vec![(b.total(s), b.possible())];
        for (_, a, z) in &b.groups {
            blocks.push(b.group_points(s, *a, *z));
        }
        for (k, (got, poss)) in blocks.into_iter().enumerate() {
            let c = base + 6 * k as u16;
            ws.write_string(r, c, "C").expect("cell");
            ws.write_number(r, c + 1, got).expect("cell");
            ws.write_string(r, c + 2, "/").expect("cell");
            ws.write_number(r, c + 3, poss).expect("cell");
            ws.write_number(r, c + 4, 100.0 * got / poss).expect("cell");
            ws.write_number(r, c + 5, 0.0).expect("cell");
        }
    }
}

fn test_stats(ws: &mut Worksheet, b: &Book) {
    ws.write_string(1, 0, "Test Statistics Report")
        .expect("cell");
    ws.write_string(5, 1, "Overall").expect("cell");
    ws.write_string(5, 2, "Extra Credit").expect("cell");
    let (mean, sd, max, kr20, alpha) = b.stats();
    let rows: [(&str, f64); 6] = [
        ("Total Possible Points", b.possible()),
        ("Maximum Score", max),
        ("Mean Score", mean),
        ("Standard Deviation", sd),
        ("Kuder-Richardson Formula 20", kr20),
        ("Coefficient (Cronbach) Alpha", alpha),
    ];
    for (i, (l, v)) in rows.iter().enumerate() {
        ws.write_string(7 + i as u32, 0, *l).expect("cell");
        ws.write_number(7 + i as u32, 1, *v).expect("cell");
        ws.write_string(7 + i as u32, 2, "-").expect("cell");
    }
}

fn letter(o: u8) -> String {
    char::from(b'A' + o).to_string()
}

/// The learning-objective groups of version `form` (0 = A) and the points they give: the two
/// versions order the objectives differently, so their points differ per question.
pub fn groups(form: usize) -> Vec<(String, u16, u16, f64)> {
    let g = |n: &str, a, z, p| (n.to_string(), a, z, p);
    if form == 0 {
        vec![
            g("LO1", 1, 8, 1.0),
            g("LO2", 9, 15, 0.5),
            g("LO3", 16, 24, 1.0),
            g("LO4", 25, 30, 0.75),
        ]
    } else {
        vec![
            g("LO2", 1, 7, 0.5),
            g("LO1", 8, 15, 1.0),
            g("LO3", 16, 24, 1.0),
            g("LO4", 25, 30, 0.75),
        ]
    }
}

fn points(form: usize, uniform: bool) -> Vec<f64> {
    let mut p = vec![0.0; usize::from(synth::QUESTIONS)];
    for (_, a, z, v) in groups(form) {
        for q in a..=z {
            p[usize::from(q - 1)] = if uniform { 1.0 } else { v };
        }
    }
    p
}

/// The sheet whose clear mark Remark read as another option (a settled difference).
pub const MISREAD_SHEET: usize = 6;
/// The light fill Remark left blank (a pending question that differs).
pub const BLANKED_SHEET: usize = 9;
/// The booklet Remark has no row for.
pub const NO_ROW_SHEET: usize = 50;
/// The row whose student number Remark could not read: the first sheet from 26 on (none of
/// the other planted cases) whose student departs from the key on four questions or more, so
/// the parallel run can pair it by its answers.
pub fn error_id_sheet(spec: &Spec) -> Option<usize> {
    (26..spec.sheets)
        .filter(|k| ![41, 50, MISREAD_SHEET, BLANKED_SHEET, NUMBER_ID_SHEET].contains(k))
        .find(|&k| {
            let t = synth::sheet_truth(spec, k);
            (0..synth::QUESTIONS)
                .filter(|&q| t.answers[usize::from(q)] != Some(synth::key(spec, t.form, q)))
                .count()
                >= 4
        })
}
/// The row written with an Excel-typed (number) ID.
pub const NUMBER_ID_SHEET: usize = 12;

/// The first answered question of a sheet at or after `from` (0-based).
fn answered_from(t: &synth::SheetTruth, from: usize) -> usize {
    (from..t.answers.len())
        .find(|&q| t.answers[q].is_some() && !t.light[q] && t.residue[q].is_none())
        .expect("an answered question")
}

/// The question (0-based) of [`MISREAD_SHEET`] Remark read otherwise.
pub fn misread_question(spec: &Spec) -> usize {
    answered_from(&synth::sheet_truth(spec, MISREAD_SHEET), 2)
}

/// Remark's graded workbooks of `spec`'s sheets (versions A and B, and another section's
/// version A), with their file names.
pub fn books(spec: &Spec) -> Vec<(String, Book)> {
    books_with(spec, false)
}

/// [`books`], every question worth 1 point when `uniform` (the exam's own points: a graded
/// exam whose key the import may replace in Setup).
pub fn books_with(spec: &Spec, uniform: bool) -> Vec<(String, Book)> {
    let n = usize::from(synth::QUESTIONS);
    let names = synth::roster(spec);
    let key = |form: usize| -> Vec<String> {
        (0..synth::QUESTIONS)
            .map(|q| letter(synth::key(spec, form, q)))
            .collect()
    };
    let error_sheet = error_id_sheet(spec);
    let mut rows: [Vec<Row>; 2] = [Vec::new(), Vec::new()];
    for k in 0..spec.sheets {
        if k == NO_ROW_SHEET {
            continue;
        }
        let t = synth::sheet_truth(spec, k);
        let mut answers: Vec<String> = t
            .answers
            .iter()
            .map(|a| a.map(letter).unwrap_or_default())
            .collect();
        if k == MISREAD_SHEET {
            let q = misread_question(spec);
            let a = t.answers[q].expect("answered");
            answers[q] = letter((a + 1) % synth::OPTIONS);
        }
        if k == BLANKED_SHEET
            && let Some(q) = t.light.iter().position(|l| *l)
        {
            answers[q] = String::new();
        }
        // Remark reads the sheet's version; an unbubbled version was typed in by its operator
        let id = if Some(k) == error_sheet {
            Id::Text("ERROR 000213".into())
        } else if k == NUMBER_ID_SHEET {
            Id::Number(t.id.parse::<f64>().expect("digits"))
        } else {
            // Remark lists the number the student meant where the sheet bubbles another (an
            // operator's correction), and every digit of a faintly bubbled one
            Id::Text(t.roster_id.clone())
        };
        rows[t.form].push(Row {
            id,
            // every sheet's student is on the roster (the roster also lists the absent)
            name: names.get(k).map(|s| s.name_ar.clone()).unwrap_or_default(),
            version: letter(t.form as u8),
            answers,
        });
    }
    // the instructor's key sheet graded as a student (version A), and a student absent here
    rows[0].insert(
        3,
        Row {
            id: Id::Text("0000000000".into()),
            name: "مفتاح الإجابة".into(),
            version: "A".into(),
            answers: key(0),
        },
    );
    let absent = spec.sheets + 1;
    rows[1].push(Row {
        id: Id::Text(synth::roster_id(spec, absent)),
        name: names[absent].name_ar.clone(),
        version: "B".into(),
        answers: (0..n)
            .map(|q| letter((synth::taught(spec, 1, q as u16) + (q % 3 == 0) as u8) % 5))
            .collect(),
    });
    let book = |form: usize, rows: Vec<Row>| Book {
        sections: true,
        key: key(form),
        points: points(form, uniform),
        groups: groups(form)
            .into_iter()
            .map(|(g, a, z, _)| (g, a, z))
            .collect(),
        rows,
    };
    // another section's version A (not scanned with this exam)
    let other: Vec<Row> = (0..8)
        .map(|i| Row {
            id: Id::Text(format!("92133{:05}", 101 + i * 7)),
            name: "طالبة من شعبة أخرى".into(),
            version: "A".into(),
            answers: (0..n)
                .map(|q| letter((synth::taught(spec, 0, q as u16) + ((q + i) % 4 == 0) as u8) % 5))
                .collect(),
        })
        .collect();
    let [a, bb] = rows;
    let slug = spec.course_code.replace(' ', "");
    vec![
        (format!("{slug}_A_MCQ_Graded.xlsx"), book(0, a)),
        (format!("{slug}_B_MCQ_Graded.xlsx"), book(1, bb)),
        (
            format!("{slug}_Section173_A_MCQ_Graded.xlsx"),
            book(0, other),
        ),
    ]
}

/// Writes [`books`] into `dir` and returns their paths.
pub fn write_books(spec: &Spec, dir: &Path) -> Vec<PathBuf> {
    write_books_with(spec, dir, false)
}

/// Writes [`books_with`] into `dir` and returns their paths.
pub fn write_books_with(spec: &Spec, dir: &Path, uniform: bool) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("workbook folder");
    books_with(spec, uniform)
        .into_iter()
        .map(|(name, b)| {
            let p = dir.join(name);
            std::fs::write(&p, xlsx(&b)).expect("write workbook");
            p
        })
        .collect()
}

/// A small export whose totals leave two points open: version A's questions 2 and 3 are
/// answered right by every student, so only their sum is known; version B (the same questions
/// in another order) is fully determined and leaves 0.75 for each (the import suggests it,
/// never assumes it).
pub fn underdetermined(dir: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("workbook folder");
    let pa = vec![1.0, 0.75, 0.75, 0.5, 1.0, 0.5];
    // B asks A's questions in the order 5, 3, 1, 6, 2, 4
    let order = [4usize, 2, 0, 5, 1, 3];
    let pb: Vec<f64> = order.iter().map(|&i| pa[i]).collect();
    let ka: Vec<String> = ["A", "C", "B", "D", "E", "B"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let kb: Vec<String> = order.iter().map(|&i| ka[i].clone()).collect();
    let wrong = |k: &str| if k == "A" { "B" } else { "A" }.to_string();
    // who answered which question right: in A, questions 2 and 3 by everyone (only their sum
    // is known); in B, every question by its own students (fully determined)
    let right_a: [[u8; 6]; 6] = [
        [1, 1, 1, 0, 0, 0],
        [0, 1, 1, 1, 0, 0],
        [0, 1, 1, 0, 1, 0],
        [0, 1, 1, 0, 0, 1],
        [1, 1, 1, 1, 0, 0],
        [0, 1, 1, 0, 1, 1],
    ];
    let right_b: [[u8; 6]; 7] = [
        [1, 0, 0, 0, 0, 0],
        [0, 1, 0, 0, 0, 0],
        [0, 0, 1, 0, 0, 0],
        [0, 0, 0, 1, 0, 0],
        [0, 0, 0, 0, 1, 0],
        [0, 0, 0, 0, 0, 1],
        [1, 1, 0, 0, 0, 0],
    ];
    let row = |id: String, version: &str, key: &[String], right: &[u8; 6]| Row {
        id: Id::Text(id),
        name: "طالب تجريبي".into(),
        version: version.into(),
        answers: (0..6)
            .map(|q| {
                if right[q] == 1 {
                    key[q].clone()
                } else {
                    wrong(&key[q])
                }
            })
            .collect(),
    };
    let rows_a: Vec<Row> = right_a
        .iter()
        .enumerate()
        .map(|(i, r)| row(format!("92777{:05}", 11 + i), "A", &ka, r))
        .collect();
    let rows_b: Vec<Row> = right_b
        .iter()
        .enumerate()
        .map(|(i, r)| row(format!("92777{:05}", 51 + i), "B", &kb, r))
        .collect();
    let a = Book {
        sections: false,
        key: ka,
        points: pa,
        groups: Vec::new(),
        rows: rows_a,
    };
    let b = Book {
        sections: false,
        key: kb,
        points: pb,
        groups: Vec::new(),
        rows: rows_b,
    };
    [("Quiz_A_Graded.xlsx", a), ("Quiz_B_Graded.xlsx", b)]
        .into_iter()
        .map(|(name, book)| {
            let p = dir.join(name);
            std::fs::write(&p, xlsx(&book)).expect("write workbook");
            p
        })
        .collect()
}
