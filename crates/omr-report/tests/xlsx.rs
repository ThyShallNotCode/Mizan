//! XLSX exports: content, IDs as text, RTL view, classification label in header, footer and
//! document properties, and byte-for-byte determinism.

mod fixture;

use fixture::*;
use omr_report::{Headers, ItemAnalysis, Tp154, Tp154Labels};

fn sheet_xml(zip: &[u8], n: usize) -> String {
    unzip_str(zip, &format!("xl/worksheets/sheet{n}.xml"))
}

#[test]
fn results_workbook_content_and_metadata() {
    let f = fixture();
    let mut a = Vec::new();
    f.results.write_xlsx(&mut a).unwrap();
    let mut b = Vec::new();
    f.results.write_xlsx(&mut b).unwrap();
    assert_eq!(a, b, "XLSX output must be deterministic");
    assert!(a.starts_with(b"PK"));
    let shared = unzip_str(&a, "xl/sharedStrings.xml");
    for s in [
        "Restricted / مقيد",
        "CS 101 Quiz 1 results",
        "220000001",
        "'@Noura Test",
        "محمد العتيبي",
        "أ+",
        "هـ",
        "Response codes",
    ] {
        assert!(shared.contains(s), "shared strings miss {s}");
    }
    let s1 = sheet_xml(&a, 1);
    // Student IDs are shared strings (t="s"), never numbers: row 5 (after banner, title,
    // blank and header rows), column A.
    assert!(
        s1.contains("<c r=\"A5\" s=\""),
        "{}",
        &s1[..400.min(s1.len())]
    );
    let a5 = &s1[s1.find("<c r=\"A5\"").unwrap()..];
    assert!(a5[..a5.find("</c>").unwrap()].contains("t=\"s\""));
    // Percent is a number (100), the booklet an integer.
    let i5 = &s1[s1.find("<c r=\"I5\"").unwrap()..];
    assert!(i5[..i5.find("</c>").unwrap()].contains("<v>100</v>"));
    // Header and footer carry the label; LTR view for an English sheet.
    assert!(s1.contains("<oddHeader>&amp;LRestricted / مقيد"));
    assert!(s1.contains("<oddFooter>&amp;LRestricted / مقيد&amp;RRun 9f2c4e1ab37d5560 · Key 1"));
    assert!(!s1.contains("rightToLeft=\"1\""));
    let core = unzip_str(&a, "docProps/core.xml");
    assert!(core.contains("2026-10-15T09:00:00Z"), "{core}");
    assert!(core.contains("CS 101 Quiz 1 results"));
    let custom = unzip_str(&a, "docProps/custom.xml");
    for s in [
        "Classification",
        "Restricted / مقيد",
        "ScoreRunId",
        "ChainHead",
        "no AI",
    ] {
        assert!(custom.contains(s), "custom properties miss {s}");
    }
}

#[test]
fn tp154_arabic_workbook_is_rtl() {
    let f = fixture();
    let meta = meta();
    let labels = Tp154Labels::default();
    let tp = Tp154 {
        meta: &meta,
        labels: &labels,
        headers: Headers::Arabic,
    };
    let mut out = Vec::new();
    tp.write_xlsx(&mut out, &f.a1, &f.b1).unwrap();
    let a1 = sheet_xml(&out, 1);
    let b1 = sheet_xml(&out, 2);
    assert!(a1.contains("rightToLeft=\"1\""));
    assert!(b1.contains("rightToLeft=\"1\""));
    let shared = unzip_str(&out, "xl/sharedStrings.xml");
    for s in [
        "توزيع التقديرات",
        "عدد الطلاب",
        "نواتج تعلم المقرر",
        "مستوى الأداء المستهدف",
        "المعرفة والفهم",
        "محروم",
    ] {
        assert!(shared.contains(s), "missing {s}");
    }
    let wb = unzip_str(&out, "xl/workbook.xml");
    assert!(wb.contains("TP-154 A.1") && wb.contains("TP-154 B.1"));
    // A.1 percentages are Excel percentages: 12.5 % is stored as 0.125.
    assert!(a1.contains("<v>0.125</v>"));
}

#[test]
fn item_analysis_workbook_has_three_sheets() {
    let f = fixture();
    let meta = meta();
    let ia = ItemAnalysis {
        meta: &meta,
        report: &f.analytics,
        show_key: false,
    };
    let mut out = Vec::new();
    ia.write_xlsx(&mut out).unwrap();
    let wb = unzip_str(&out, "xl/workbook.xml");
    for s in ["Items", "Test summary", "Flags"] {
        assert!(wb.contains(&format!("name=\"{s}\"")), "missing sheet {s}");
    }
}
