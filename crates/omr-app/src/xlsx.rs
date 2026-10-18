//! Bounded reading of `.xlsx` worksheets for the importers (spec §16.3: every importer is
//! size-limited; §3.5: Remark's graded exports; §9.2: rosters).
//!
//! A workbook's cells are streamed one by one and kept sparse: a sheet with one stray value at
//! the far corner (`XFD1048576`) would otherwise be expanded into a dense 2³⁴-cell grid and the
//! allocation would abort the process (a 549 GB request from an 8 KB file). The ZIP directory
//! is checked before the workbook is opened: the parts may not declare more than
//! [`MAX_UNCOMPRESSED`] bytes, and ZIP64 archives (parts of 4 GB and more) are refused.

use calamine::{Data, Reader, Xlsx};
use std::collections::BTreeMap;
use std::io::Cursor;

/// Largest total size the workbook's parts may declare once decompressed.
pub const MAX_UNCOMPRESSED: u64 = 512 << 20;
/// Rows a sheet may use (a graded export or a roster has a few thousand).
pub const MAX_ROWS: u32 = 100_000;
/// Columns a sheet may use (200 answers and their companions fit many times over).
pub const MAX_COLS: u32 = 2_048;
/// Non-empty cells a sheet may hold.
pub const MAX_CELLS: usize = 5_000_000;

/// A sheet's non-empty rows: the worksheet row number (1-based, as Excel shows it) and the
/// row's cells from column A, `Data::Empty` where nothing is written.
pub type SheetRows = Vec<(u32, Vec<Data>)>;

fn u16_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 2)
        .map(|x| u64::from(u16::from_le_bytes([x[0], x[1]])))
}

fn u32_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 4)
        .map(|x| u64::from(u32::from_le_bytes([x[0], x[1], x[2], x[3]])))
}

/// The total decompressed size the ZIP central directory declares.
fn declared_size(bytes: &[u8]) -> Result<u64, String> {
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const CDH: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    let n = bytes.len();
    if n < 22 {
        return Err("not a ZIP archive".into());
    }
    let lo = n.saturating_sub(22 + 65_535);
    let e = (lo..=n - 22)
        .rev()
        .find(|&i| bytes[i..i + 4] == EOCD)
        .ok_or("no ZIP directory")?;
    let damaged = || "damaged ZIP directory".to_string();
    let entries = u16_at(bytes, e + 10).ok_or_else(damaged)?;
    let offset = u32_at(bytes, e + 16).ok_or_else(damaged)?;
    if entries == 0xFFFF || offset == 0xFFFF_FFFF {
        return Err("a ZIP64 archive (parts of 4 GB or more)".into());
    }
    let mut p = usize::try_from(offset).map_err(|_| damaged())?;
    let mut total = 0u64;
    for _ in 0..entries {
        if bytes.get(p..p + 4) != Some(&CDH[..]) {
            return Err(damaged());
        }
        let size = u32_at(bytes, p + 24).ok_or_else(damaged)?;
        if size == 0xFFFF_FFFF {
            return Err("a ZIP64 archive (parts of 4 GB or more)".into());
        }
        total += size;
        let names = u16_at(bytes, p + 28).ok_or_else(damaged)?
            + u16_at(bytes, p + 30).ok_or_else(damaged)?
            + u16_at(bytes, p + 32).ok_or_else(damaged)?;
        p += 46 + usize::try_from(names).map_err(|_| damaged())?;
    }
    Ok(total)
}

/// Opens a workbook after checking its ZIP directory ([`MAX_UNCOMPRESSED`]).
pub fn open(bytes: &[u8]) -> Result<Xlsx<Cursor<Vec<u8>>>, String> {
    let size = declared_size(bytes)?;
    if size > MAX_UNCOMPRESSED {
        return Err(format!(
            "its parts declare {} MB once decompressed, more than any workbook of this kind \
             ({} MB at most)",
            size >> 20,
            MAX_UNCOMPRESSED >> 20
        ));
    }
    Xlsx::new(Cursor::new(bytes.to_vec())).map_err(|e| e.to_string())
}

/// `XFD1048576` for a 0-based (row, column).
fn a1(row: u32, col: u32) -> String {
    let mut c = col + 1;
    let mut letters = Vec::new();
    while c > 0 {
        let r = (c - 1) % 26;
        letters.push(char::from(b'A' + r as u8));
        c = (c - 1) / 26;
    }
    letters.iter().rev().collect::<String>() + &(u64::from(row) + 1).to_string()
}

/// The sheet's non-empty rows, streamed cell by cell; a value beyond [`MAX_ROWS`] rows or
/// [`MAX_COLS`] columns, or more than [`MAX_CELLS`] values, is refused naming the cell (empty
/// formatted cells anywhere are ignored).
pub fn sheet_rows(wb: &mut Xlsx<Cursor<Vec<u8>>>, name: &str) -> Result<SheetRows, String> {
    let mut reader = wb.worksheet_cells_reader(name).map_err(|e| e.to_string())?;
    let mut rows: BTreeMap<u32, Vec<(u32, Data)>> = BTreeMap::new();
    let mut count = 0usize;
    while let Some(cell) = reader.next_cell().map_err(|e| e.to_string())? {
        let (r, c) = cell.get_position();
        let v: Data = cell.get_value().clone().into();
        if v == Data::Empty || matches!(&v, Data::String(s) if s.is_empty()) {
            continue;
        }
        if r >= MAX_ROWS || c >= MAX_COLS {
            return Err(format!(
                "a value in cell {} of sheet `{name}`, beyond the {MAX_ROWS} rows and {MAX_COLS} \
                 columns a table of this kind uses; delete the cells outside the table and save \
                 the workbook again",
                a1(r, c)
            ));
        }
        count += 1;
        if count > MAX_CELLS {
            return Err(format!(
                "sheet `{name}` holds more than {MAX_CELLS} values, more than a table of this \
                 kind"
            ));
        }
        rows.entry(r).or_default().push((c, v));
    }
    Ok(rows
        .into_iter()
        .map(|(r, cells)| {
            let width = cells
                .iter()
                .map(|(c, _)| *c as usize + 1)
                .max()
                .unwrap_or(0);
            let mut row = vec![Data::Empty; width];
            for (c, v) in cells {
                row[c as usize] = v;
            }
            (r + 1, row)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_names_and_zip_directories() {
        assert_eq!(a1(0, 0), "A1");
        assert_eq!(a1(9, 27), "AB10");
        assert_eq!(a1(1_048_575, 16_383), "XFD1048576");
        assert!(declared_size(b"PK\x03\x04 not a zip").is_err());
        assert!(declared_size(&[]).is_err());
    }
}
