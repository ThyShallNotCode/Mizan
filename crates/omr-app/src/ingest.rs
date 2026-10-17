//! Ingest (§7.3, §7.4): files and folders in, pages out. PDF pages through `omr-pdf` (image
//! XObjects, no rasterising), JPEG/PNG/TIFF through `omr-imaging`. Every page is isolated: a
//! corrupt or unsupported input becomes CAP-001 (or CAP-004) on that page and the batch goes on.
//! File I/O lives here (R4: only `omr-app` and the CLI touch files).

use crate::AppError;
use crate::engine::WORKING_DPI;
use omr_imaging::{CaptureKind, GrayImage};
use omr_model::exceptions::Target;
use omr_model::{Exception, Scope, Severity};
use omr_pdf::{ExtractOptions, PageOutcome, PageRaster, PdfDocument};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Kind of an input file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// Scanner PDF.
    Pdf,
    /// JPEG, PNG or TIFF image.
    Image,
}

/// Extensions accepted when walking folders.
pub const EXTENSIONS: &[&str] = &["pdf", "jpg", "jpeg", "png", "tif", "tiff"];

fn kind_of(path: &Path) -> Option<SourceKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some(SourceKind::Pdf),
        "jpg" | "jpeg" | "png" | "tif" | "tiff" => Some(SourceKind::Image),
        _ => None,
    }
}

/// Expands files and folders into the input files, folders walked recursively in name order.
/// A path that does not exist is an error; files of other types inside folders are skipped,
/// an explicitly named file of an unknown type is an error.
pub fn collect_paths(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, AppError> {
    let mut out = Vec::new();
    for p in inputs {
        if p.is_dir() {
            let mut files: Vec<PathBuf> = walkdir::WalkDir::new(p)
                .sort_by_file_name()
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
                .map(|e| e.into_path())
                .filter(|f| kind_of(f).is_some())
                .collect();
            files.sort();
            out.extend(files);
        } else if p.is_file() {
            if kind_of(p).is_none() {
                return Err(AppError::Input(format!(
                    "{}: not a PDF, JPEG, PNG or TIFF file",
                    p.display()
                )));
            }
            out.push(p.clone());
        } else {
            return Err(AppError::Input(format!(
                "{}: no such file or folder",
                p.display()
            )));
        }
    }
    if out.is_empty() {
        return Err(AppError::Input(
            "no input files (PDF, JPEG, PNG, TIFF) found".into(),
        ));
    }
    Ok(out)
}

/// One input file held in memory.
#[derive(Clone)]
pub struct SourceFile {
    /// Path as given.
    pub path: PathBuf,
    /// File name (no folders: report-safe).
    pub name: String,
    /// Bytes.
    pub bytes: Arc<[u8]>,
    /// SHA-384 (hex) of the bytes.
    pub sha384: String,
    /// Kind.
    pub kind: SourceKind,
}

impl std::fmt::Debug for SourceFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SourceFile({}, {} bytes)", self.name, self.bytes.len())
    }
}

impl SourceFile {
    /// Wraps bytes already in memory.
    pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Option<Self> {
        let kind = kind_of(path)?;
        let sha384 = omr_store::jcs::sha384_hex(&bytes);
        Some(Self {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bytes: bytes.into(),
            sha384,
            kind,
        })
    }

    /// Reads a file.
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_bytes(path, bytes).ok_or_else(|| "unknown file type".to_string())
    }
}

/// An opened source: a parsed PDF, an image, or a refusal.
pub enum OpenSource {
    /// A PDF document (pages extracted lazily, in parallel).
    Pdf(Arc<PdfDocument>),
    /// A single image.
    Image,
    /// The file could not be opened: every page of it is CAP-001.
    Broken(String),
}

/// Opens a source and counts its pages (a broken file counts one page).
pub fn open_source(src: &SourceFile) -> (OpenSource, u32) {
    match src.kind {
        SourceKind::Image => {
            if src.bytes.is_empty() {
                (OpenSource::Broken("empty (zero-byte) file".into()), 1)
            } else {
                (OpenSource::Image, 1)
            }
        }
        SourceKind::Pdf => match PdfDocument::open(src.bytes.clone(), ExtractOptions::default()) {
            Ok(doc) => {
                let n = doc.page_count() as u32;
                if n == 0 {
                    (OpenSource::Broken("PDF without pages".into()), 1)
                } else {
                    (OpenSource::Pdf(Arc::new(doc)), n)
                }
            }
            Err(e) => (OpenSource::Broken(e.to_string()), 1),
        },
    }
}

/// A decoded page before working-resolution conversion.
#[derive(Clone, Debug)]
pub struct DecodedPage {
    /// Gray image (upright as displayed).
    pub image: GrayImage,
    /// Capture classification.
    pub capture: CaptureKind,
    /// Resolution of `image` when known from the PDF placement.
    pub known_dpi: Option<u32>,
    /// Resolution of the original (PDF placement), for provenance.
    pub source_dpi: Option<u32>,
}

/// CAP-001 (BLOCKER, Page) for `page_id`.
pub fn cap001(page_id: &str, detail: &str) -> Exception {
    Exception::must("CAP-001", Severity::Blocker, Scope::Page)
        .with_target(Target {
            page: Some(page_id.to_string()),
            ..Target::default()
        })
        .with_detail(format!("{detail}; page quarantined, batch continues"))
}

fn to_gray(g: omr_pdf::GrayImage8) -> GrayImage {
    let (w, h) = (g.width() as usize, g.height() as usize);
    GrayImage::from_raw(w, h, g.into_raw()).expect("consistent raster")
}

/// Decodes page `index` of an opened source (pure apart from the bytes in memory).
pub fn decode_page(
    src: &SourceFile,
    open: &OpenSource,
    index: u32,
    page_id: &str,
) -> Result<DecodedPage, Exception> {
    match open {
        OpenSource::Broken(why) => Err(cap001(page_id, &format!("{}: {why}", src.name))),
        OpenSource::Image => {
            let d = omr_imaging::decode(&src.bytes)
                .map_err(|e| cap001(page_id, &format!("{}: {e}", src.name)))?;
            Ok(DecodedPage {
                image: d.image,
                capture: d.capture,
                known_dpi: None,
                source_dpi: None,
            })
        }
        OpenSource::Pdf(doc) => match doc.extract_page(index as usize) {
            PageOutcome::Refused(r) => {
                let mut e = r.exception.clone();
                e.target.page = Some(page_id.to_string());
                Err(e)
            }
            PageOutcome::Extracted(p) => {
                let src_dpi = p.nominal_dpi();
                let fail =
                    |e: String| cap001(page_id, &format!("{} page {}: {e}", src.name, index + 1));
                match &p.raster {
                    PageRaster::Bilevel(_) | PageRaster::Gray(_) => {
                        let bilevel = matches!(p.raster, PageRaster::Bilevel(_));
                        let g = p
                            .working_gray(WORKING_DPI)
                            .map_err(|e| fail(e.to_string()))?
                            .ok_or_else(|| fail("no raster".into()))?;
                        let image = to_gray(g);
                        let s = src_dpi.unwrap_or(WORKING_DPI);
                        let known = Some(s.min(WORKING_DPI));
                        let capture = if bilevel {
                            let f = (f64::from(s) / f64::from(WORKING_DPI)).round();
                            if f >= 2.0 {
                                CaptureKind::ReducedBitonal {
                                    factor: f.min(255.0) as u8,
                                }
                            } else {
                                CaptureKind::Bitonal
                            }
                        } else {
                            omr_imaging::classify_capture(&image)
                        };
                        Ok(DecodedPage {
                            image,
                            capture,
                            known_dpi: known,
                            source_dpi: src_dpi,
                        })
                    }
                    PageRaster::Jpeg(j) => {
                        let d = omr_imaging::decode(&j.data).map_err(|e| fail(e.to_string()))?;
                        let mut img = d.image;
                        if j.invert {
                            img = img.inverted();
                        }
                        let (w, h) = (img.width() as u32, img.height() as u32);
                        let g8 = omr_pdf::GrayImage8::from_raw(w, h, img.into_raw())
                            .ok_or_else(|| fail("JPEG size".into()))?;
                        let image = to_gray(g8.oriented(p.placement.orientation));
                        Ok(DecodedPage {
                            capture: omr_imaging::classify_capture(&image),
                            image,
                            known_dpi: src_dpi,
                            source_dpi: src_dpi,
                        })
                    }
                }
            }
        },
    }
}

/// Page identifier: the source's SHA-384 prefix and the 1-based page number (content-derived,
/// so it does not depend on the input order, §8.11).
pub fn page_id(sha384: &str, index: u32) -> String {
    format!("pg-{}-{:03}", &sha384[..16.min(sha384.len())], index + 1)
}
