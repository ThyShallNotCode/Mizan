//! The instructors' key sheets of the two real IAU exams (exam-confidential; spec §2 "never
//! silently wrong", §8.4 registration gates). Skips unless `OMR_IAU_PDF_DIR` (the PDFs) and
//! `OMR_IAU_KEYS` (the verified key CSVs) are set. Prints match counts only — never a key
//! letter, a read answer or an image.
//!
//! The CS 522 key is two phone photos (RGB JPEG ≈ 142 ppi, perspective, uneven light); the
//! ARTI 503 keys are 600-dpi bitonal scans like the student sheets. Every page must register,
//! read its version and the all-0 / all-9 student number, and read every answer equal to the
//! verified key; nothing auto-accepted may differ from it.

#[path = "support/key_sheets.rs"]
mod key_sheets;

#[test]
fn req_a1_real_key_sheets_read_exactly() {
    let Some(v) = key_sheets::evaluate() else {
        eprintln!("OMR_IAU_PDF_DIR / OMR_IAU_KEYS not set: skipping the key-sheet evaluation");
        return;
    };
    eprintln!("{v:#}");
    for f in v.as_array().unwrap() {
        assert!(f.get("missing").is_none(), "key file missing: {f:#}");
        for p in f["pages"].as_array().unwrap() {
            let label = format!("{} page {}", f["file"], p["page"]);
            assert_eq!(p["auto_accepted_wrong"], 0, "{label}: silent error");
            assert_eq!(p["registered"], true, "{label}");
            assert_eq!(p["version_ok"], true, "{label}: version");
            assert_eq!(p["id_ok"], true, "{label}: student-number pattern");
            assert_eq!(p["equal_key"], p["questions"], "{label}: answers vs key");
        }
    }
}
