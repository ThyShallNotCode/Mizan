//! `omr-report`: exports and reports (spec §16.3, §17.1–§17.2, §11.7).
//!
//! Facts in, bytes out: every writer takes any `std::io::Write` and the caller opens files
//! (R4). Output is deterministic (document date = snapshot time) and carries the
//! classification label (default "Restricted / مقيد") in headers, footers and metadata.
//!
//! - [`results`]: one row per student with responses, scores and the Saudi letter with its
//!   Arabic letter (CSV, XLSX);
//! - [`items`]: item analysis, test summary and mis-key flags from `omr-analytics` (CSV, XLSX);
//! - [`ncaaa`]: NCAAA TP-154 A.1 grade distribution and B.1 CLO table with English and Arabic
//!   headers (CSV, XLSX);
//! - [`abet`]: ABET Criterion 3 summary, aggregate-only (CSV, XLSX);
//! - [`html`]: a self-contained single-file HTML results report (inline CSS and SVG);
//! - [`manifest`]: the SHA-384 export manifest the caller signs and registers;
//! - [`table`]: the shared sheet model and the CSV/XLSX writers; [`text`]: output safety
//!   rules (formula guard, bidi stripping, canonical numbers, escaping).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod abet;
pub mod error;
pub mod html;
pub mod items;
pub mod manifest;
pub mod meta;
pub mod ncaaa;
pub mod results;
pub mod table;
pub mod text;

pub use abet::{AbetIndicator, AbetSoRow, AbetSummary, CourseInput};
pub use error::ReportError;
pub use html::{GradeBar, HtmlReport, grade_distribution};
pub use items::ItemAnalysis;
pub use manifest::{ExportManifest, ManifestFile};
pub use meta::{Classification, Language, ReportMeta};
pub use ncaaa::{Headers, Tp154, Tp154Labels, Tp154Text};
pub use results::{ResultRow, ResultsTable, StudentInfo};
pub use table::{Cell, Row, RowStyle, Sheet, write_csv, write_xlsx};
