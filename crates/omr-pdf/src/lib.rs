//! `omr-pdf`: PDF ingest (§7.3): extracts embedded page images (image XObjects) from scanner/MFP PDFs
//! without rasterising; flags lossy JBIG2/MRC pages (CAP-004).
//!
//! Input is PDF bytes; the crate does no file or network I/O and creates no threads (R4 spirit).
//! Each page is isolated: a corrupt or unsupported page is refused with an Appendix A exception
//! (CAP-001, or CAP-004 for JBIG2 and layered/MRC pages) while the other pages extract normally.
//!
//! ```no_run
//! use omr_pdf::{ExtractOptions, PageOutcome, PdfDocument};
//! # fn demo(bytes: Vec<u8>) -> Result<(), omr_pdf::PdfError> {
//! let doc = PdfDocument::open(bytes, ExtractOptions::default())?;
//! for outcome in doc.extract_all() {
//!     match outcome {
//!         PageOutcome::Extracted(page) => {
//!             let dpi = page.nominal_dpi();                 // e.g. Some(600)
//!             let gray200 = page.working_gray(200)?;        // 8-bit coverage gray, upright
//!             let _ = (dpi, gray200);
//!         }
//!         PageOutcome::Refused(r) => eprintln!("{}: {}", r.exception.code, r.exception.detail),
//!     }
//! }
//! # Ok(()) }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ccitt;
pub mod extract;
pub mod flate;
pub mod raster;
pub mod reduce;
mod stream;

pub use extract::{
    ColorSpaceKind, ExtractOptions, ExtractedPage, ImageEncoding, ImageSource, Limits, PageNote,
    PageOutcome, PdfDocument, Placement, RefusedPage,
};
pub use raster::{BitImage, GrayImage8, JpegStream, Orientation, PageRaster};
pub use reduce::{reduce_bilevel, reduce_bilevel_to_dpi, reduce_gray};

use omr_model::{Exception, Scope, Severity};
use thiserror::Error;

/// Errors opening a PDF or decoding one of its images.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PdfError {
    /// Zero-byte input.
    #[error("empty (zero-byte) file")]
    Empty,
    /// Not a PDF, or the cross-reference/page tree cannot be read even in repair mode.
    #[error("not a readable PDF: {0}")]
    Invalid(String),
    /// Encrypted with a user password: prompt the operator (never brute-forced, §7.3).
    #[error("encrypted PDF: password required")]
    PasswordRequired,
    /// Over the 2 GB file limit (§7.4).
    #[error("file of {0} bytes exceeds the input limit")]
    FileTooLarge(u64),
    /// Over the 5,000-page limit (§7.4).
    #[error("{0} pages exceed the per-PDF limit")]
    TooManyPages(usize),
    /// Image over the 20,000 px per side / 100 MP limits, or not allocatable.
    #[error("image {width}x{height} exceeds the decode limits")]
    TooLarge {
        /// Declared width.
        width: u32,
        /// Declared height.
        height: u32,
    },
    /// Damaged data (truncated stream, bad codes, missing EOI).
    #[error("corrupt image data: {0}")]
    Corrupt(String),
    /// A valid construct this extractor does not handle.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Page index out of range.
    #[error("page {0} does not exist")]
    PageOutOfRange(usize),
}

impl PdfError {
    /// The Appendix A exception for this error: CAP-001 (BLOCKER, page scope), reason in the
    /// detail. For a document-level error it applies to every page of the file.
    pub fn exception(&self) -> Exception {
        Exception::must("CAP-001", Severity::Blocker, Scope::Page).with_detail(self.to_string())
    }
}

/// Why a page was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// JBIG2-coded page image (lossy symbol substitution risk, §7.2): CAP-004.
    Jbig2 {
        /// `/JBIG2Globals` present (the classic lossy "compact" mode).
        globals: bool,
    },
    /// Mask-plus-layers ("MRC compact PDF") page: CAP-004.
    Mrc,
    /// Anything else (corrupt, unsupported, too large, no image): CAP-001.
    Error(PdfError),
}

impl From<PdfError> for Refusal {
    fn from(e: PdfError) -> Self {
        Refusal::Error(e)
    }
}

impl Refusal {
    /// The Appendix A exception (CAP-004 ERROR or CAP-001 BLOCKER, page scope).
    pub fn exception(&self, page_index: u32) -> Exception {
        let page = format!("page {}", page_index + 1);
        let e = match self {
            Refusal::Jbig2 { globals } => Exception::must("CAP-004", Severity::Error, Scope::Page)
                .with_detail(format!(
                    "{page}: JBIG2 page image{} refused (admin override forces double review)",
                    if *globals { " with /JBIG2Globals" } else { "" }
                )),
            Refusal::Mrc => Exception::must("CAP-004", Severity::Error, Scope::Page).with_detail(
                format!("{page}: layered/MRC page (mask plus images) refused"),
            ),
            Refusal::Error(err) => Exception::must("CAP-001", Severity::Blocker, Scope::Page)
                .with_detail(format!("{page}: {err}")),
        };
        e.with_target(omr_model::exceptions::Target {
            page: Some(page),
            ..Default::default()
        })
    }
}
