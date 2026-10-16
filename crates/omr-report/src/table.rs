//! One table model, two writers: every tabular report becomes [`Sheet`]s, written as CSV
//! (spec §16.3: UTF-8 with BOM, IDs as quoted text, canonical numbers, formula guard on text
//! columns, bidi isolates stripped) or XLSX (`rust_xlsxwriter`: IDs as text, RTL view for
//! Arabic sheets, classification label in the header, footer and document properties).
//!
//! Writers take any `std::io::Write`; the caller opens files (R4).

use crate::error::ReportError;
use crate::meta::ReportMeta;
use crate::text::{guard_text, is_canonical_number, strip_bidi};
use rust_xlsxwriter::{
    DocProperties, ExcelDateTime, Format, FormatAlign, FormatBorder, Workbook, Worksheet,
};
use std::io::Write;

/// One cell.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    /// Nothing.
    Empty,
    /// Free text (names, notes, labels): formula-guarded.
    Text(String),
    /// An identifier: always text (quoted in CSV), formula-guarded.
    Id(String),
    /// A canonical number with its display precision.
    Num {
        /// Canonical text, `-?[0-9]+(\.[0-9]+)?`.
        text: String,
        /// Decimal places for XLSX display.
        decimals: u32,
    },
    /// A percentage 0–100 written as a canonical number (CSV) or an Excel percentage (XLSX).
    Percent {
        /// Canonical text of the 0–100 value.
        text: String,
        /// Decimal places.
        decimals: u32,
    },
    /// An integer.
    Int(i64),
}

impl Cell {
    /// Text cell.
    pub fn text(s: impl Into<String>) -> Self {
        Cell::Text(s.into())
    }

    /// Optional text (empty when `None`).
    pub fn opt_text(s: Option<impl Into<String>>) -> Self {
        s.map_or(Cell::Empty, |s| Cell::Text(s.into()))
    }

    /// A number from canonical text (empty when `None`).
    pub fn num(text: Option<String>, decimals: u32) -> Self {
        match text {
            Some(t) if is_canonical_number(&t) => Cell::Num { text: t, decimals },
            Some(t) => Cell::Text(t),
            None => Cell::Empty,
        }
    }

    /// A 0–100 percentage from canonical text.
    pub fn percent(text: Option<String>, decimals: u32) -> Self {
        match text {
            Some(t) if is_canonical_number(&t) => Cell::Percent { text: t, decimals },
            Some(t) => Cell::Text(t),
            None => Cell::Empty,
        }
    }
}

/// Row emphasis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowStyle {
    /// Data row.
    #[default]
    Normal,
    /// A group heading (e.g. an NQF domain in TP-154 B.1).
    Group,
    /// A total row.
    Total,
}

/// One row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// Emphasis.
    pub style: RowStyle,
    /// Cells.
    pub cells: Vec<Cell>,
}

impl Row {
    /// A data row.
    pub fn new(cells: Vec<Cell>) -> Self {
        Self {
            style: RowStyle::Normal,
            cells,
        }
    }

    /// A group heading row.
    pub fn group(cells: Vec<Cell>) -> Self {
        Self {
            style: RowStyle::Group,
            cells,
        }
    }
}

/// A rectangular table with one or more header rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    /// Sheet name (≤ 31 characters in XLSX).
    pub name: String,
    /// Title shown above the table in XLSX.
    pub title: String,
    /// Header rows (e.g. English then Arabic).
    pub headers: Vec<Vec<String>>,
    /// Data rows.
    pub rows: Vec<Row>,
    /// Footnotes (XLSX only; CSV carries data only).
    pub notes: Vec<String>,
    /// Right-to-left sheet view.
    pub rtl: bool,
    /// Column widths (characters); missing columns use 12.
    pub widths: Vec<f64>,
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

fn csv_field(out: &mut String, s: &str, force_quote: bool) {
    let needs =
        force_quote || s.contains([',', '"', '\r', '\n']) || s.starts_with(' ') || s.ends_with(' ');
    if needs {
        out.push('"');
        out.push_str(&s.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(s);
    }
}

/// Writes `sheet` as CSV: UTF-8 with BOM, CRLF line ends, header rows then data rows.
pub fn write_csv<W: Write>(w: &mut W, sheet: &Sheet) -> Result<(), ReportError> {
    let mut out = String::new();
    for h in &sheet.headers {
        for (k, s) in h.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            csv_field(&mut out, &guard_text(s), false);
        }
        out.push_str("\r\n");
    }
    for row in &sheet.rows {
        for (k, c) in row.cells.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            match c {
                Cell::Empty => {}
                Cell::Text(s) => csv_field(&mut out, &guard_text(s), false),
                Cell::Id(s) => csv_field(&mut out, &guard_text(s), true),
                Cell::Num { text, .. } | Cell::Percent { text, .. } => out.push_str(text),
                Cell::Int(i) => out.push_str(&i.to_string()),
            }
        }
        out.push_str("\r\n");
    }
    w.write_all(BOM)?;
    w.write_all(out.as_bytes())?;
    Ok(())
}

/// Escapes `&` for Excel header/footer strings.
fn hf(s: &str) -> String {
    strip_bidi(s).replace('&', "&&")
}

/// Truncates an escaped header/footer string to `n` characters without leaving half of a
/// `&&` escape at the end.
fn hf_truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(n).collect();
    let trailing = t.chars().rev().take_while(|&c| c == '&').count();
    if trailing % 2 == 1 {
        t.pop();
    }
    t
}

fn num_format(decimals: u32) -> String {
    if decimals == 0 {
        "0".into()
    } else {
        format!("0.{}", "0".repeat(decimals as usize))
    }
}

/// The document date: the snapshot time, which must be RFC 3339 UTC
/// (`YYYY-MM-DDTHH:MM:SS[.fff]Z`). Without it `rust_xlsxwriter` would stamp the current time,
/// so the bytes would change on every run and the date would not be the snapshot's (§17.1).
fn snapshot_datetime(meta: &ReportMeta) -> Result<ExcelDateTime, ReportError> {
    let s = meta.snapshot_utc.as_str();
    let bad = || {
        ReportError::Invalid(format!(
            "snapshot time {s:?} is not RFC 3339 UTC (YYYY-MM-DDTHH:MM:SS[.fff]Z)"
        ))
    };
    if !(s.len() >= 20 && s.is_char_boundary(19) && (s.ends_with('Z') || s.ends_with('z'))) {
        return Err(bad());
    }
    // Optional fraction between the seconds and the `Z`.
    let frac = &s[19..s.len() - 1];
    if !(frac.is_empty()
        || (frac.len() >= 2
            && frac.starts_with('.')
            && frac[1..].bytes().all(|b| b.is_ascii_digit())))
    {
        return Err(bad());
    }
    ExcelDateTime::parse_from_str(&format!("{}Z", &s[..19])).map_err(|_| bad())
}

fn properties(meta: &ReportMeta) -> Result<DocProperties, ReportError> {
    let mut p = DocProperties::new()
        .set_title(meta.title.clone())
        .set_author("OMR Correction")
        .set_comment(meta.footer_line())
        .set_keywords(format!("classification: {}", meta.classification.label()))
        .set_custom_property("Classification", meta.classification.label().as_str())
        .set_custom_property("SnapshotUTC", meta.snapshot_utc.as_str())
        .set_custom_property("AI", meta.ai_label.as_str());
    if let Some(s) = &meta.course_code {
        p = p.set_subject(s.clone());
    }
    if let Some(s) = &meta.institution {
        p = p.set_company(s.clone());
    }
    if let Some(s) = &meta.score_run_id {
        p = p.set_custom_property("ScoreRunId", s.as_str());
    }
    if let Some(s) = &meta.chain_head {
        p = p.set_custom_property("ChainHead", s.as_str());
    }
    if let Some(s) = &meta.roster_snapshot {
        p = p.set_custom_property("RosterSnapshot", s.as_str());
    }
    // Document date = snapshot time (determinism, §17.1).
    Ok(p.set_creation_datetime(&snapshot_datetime(meta)?))
}

struct Formats {
    banner: Format,
    title: Format,
    header: Format,
    group: Format,
    total: Format,
    text: Format,
    note: Format,
}

fn formats() -> Formats {
    let ink = rust_xlsxwriter::Color::RGB(0x1C2430);
    Formats {
        banner: Format::new()
            .set_bold()
            .set_font_color(rust_xlsxwriter::Color::RGB(0x9A5B00))
            .set_background_color(rust_xlsxwriter::Color::RGB(0xFAEBD3)),
        title: Format::new()
            .set_bold()
            .set_font_size(14)
            .set_font_color(ink),
        header: Format::new()
            .set_bold()
            .set_text_wrap()
            .set_align(FormatAlign::Top)
            .set_background_color(rust_xlsxwriter::Color::RGB(0xF0EADF))
            .set_border_bottom(FormatBorder::Thin),
        group: Format::new()
            .set_bold()
            .set_background_color(rust_xlsxwriter::Color::RGB(0xDCEBE6)),
        total: Format::new().set_bold().set_border_top(FormatBorder::Thin),
        text: Format::new().set_text_wrap().set_align(FormatAlign::Top),
        note: Format::new()
            .set_italic()
            .set_font_color(rust_xlsxwriter::Color::RGB(0x55606E)),
    }
}

fn write_sheet(
    ws: &mut Worksheet,
    meta: &ReportMeta,
    sheet: &Sheet,
    f: &Formats,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    ws.set_name(sheet.name.chars().take(31).collect::<String>())?;
    ws.set_right_to_left(sheet.rtl);
    let title = hf_truncate(&hf(&sheet.title), 200);
    ws.set_header(format!(
        "&L{}&C{title}&R&P / &N",
        hf(&meta.classification.label())
    ));
    // Excel caps a header or footer at 255 characters (escapes included).
    let label = hf(&meta.classification.label());
    let room = 255usize.saturating_sub(label.chars().count() + 4);
    let prov = hf_truncate(&hf(&meta.compact_footer()), room);
    ws.set_footer(format!("&L{label}&R{prov}"));
    let ncols = sheet
        .headers
        .iter()
        .map(Vec::len)
        .chain(sheet.rows.iter().map(|r| r.cells.len()))
        .max()
        .unwrap_or(1);
    for c in 0..ncols {
        let wdt = sheet.widths.get(c).copied().unwrap_or(12.0);
        ws.set_column_width(c as u16, wdt)?;
    }
    let mut r: u32 = 0;
    ws.write_string_with_format(r, 0, meta.classification.label(), &f.banner)?;
    r += 1;
    ws.write_string_with_format(r, 0, strip_bidi(&sheet.title), &f.title)?;
    r += 2;
    for h in &sheet.headers {
        for (c, s) in h.iter().enumerate() {
            ws.write_string_with_format(r, c as u16, strip_bidi(s), &f.header)?;
        }
        r += 1;
    }
    if !sheet.headers.is_empty() {
        ws.set_freeze_panes(r, 0)?;
    }
    let mut num_formats: Vec<(u32, bool, Format)> = Vec::new();
    for row in &sheet.rows {
        let base = match row.style {
            RowStyle::Normal => None,
            RowStyle::Group => Some(&f.group),
            RowStyle::Total => Some(&f.total),
        };
        for (c, cell) in row.cells.iter().enumerate() {
            let c = c as u16;
            match cell {
                Cell::Empty => {
                    if let Some(b) = base {
                        ws.write_blank(r, c, b)?;
                    }
                }
                Cell::Text(s) | Cell::Id(s) => {
                    ws.write_string_with_format(r, c, guard_text(s), base.unwrap_or(&f.text))?;
                }
                Cell::Num { text, decimals } | Cell::Percent { text, decimals } => {
                    let pct = matches!(cell, Cell::Percent { .. });
                    let v: f64 = text.parse().unwrap_or(0.0);
                    let fmt = match num_formats
                        .iter()
                        .find(|(d, p, _)| d == decimals && *p == pct)
                    {
                        Some((_, _, fm)) => fm.clone(),
                        None => {
                            let code = if pct {
                                format!("{}%", num_format(*decimals))
                            } else {
                                num_format(*decimals)
                            };
                            let fm = Format::new().set_num_format(code);
                            num_formats.push((*decimals, pct, fm.clone()));
                            fm
                        }
                    };
                    let fmt = match row.style {
                        RowStyle::Total => fmt.set_bold(),
                        _ => fmt,
                    };
                    ws.write_number_with_format(r, c, if pct { v / 100.0 } else { v }, &fmt)?;
                }
                Cell::Int(i) => {
                    match base {
                        Some(b) => ws.write_number_with_format(r, c, *i as f64, b)?,
                        None => ws.write_number(r, c, *i as f64)?,
                    };
                }
            }
        }
        r += 1;
    }
    if !sheet.notes.is_empty() {
        r += 1;
        for n in &sheet.notes {
            ws.write_string_with_format(r, 0, strip_bidi(n), &f.note)?;
            r += 1;
        }
    }
    r += 1;
    ws.write_string_with_format(r, 0, meta.footer_line(), &f.note)?;
    Ok(())
}

/// Writes `sheets` as one XLSX workbook.
pub fn write_xlsx<W: Write>(
    w: &mut W,
    meta: &ReportMeta,
    sheets: &[Sheet],
) -> Result<(), ReportError> {
    let mut wb = Workbook::new();
    wb.set_properties(&properties(meta)?);
    let f = formats();
    for s in sheets {
        let ws = wb.add_worksheet();
        write_sheet(ws, meta, s, &f)?;
    }
    let buf = wb.save_to_buffer()?;
    w.write_all(&buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_ids_and_guards_text() {
        let s = Sheet {
            name: "t".into(),
            headers: vec![vec!["ID".into(), "Name".into(), "Score".into()]],
            rows: vec![
                Row::new(vec![
                    Cell::Id("0042".into()),
                    Cell::text("=HYPERLINK(\"x\")"),
                    Cell::num(Some("-2.50".into()), 2),
                ]),
                Row::new(vec![Cell::Id("7".into()), Cell::text("a, b"), Cell::Empty]),
            ],
            ..Sheet::default()
        };
        let mut out = Vec::new();
        write_csv(&mut out, &s).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text,
            "\u{feff}ID,Name,Score\r\n\"0042\",\"'=HYPERLINK(\"\"x\"\")\",-2.50\r\n\"7\",\"a, b\",\r\n"
        );
    }

    #[test]
    fn xlsx_document_date_is_the_utc_snapshot_or_an_error() {
        let s = Sheet {
            name: "t".into(),
            ..Sheet::default()
        };
        // Never a silent fallback to "now" (non-deterministic bytes, wrong document date).
        for bad in [
            "",
            "now",
            "2026-10-15",
            "2026-10-15T12:00:00+03:00",
            "2026-10-15T09:00:00.Z",
            "2026-13-45T09:00:00Z",
        ] {
            let m = ReportMeta::new("t", bad);
            assert!(
                matches!(
                    write_xlsx(&mut Vec::new(), &m, std::slice::from_ref(&s)),
                    Err(ReportError::Invalid(_))
                ),
                "{bad:?} accepted"
            );
        }
        for ok in ["2026-10-15T09:00:00Z", "2026-10-15T09:00:00.123Z"] {
            let m = ReportMeta::new("t", ok);
            let (mut a, mut b) = (Vec::new(), Vec::new());
            write_xlsx(&mut a, &m, std::slice::from_ref(&s)).unwrap();
            write_xlsx(&mut b, &m, std::slice::from_ref(&s)).unwrap();
            assert_eq!(a, b);
        }
    }

    #[test]
    fn non_canonical_numbers_become_text() {
        assert_eq!(Cell::num(Some("1e3".into()), 0), Cell::Text("1e3".into()));
        assert_eq!(Cell::num(None, 0), Cell::Empty);
    }
}
