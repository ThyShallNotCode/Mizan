//! Page enumeration and per-page scan-image extraction (spec §7.3, "PDF, one full-page image").
//!
//! For each page the content stream is walked (graphics-state stack, `cm`, `Do`, nested Form
//! XObjects) to find every image drawn and its placement matrix. The largest image must cover
//! most of the page; it is decoded natively (CCITT, Flate, raw) or passed through (DCT).
//! JBIG2 and mask/layered (MRC) pages are refused with CAP-004; anything else that cannot be
//! read as one full-page scan is refused with CAP-001. Every page is isolated: a failure or a
//! decoder panic on one page never affects another.

use crate::ccitt::{self, CcittParams};
use crate::flate::{self, PredictorParams};
use crate::raster::{BitImage, GrayImage8, JpegStream, Orientation, PageRaster};
use crate::reduce;
use crate::stream::{self, StreamError};
use crate::{PdfError, Refusal};
use hayro_syntax::Pdf;
use hayro_syntax::content::TypedIter;
use hayro_syntax::content::ops::TypedInstruction;
use hayro_syntax::object::{Array, Dict, Name, Object, Stream};
use hayro_syntax::page::{Page, Resources, Rotation};
use omr_model::Exception;
use serde::{Deserialize, Serialize};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

/// Input limits checked from headers before any allocation (spec §7.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Maximum pages per PDF (5,000).
    pub max_pages: usize,
    /// Maximum pixels per image side (20,000).
    pub max_side: u32,
    /// Maximum pixels per image (100 MP).
    pub max_pixels: u64,
    /// Maximum file size (2 GB).
    pub max_file_bytes: u64,
    /// Maximum decoded bytes of a page's content streams and Form XObjects together (64 MiB):
    /// a decompression bomb refuses the page (CAP-001) instead of exhausting memory.
    pub max_stream_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pages: 5_000,
            max_side: 20_000,
            max_pixels: 100_000_000,
            max_file_bytes: 2_000_000_000,
            max_stream_bytes: 64 << 20,
        }
    }
}

/// Extraction options.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractOptions {
    /// Header limits.
    pub limits: Limits,
    /// Maximum nesting of Form XObjects followed when looking for the page image.
    pub max_form_depth: u8,
    /// The page image must cover at least this fraction of the page box (else CAP-001: a
    /// vector or composite page that needs the rasterising worker, not in the MVP).
    pub min_page_coverage: f64,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            max_form_depth: 8,
            min_page_coverage: 0.5,
        }
    }
}

/// How the image was encoded in the PDF.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageEncoding {
    /// CCITTFaxDecode.
    Ccitt {
        /// "G4", "G3-1D" or "G3-2D".
        scheme: String,
        /// `/K`.
        k: i32,
        /// `/BlackIs1`.
        black_is_1: bool,
        /// `/EncodedByteAlign`.
        byte_align: bool,
        /// `/EndOfLine`.
        end_of_line: bool,
    },
    /// DCTDecode (JPEG), passed through.
    Dct,
    /// FlateDecode (with the predictor number).
    Flate {
        /// `/Predictor`.
        predictor: i32,
    },
    /// No compression, or only ASCII/RunLength filters.
    Raw,
}

/// The image's colour space as declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSpaceKind {
    /// DeviceGray, CalGray or a 1-component ICC profile.
    Gray,
    /// DeviceRGB, CalRGB or a 3-component ICC profile (converted to luma).
    Rgb,
    /// DeviceCMYK or a 4-component ICC profile (naive conversion to gray).
    Cmyk,
    /// Indexed over one of the above, with `hival + 1` entries.
    Indexed {
        /// Number of palette entries.
        entries: u16,
    },
    /// A stencil mask (`/ImageMask true`).
    StencilMask,
    /// Not declared (allowed for JPEG streams).
    Unspecified,
}

/// Where the image came from (provenance stored with the page, §7.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSource {
    /// 0-based page index in the PDF.
    pub page_index: u32,
    /// The XObject resource name (`Im0`, `Obj4`, …).
    pub xobject: String,
    /// Object number and generation of the image stream.
    pub object_id: Option<(i32, i32)>,
    /// The `/Filter` chain, outermost first.
    pub filters: Vec<String>,
    /// Encoding of the image data.
    pub encoding: ImageEncoding,
    /// `/Width`.
    pub width: u32,
    /// `/Height`.
    pub height: u32,
    /// `/BitsPerComponent` (1 for masks).
    pub bits_per_component: u8,
    /// Declared colour space.
    pub color_space: ColorSpaceKind,
    /// `/Decode` inverts the samples.
    pub decode_inverted: bool,
    /// Encoded stream length in bytes.
    pub stream_bytes: usize,
}

/// Where and how the image is placed on the page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// The CTM at `Do`: maps the image unit square to default user space (points).
    pub ctm: [f64; 6],
    /// The page box used (CropBox ∩ MediaBox), points: `[x0, y0, x1, y1]`.
    pub page_box: [f64; 4],
    /// `/Rotate` (0, 90, 180, 270).
    pub rotate: u16,
    /// `/UserUnit` (default 1).
    pub user_unit: f64,
    /// Effective resolution along the stored image's x axis, from the CTM.
    pub dpi_x: f64,
    /// Effective resolution along the stored image's y axis, from the CTM.
    pub dpi_y: f64,
    /// Resolution assuming the image fills the page box (stored image size ÷ page size in the
    /// displayed orientation): (x, y).
    pub page_box_dpi: (f64, f64),
    /// Fraction of the page box covered by the image.
    pub coverage: f64,
    /// Stored image → displayed page.
    pub orientation: Orientation,
    /// Residual rotation off the quarter turn, degrees (0 for axis-aligned placements).
    pub skew_degrees: f64,
}

impl Placement {
    /// The effective dpi snapped to a common scanner resolution when within 1.5 %, otherwise
    /// rounded. `None` when degenerate.
    pub fn nominal_dpi(&self) -> Option<u32> {
        let d = (self.dpi_x + self.dpi_y) / 2.0;
        if !d.is_finite() || d < 1.0 {
            return None;
        }
        const COMMON: [u32; 16] = [
            72, 75, 96, 100, 120, 150, 200, 240, 250, 300, 360, 400, 500, 600, 720, 1200,
        ];
        COMMON
            .iter()
            .copied()
            .find(|&c| (d - f64::from(c)).abs() <= 0.015 * f64::from(c))
            .or(Some(d.round() as u32))
    }
}

/// Facts noticed while extracting a page (kept with the page's provenance).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PageNote {
    /// The CCITT stream ended (end-of-block) before `/Height` rows; the rest is white.
    RowsPadded {
        /// Rows present in the stream.
        decoded: u32,
        /// Declared height.
        height: u32,
    },
    /// `/Columns` was absent; the image `/Width` was used.
    ColumnsDefaulted,
    /// The compressed stream was damaged (e.g. bad checksum) but yielded every pixel.
    DamagedStream,
    /// Smaller images drawn on the page were ignored (logos, stamps).
    ExtraImagesIgnored {
        /// How many.
        count: u32,
    },
    /// Vector painting operators were present and ignored.
    VectorContentIgnored {
        /// Number of painting operators.
        ops: u32,
    },
    /// An invisible text layer (scanner OCR, render mode 3) was present and ignored.
    InvisibleTextIgnored,
    /// Visible text was present and ignored.
    VisibleTextIgnored,
    /// The JPEG's SOF size differs from `/Width`/`/Height`; the SOF size is reported.
    SofSizeMismatch {
        /// Dictionary size.
        dict: (u32, u32),
        /// SOF size.
        sof: (u32, u32),
    },
    /// The placement is rotated off the quarter turn by this many degrees.
    SkewedPlacement {
        /// Degrees.
        degrees: f64,
    },
}

/// One page's extracted scan image.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedPage {
    /// 0-based page index.
    pub index: u32,
    /// The image in its stored orientation.
    pub raster: PageRaster,
    /// Provenance of the image stream.
    pub source: ImageSource,
    /// Placement, effective dpi and orientation.
    pub placement: Placement,
    /// Notes.
    pub notes: Vec<PageNote>,
}

impl ExtractedPage {
    /// Effective dpi snapped to a common value.
    pub fn nominal_dpi(&self) -> Option<u32> {
        self.placement.nominal_dpi()
    }

    /// 8-bit coverage gray at `dst_dpi` in the **stored** orientation (no rotation). Bilevel
    /// pages use [`reduce::reduce_bilevel_to_dpi`]; gray pages a box reduction for integer
    /// ratios (other ratios are returned at native resolution). `None` for JPEG pages, which
    /// `omr-imaging` decodes.
    pub fn reduced(&self, dst_dpi: u32) -> Result<Option<GrayImage8>, PdfError> {
        let Some(src) = self.nominal_dpi() else {
            return Err(PdfError::Unsupported(
                "degenerate placement (no dpi)".into(),
            ));
        };
        match &self.raster {
            PageRaster::Bilevel(b) => reduce::reduce_bilevel_to_dpi(b, src, dst_dpi).map(Some),
            PageRaster::Gray(g) => {
                if src > dst_dpi && src % dst_dpi == 0 {
                    reduce::reduce_gray(g, src / dst_dpi).map(Some)
                } else {
                    Ok(Some(g.clone()))
                }
            }
            PageRaster::Jpeg(_) => Ok(None),
        }
    }

    /// The working image: reduced to `dst_dpi`, then turned into the displayed page orientation.
    pub fn working_gray(&self, dst_dpi: u32) -> Result<Option<GrayImage8>, PdfError> {
        Ok(self
            .reduced(dst_dpi)?
            .map(|g| g.oriented(self.placement.orientation)))
    }
}

/// A page that was not extracted, with its Appendix A exception.
#[derive(Clone, Debug, PartialEq)]
pub struct RefusedPage {
    /// 0-based page index.
    pub index: u32,
    /// Why.
    pub reason: Refusal,
    /// CAP-004 (JBIG2, MRC) or CAP-001 (everything else).
    pub exception: Exception,
}

/// Per-page result.
#[derive(Clone, Debug, PartialEq)]
pub enum PageOutcome {
    /// The page's scan image.
    Extracted(Box<ExtractedPage>),
    /// The page was refused.
    Refused(RefusedPage),
}

impl PageOutcome {
    /// The extracted page, if any.
    pub fn extracted(&self) -> Option<&ExtractedPage> {
        match self {
            PageOutcome::Extracted(p) => Some(p),
            PageOutcome::Refused(_) => None,
        }
    }

    /// The refusal, if any.
    pub fn refused(&self) -> Option<&RefusedPage> {
        match self {
            PageOutcome::Refused(r) => Some(r),
            PageOutcome::Extracted(_) => None,
        }
    }
}

/// `Arc<[u8]>` adaptor so the parser can share the caller's buffer without copying.
struct SharedBytes(Arc<[u8]>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// An opened PDF. Parsing is lazy: objects are read when a page asks for them.
pub struct PdfDocument {
    pdf: Pdf,
    opts: ExtractOptions,
    page_count: usize,
}

impl std::fmt::Debug for PdfDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PdfDocument({} pages)", self.page_count)
    }
}

impl PdfDocument {
    /// Opens PDF bytes (no file I/O). Document-level failures (empty, not a PDF, encrypted with
    /// a user password, too many pages) are errors; their [`PdfError::exception`] is CAP-001.
    pub fn open(bytes: impl Into<Arc<[u8]>>, opts: ExtractOptions) -> Result<Self, PdfError> {
        Self::open_with_password(bytes, "", opts)
    }

    /// Opens an encrypted PDF with the password the operator typed (never brute-forced, §7.3).
    pub fn open_with_password(
        bytes: impl Into<Arc<[u8]>>,
        password: &str,
        opts: ExtractOptions,
    ) -> Result<Self, PdfError> {
        let bytes: Arc<[u8]> = bytes.into();
        if bytes.is_empty() {
            return Err(PdfError::Empty);
        }
        if bytes.len() as u64 > opts.limits.max_file_bytes {
            return Err(PdfError::FileTooLarge(bytes.len() as u64));
        }
        if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
            return Err(PdfError::Invalid(
                "no %PDF- header in the first 1 KiB".into(),
            ));
        }
        let data = Arc::new(SharedBytes(bytes));
        let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(data, password)))
            .map_err(|_| PdfError::Invalid("parser panic".into()))?
            .map_err(|e| match e {
                hayro_syntax::LoadPdfError::Decryption(_) => PdfError::PasswordRequired,
                hayro_syntax::LoadPdfError::Invalid => {
                    PdfError::Invalid("unreadable cross-reference or page tree".into())
                }
            })?;
        let page_count = pdf.pages().len();
        if page_count == 0 {
            return Err(PdfError::Invalid("no pages".into()));
        }
        if page_count > opts.limits.max_pages {
            return Err(PdfError::TooManyPages(page_count));
        }
        Ok(Self {
            pdf,
            opts,
            page_count,
        })
    }

    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// Extracts one page (0-based). Never panics; failures are [`PageOutcome::Refused`].
    pub fn extract_page(&self, index: usize) -> PageOutcome {
        let idx = index as u32;
        if index >= self.page_count {
            return refused(idx, Refusal::Error(PdfError::PageOutOfRange(index)));
        }
        let res = catch_unwind(AssertUnwindSafe(|| {
            let page = &self.pdf.pages()[index];
            extract(page, idx, &self.opts)
        }));
        match res {
            Ok(Ok(p)) => PageOutcome::Extracted(Box::new(p)),
            Ok(Err(r)) => refused(idx, r),
            Err(_) => refused(
                idx,
                Refusal::Error(PdfError::Corrupt("decoder panic (page isolated)".into())),
            ),
        }
    }

    /// Extracts every page in order.
    pub fn extract_all(&self) -> Vec<PageOutcome> {
        (0..self.page_count).map(|i| self.extract_page(i)).collect()
    }
}

fn refused(index: u32, reason: Refusal) -> PageOutcome {
    let exception = reason.exception(index);
    PageOutcome::Refused(RefusedPage {
        index,
        reason,
        exception,
    })
}

// ---------------------------------------------------------------------------------------------
// Content walk
// ---------------------------------------------------------------------------------------------

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m1 × m2` (apply `m1` first).
fn mul(m1: &Matrix, m2: &Matrix) -> Matrix {
    [
        m1[0] * m2[0] + m1[1] * m2[2],
        m1[0] * m2[1] + m1[1] * m2[3],
        m1[2] * m2[0] + m1[3] * m2[2],
        m1[2] * m2[1] + m1[3] * m2[3],
        m1[4] * m2[0] + m1[5] * m2[2] + m2[4],
        m1[4] * m2[1] + m1[5] * m2[3] + m2[5],
    ]
}

struct Drawn<'a> {
    name: String,
    stream: Stream<'a>,
    ctm: Matrix,
}

#[derive(Default)]
struct Walk<'a> {
    images: Vec<Drawn<'a>>,
    inline_images: u32,
    vector_ops: u32,
    invisible_text: bool,
    visible_text: bool,
    forms_visited: Vec<(i32, i32)>,
    /// Decoded content bytes still allowed on this page.
    budget: usize,
    /// A Form XObject exceeded the budget (the page is refused).
    bomb: Option<StreamError>,
}

impl<'a> Walk<'a> {
    fn run(&mut self, content: &[u8], resources: &Resources<'a>, base: Matrix, depth: u8, max: u8) {
        let mut ops = TypedIter::new(content);
        let mut stack: Vec<(Matrix, i32)> = Vec::new();
        let mut ctm = base;
        let mut tr = 0i32;
        while let Some(op) = ops.next() {
            if self.bomb.is_some() {
                return;
            }
            match op {
                TypedInstruction::SaveState(_) => {
                    if stack.len() < 256 {
                        stack.push((ctm, tr));
                    }
                }
                TypedInstruction::RestoreState(_) => {
                    if let Some((m, t)) = stack.pop() {
                        ctm = m;
                        tr = t;
                    }
                }
                TypedInstruction::Transform(t) => {
                    let m = [
                        t.0.as_f64(),
                        t.1.as_f64(),
                        t.2.as_f64(),
                        t.3.as_f64(),
                        t.4.as_f64(),
                        t.5.as_f64(),
                    ];
                    ctm = mul(&m, &ctm);
                }
                TypedInstruction::TextRenderingMode(m) => tr = m.0.as_f64() as i32,
                TypedInstruction::ShowText(_)
                | TypedInstruction::ShowTexts(_)
                | TypedInstruction::NextLineAndShowText(_)
                | TypedInstruction::ShowTextWithParameters(_) => {
                    if tr == 3 || tr == 7 {
                        self.invisible_text = true;
                    } else {
                        self.visible_text = true;
                    }
                }
                TypedInstruction::StrokePath(_)
                | TypedInstruction::CloseAndStrokePath(_)
                | TypedInstruction::FillPathNonZero(_)
                | TypedInstruction::FillPathNonZeroCompatibility(_)
                | TypedInstruction::FillPathEvenOdd(_)
                | TypedInstruction::FillAndStrokeNonZero(_)
                | TypedInstruction::FillAndStrokeEvenOdd(_)
                | TypedInstruction::CloseFillAndStrokeNonZero(_)
                | TypedInstruction::CloseFillAndStrokeEvenOdd(_)
                | TypedInstruction::Shading(_) => self.vector_ops += 1,
                TypedInstruction::InlineImage(_) => self.inline_images += 1,
                TypedInstruction::XObject(x) => {
                    let name: &Name<'_> = x.0;
                    let Some(stream) = resources.get_x_object(name) else {
                        continue;
                    };
                    let dict = stream.dict().clone();
                    match dict.get::<Name<'_>>(b"Subtype").as_deref() {
                        Some(b"Image") => self.images.push(Drawn {
                            name: name.as_str().to_owned(),
                            stream,
                            ctm,
                        }),
                        Some(b"Form") => {
                            if depth >= max {
                                continue;
                            }
                            let id = stream.obj_id();
                            let key = (id.obj_number, id.gen_number);
                            if self.forms_visited.contains(&key) {
                                continue;
                            }
                            self.forms_visited.push(key);
                            let fm = dict
                                .get::<Array<'_>>(b"Matrix")
                                .and_then(|a| matrix_from(&a))
                                .unwrap_or(IDENTITY);
                            let res = dict
                                .get::<Dict<'a>>(b"Resources")
                                .map(Resources::new)
                                .unwrap_or_else(|| resources.clone());
                            match decode_bounded(&stream, self.budget) {
                                Ok(data) => {
                                    self.budget -= data.len().min(self.budget);
                                    self.run(&data, &res, mul(&fm, &ctm), depth + 1, max);
                                }
                                Err(e @ StreamError::Bound(_)) => self.bomb = Some(e),
                                // An undecodable form is skipped, as a viewer would.
                                Err(_) => {}
                            }
                            self.forms_visited.pop();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

/// Decodes a stream's whole filter chain with an output bound (never `Stream::decoded()`,
/// which is unbounded).
fn decode_bounded(s: &Stream<'_>, limit: usize) -> Result<Vec<u8>, StreamError> {
    let filters = filter_names(s.dict());
    let parms = decode_parms(s.dict(), filters.len());
    stream::decode_filters(&s.raw_data(), &filters, &parms, limit).map(|d| d.data)
}

/// The page's content streams, concatenated, within `budget` decoded bytes. An undecodable
/// stream is skipped (as a viewer would); one over the bound refuses the page.
fn page_content(page: &Page<'_>, budget: &mut usize) -> Result<Vec<u8>, Refusal> {
    let dict = page.raw();
    let streams: Vec<Stream<'_>> = if let Some(s) = dict.get::<Stream<'_>>(b"Contents") {
        vec![s]
    } else if let Some(a) = dict.get::<Array<'_>>(b"Contents") {
        a.iter::<Stream<'_>>().collect()
    } else {
        Vec::new()
    };
    let mut out = Vec::new();
    for s in &streams {
        match decode_bounded(s, *budget) {
            Ok(d) => {
                *budget -= d.len().min(*budget);
                out.extend_from_slice(&d);
                out.push(b' ');
            }
            Err(e @ StreamError::Bound(_)) => return Err(Refusal::Error(e.into())),
            Err(_) => {}
        }
    }
    Ok(out)
}

fn matrix_from(a: &Array<'_>) -> Option<Matrix> {
    let v: Vec<f64> = a.iter::<f64>().collect();
    (v.len() == 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
}

// ---------------------------------------------------------------------------------------------
// Page extraction
// ---------------------------------------------------------------------------------------------

fn extract(page: &Page<'_>, index: u32, opts: &ExtractOptions) -> Result<ExtractedPage, Refusal> {
    let mut budget = opts.limits.max_stream_bytes;
    let content = page_content(page, &mut budget)?;
    let mut walk = Walk {
        budget,
        ..Walk::default()
    };
    walk.run(&content, page.resources(), IDENTITY, 0, opts.max_form_depth);
    if let Some(e) = walk.bomb.take() {
        return Err(Refusal::Error(e.into()));
    }

    // JBIG2 anywhere on a scan page refuses the page (lossy symbol substitution, §7.2).
    for d in &walk.images {
        let filters = filter_names(d.stream.dict());
        if filters.iter().any(|f| f == "JBIG2Decode") {
            let globals = decode_parms(d.stream.dict(), filters.len())
                .last()
                .and_then(|p| p.as_ref().map(|p| p.contains_key(b"JBIG2Globals")))
                .unwrap_or(false);
            return Err(Refusal::Jbig2 { globals });
        }
    }

    let pbox = page.intersected_crop_box();
    let user_unit = page
        .raw()
        .get::<f64>(b"UserUnit")
        .filter(|u| u.is_finite() && *u > 0.0)
        .unwrap_or(1.0);
    let page_area = (pbox.width() * pbox.height()).abs();
    let area = |m: &Matrix| (m[0] * m[3] - m[1] * m[2]).abs();

    let Some(main_idx) = (0..walk.images.len()).max_by(|&a, &b| {
        area(&walk.images[a].ctm)
            .partial_cmp(&area(&walk.images[b].ctm))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.cmp(&a))
    }) else {
        let what = if walk.inline_images > 0 {
            "only inline images (not a scanner page image)"
        } else {
            "no image XObject: vector or born-digital page (rasterising worker not in the MVP)"
        };
        return Err(Refusal::Error(PdfError::Unsupported(what.into())));
    };
    let main = &walk.images[main_idx];
    let coverage = if page_area > 0.0 {
        area(&main.ctm) / page_area
    } else {
        0.0
    };

    // MRC / layered pages: a soft or stencil mask on the page image, a stencil-mask page image,
    // or a second large image overlapping it.
    let md = main.stream.dict();
    let has_mask_stream =
        md.get::<Stream<'_>>(b"SMask").is_some() || md.get::<Stream<'_>>(b"Mask").is_some();
    let large_others = walk
        .images
        .iter()
        .enumerate()
        .filter(|(i, d)| *i != main_idx && page_area > 0.0 && area(&d.ctm) / page_area >= 0.25)
        .count();
    let is_stencil = md.get::<bool>(b"ImageMask").unwrap_or(false);
    if has_mask_stream || large_others > 0 || (is_stencil && walk.images.len() > 1) {
        return Err(Refusal::Mrc);
    }
    if coverage < opts.min_page_coverage {
        return Err(Refusal::Error(PdfError::Unsupported(format!(
            "page image covers {:.0} % of the page (composite page; rasterising worker not in the MVP)",
            coverage * 100.0
        ))));
    }

    let mut notes = Vec::new();
    let extra = walk.images.len() as u32 - 1;
    if extra > 0 {
        notes.push(PageNote::ExtraImagesIgnored { count: extra });
    }
    if walk.vector_ops > 0 {
        notes.push(PageNote::VectorContentIgnored {
            ops: walk.vector_ops,
        });
    }
    if walk.invisible_text {
        notes.push(PageNote::InvisibleTextIgnored);
    }
    if walk.visible_text {
        notes.push(PageNote::VisibleTextIgnored);
    }

    let (raster, source) = decode_image(page, main, index, &opts.limits, &mut notes)?;
    let placement = placement(page, &main.ctm, &raster, user_unit, coverage);
    if placement.skew_degrees.abs() > 0.05 {
        notes.push(PageNote::SkewedPlacement {
            degrees: placement.skew_degrees,
        });
    }
    Ok(ExtractedPage {
        index,
        raster,
        source,
        placement,
        notes,
    })
}

fn placement(
    page: &Page<'_>,
    ctm: &Matrix,
    raster: &PageRaster,
    user_unit: f64,
    coverage: f64,
) -> Placement {
    let (w, h) = (f64::from(raster.width()), f64::from(raster.height()));
    let rotate: u16 = match page.rotation() {
        Rotation::None => 0,
        Rotation::Horizontal => 90,
        Rotation::Flipped => 180,
        Rotation::FlippedHorizontal => 270,
    };
    let pts_per_inch = 72.0 / user_unit;
    let len_x = ctm[0].hypot(ctm[1]);
    let len_y = ctm[2].hypot(ctm[3]);
    let dpi_x = if len_x > 0.0 {
        w * pts_per_inch / len_x
    } else {
        0.0
    };
    let dpi_y = if len_y > 0.0 {
        h * pts_per_inch / len_y
    } else {
        0.0
    };
    // Image axes in display space (y down, page rotated clockwise by /Rotate).
    let to_display = |v: (f64, f64)| {
        let mut p = (v.0, -v.1);
        for _ in 0..(rotate / 90) {
            p = (-p.1, p.0);
        }
        p
    };
    let col = to_display((ctm[0], ctm[1]));
    let row = to_display((-ctm[2], -ctm[3]));
    let norm = |v: (f64, f64)| {
        let l = v.0.hypot(v.1);
        if l > 0.0 {
            (v.0 / l, v.1 / l)
        } else {
            (0.0, 0.0)
        }
    };
    let (col, row) = (norm(col), norm(row));
    let mut best = (Orientation::UPRIGHT, f64::MIN);
    for q in 0..4u8 {
        for m in [false, true] {
            let mut c = (1.0, 0.0);
            let mut r = (0.0, 1.0);
            for _ in 0..q {
                c = (-c.1, c.0);
                r = (-r.1, r.0);
            }
            if m {
                c.0 = -c.0;
                r.0 = -r.0;
            }
            let score = col.0 * c.0 + col.1 * c.1 + row.0 * r.0 + row.1 * r.1;
            if score > best.1 + 1e-9 {
                best = (
                    Orientation {
                        quarter_turns_cw: q,
                        mirrored: m,
                    },
                    score,
                );
            }
        }
    }
    let orientation = best.0;
    let expected_col = {
        let mut c = (1.0f64, 0.0f64);
        for _ in 0..orientation.quarter_turns_cw {
            c = (-c.1, c.0);
        }
        if orientation.mirrored {
            c.0 = -c.0;
        }
        c
    };
    let cross = expected_col.0 * col.1 - expected_col.1 * col.0;
    let dot = expected_col.0 * col.0 + expected_col.1 * col.1;
    let skew_degrees = cross.atan2(dot).to_degrees();

    let pb = page.intersected_crop_box();
    let (pw, ph) = if rotate % 180 == 90 {
        (pb.height(), pb.width())
    } else {
        (pb.width(), pb.height())
    };
    let (dw, dh) = if orientation.quarter_turns_cw % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    let page_box_dpi = (
        if pw > 0.0 {
            dw * pts_per_inch / pw
        } else {
            0.0
        },
        if ph > 0.0 {
            dh * pts_per_inch / ph
        } else {
            0.0
        },
    );
    Placement {
        ctm: *ctm,
        page_box: [pb.x0, pb.y0, pb.x1, pb.y1],
        rotate,
        user_unit,
        dpi_x,
        dpi_y,
        page_box_dpi,
        coverage,
        orientation,
        skew_degrees,
    }
}

fn filter_names(d: &Dict<'_>) -> Vec<String> {
    let canon = |n: &str| -> String {
        match n {
            "Fl" => "FlateDecode",
            "LZW" => "LZWDecode",
            "AHx" => "ASCIIHexDecode",
            "A85" => "ASCII85Decode",
            "RL" => "RunLengthDecode",
            "CCF" => "CCITTFaxDecode",
            "DCT" => "DCTDecode",
            other => other,
        }
        .to_owned()
    };
    if let Some(n) = d.get::<Name<'_>>(b"Filter") {
        vec![canon(n.as_str())]
    } else if let Some(a) = d.get::<Array<'_>>(b"Filter") {
        a.iter::<Name<'_>>().map(|n| canon(n.as_str())).collect()
    } else {
        Vec::new()
    }
}

/// `/DecodeParms` per filter: an array of dicts/nulls, or one dict that applies to the first
/// filter (as pdf.js reads it).
fn decode_parms<'a>(d: &Dict<'a>, n: usize) -> Vec<Option<Dict<'a>>> {
    let mut out: Vec<Option<Dict<'a>>> = vec![None; n.max(1)];
    if let Some(p) = d.get::<Dict<'a>>(b"DecodeParms") {
        out[0] = Some(p);
    } else if let Some(a) = d.get::<Array<'a>>(b"DecodeParms") {
        for (slot, o) in out.iter_mut().zip(a.iter::<Object<'a>>()) {
            *slot = o.into_dict();
        }
    }
    out.truncate(n.max(1));
    out
}

struct ColorInfo {
    kind: ColorSpaceKind,
    components: u8,
    /// Gray value (0–255) per palette index, for Indexed.
    palette: Option<Vec<u8>>,
}

fn color_space(page: &Page<'_>, obj: Option<Object<'_>>, depth: u8) -> Result<ColorInfo, PdfError> {
    let gray = ColorInfo {
        kind: ColorSpaceKind::Gray,
        components: 1,
        palette: None,
    };
    let Some(obj) = obj else {
        return Ok(ColorInfo {
            kind: ColorSpaceKind::Unspecified,
            components: 0,
            palette: None,
        });
    };
    if depth > 4 {
        return Err(PdfError::Unsupported(
            "colour space nesting too deep".into(),
        ));
    }
    match obj {
        Object::Name(n) => match n.as_str() {
            "DeviceGray" | "G" | "CalGray" => Ok(gray),
            "DeviceRGB" | "RGB" | "CalRGB" => Ok(ColorInfo {
                kind: ColorSpaceKind::Rgb,
                components: 3,
                palette: None,
            }),
            "DeviceCMYK" | "CMYK" => Ok(ColorInfo {
                kind: ColorSpaceKind::Cmyk,
                components: 4,
                palette: None,
            }),
            other => match page.resources().get_color_space(&n) {
                Some(o) => color_space(page, Some(o), depth + 1),
                None => Err(PdfError::Unsupported(format!("colour space {other}"))),
            },
        },
        Object::Array(a) => {
            let items: Vec<Object<'_>> = a.iter::<Object<'_>>().collect();
            let family = items
                .first()
                .and_then(|o| o.clone().into_name())
                .map(|n| n.as_str().to_owned())
                .unwrap_or_default();
            match family.as_str() {
                "CalGray" => Ok(gray),
                "CalRGB" => Ok(ColorInfo {
                    kind: ColorSpaceKind::Rgb,
                    components: 3,
                    palette: None,
                }),
                "ICCBased" => {
                    let n = items
                        .get(1)
                        .and_then(|o| o.clone().into_stream())
                        .and_then(|s| s.dict().get::<u32>(b"N"))
                        .unwrap_or(0);
                    match n {
                        1 => Ok(gray),
                        3 => Ok(ColorInfo {
                            kind: ColorSpaceKind::Rgb,
                            components: 3,
                            palette: None,
                        }),
                        4 => Ok(ColorInfo {
                            kind: ColorSpaceKind::Cmyk,
                            components: 4,
                            palette: None,
                        }),
                        _ => Err(PdfError::Unsupported(format!("ICCBased with N = {n}"))),
                    }
                }
                "Indexed" | "I" => {
                    let base = color_space(page, items.get(1).cloned(), depth + 1)?;
                    let hival = items
                        .get(2)
                        .and_then(|o| o.clone().into_number())
                        .map(|n| n.as_f64() as i64)
                        .unwrap_or(-1);
                    if !(0..=255).contains(&hival) || base.palette.is_some() || base.components == 0
                    {
                        return Err(PdfError::Unsupported(
                            "malformed Indexed colour space".into(),
                        ));
                    }
                    let lookup: Vec<u8> = match items.get(3) {
                        Some(Object::String(s)) => s.as_bytes().to_vec(),
                        Some(Object::Stream(s)) => decode_bounded(s, 1 << 16)
                            .map_err(|_| PdfError::Corrupt("Indexed lookup stream".into()))?,
                        _ => return Err(PdfError::Unsupported("Indexed lookup missing".into())),
                    };
                    let nc = base.components as usize;
                    let entries = hival as usize + 1;
                    if lookup.len() < entries * nc {
                        return Err(PdfError::Corrupt("Indexed lookup too short".into()));
                    }
                    let palette = (0..entries)
                        .map(|i| to_gray(&lookup[i * nc..(i + 1) * nc], &base.kind))
                        .collect();
                    Ok(ColorInfo {
                        kind: ColorSpaceKind::Indexed {
                            entries: entries as u16,
                        },
                        components: 1,
                        palette: Some(palette),
                    })
                }
                other => Err(PdfError::Unsupported(format!("colour space {other}"))),
            }
        }
        _ => Err(PdfError::Unsupported("colour space object".into())),
    }
}

/// One pixel's components (8-bit) to gray: BT.601 luma for RGB, naive for CMYK.
fn to_gray(c: &[u8], kind: &ColorSpaceKind) -> u8 {
    match kind {
        ColorSpaceKind::Rgb => {
            ((299 * u32::from(c[0]) + 587 * u32::from(c[1]) + 114 * u32::from(c[2]) + 500) / 1000)
                as u8
        }
        ColorSpaceKind::Cmyk => {
            let ink = (30 * u32::from(c[0]) + 59 * u32::from(c[1]) + 11 * u32::from(c[2])) / 100
                + u32::from(c[3]);
            255 - ink.min(255) as u8
        }
        _ => c[0],
    }
}

fn decode_image(
    page: &Page<'_>,
    d: &Drawn<'_>,
    index: u32,
    limits: &Limits,
    notes: &mut Vec<PageNote>,
) -> Result<(PageRaster, ImageSource), Refusal> {
    let dict = d.stream.dict();
    let width = dict.get::<u32>(b"Width").unwrap_or(0);
    let height = dict.get::<u32>(b"Height").unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(PdfError::Corrupt("image without /Width or /Height".into()).into());
    }
    if width > limits.max_side
        || height > limits.max_side
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        return Err(PdfError::TooLarge { width, height }.into());
    }
    let image_mask = dict.get::<bool>(b"ImageMask").unwrap_or(false);
    let bpc: u8 = if image_mask {
        1
    } else {
        dict.get::<u8>(b"BitsPerComponent").unwrap_or(0)
    };
    let filters = filter_names(dict);
    let parms = decode_parms(dict, filters.len());
    let image_filter = filters
        .last()
        .filter(|f| matches!(f.as_str(), "CCITTFaxDecode" | "DCTDecode" | "JPXDecode"))
        .cloned();
    if filters.iter().any(|f| f == "JPXDecode") {
        return Err(PdfError::Unsupported("JPXDecode (JPEG 2000) page image".into()).into());
    }
    if filters.iter().any(|f| f == "LZWDecode" || f == "Crypt") {
        return Err(PdfError::Unsupported(format!("filter chain {filters:?}")).into());
    }
    if let Some(unknown) = filters.iter().find(|f| {
        !matches!(
            f.as_str(),
            "FlateDecode"
                | "ASCIIHexDecode"
                | "ASCII85Decode"
                | "RunLengthDecode"
                | "CCITTFaxDecode"
                | "DCTDecode"
        )
    }) {
        return Err(PdfError::Unsupported(format!("filter {unknown}")).into());
    }
    let cs = if image_mask {
        ColorInfo {
            kind: ColorSpaceKind::StencilMask,
            components: 1,
            palette: None,
        }
    } else {
        color_space(page, dict.get::<Object<'_>>(b"ColorSpace"), 0)?
    };
    let decode: Vec<f64> = dict
        .get::<Array<'_>>(b"Decode")
        .map(|a| a.iter::<f64>().collect())
        .unwrap_or_default();
    let decode_inverted = decode.len() >= 2 && decode[0] > decode[1];
    let id = d.stream.obj_id();
    let raw = d.stream.raw_data();
    let mut source = ImageSource {
        page_index: index,
        xobject: d.name.clone(),
        object_id: Some((id.obj_number, id.gen_number)),
        filters: filters.clone(),
        encoding: ImageEncoding::Raw,
        width,
        height,
        bits_per_component: bpc,
        color_space: cs.kind.clone(),
        decode_inverted,
        stream_bytes: raw.len(),
    };

    // Pre-filters before an image filter: only FlateDecode is supported (bounded).
    let pre: &[String] = match &image_filter {
        Some(_) => &filters[..filters.len() - 1],
        None => &filters[..],
    };
    let pixels = u64::from(width) * u64::from(height);

    match image_filter.as_deref() {
        Some("CCITTFaxDecode") => {
            let data = prefiltered(
                &raw,
                pre,
                &parms[..pre.len()],
                (pixels / 2 + (1 << 20)) as usize,
                notes,
            )?;
            let p = parms.last().cloned().flatten();
            let get_i = |k: &[u8]| p.as_ref().and_then(|p| p.get::<i32>(k));
            let get_b = |k: &[u8]| p.as_ref().and_then(|p| p.get::<bool>(k));
            let columns = match get_i(b"Columns") {
                Some(c) if c > 0 => c as u32,
                _ => {
                    notes.push(PageNote::ColumnsDefaulted);
                    width
                }
            };
            let params = CcittParams {
                k: get_i(b"K").unwrap_or(0),
                columns,
                rows: get_i(b"Rows")
                    .filter(|r| *r > 0)
                    .map_or(height, |r| r as u32),
                end_of_line: get_b(b"EndOfLine").unwrap_or(false),
                encoded_byte_align: get_b(b"EncodedByteAlign").unwrap_or(false),
                end_of_block: get_b(b"EndOfBlock").unwrap_or(true),
                black_is_1: get_b(b"BlackIs1").unwrap_or(false),
            };
            if !image_mask && bpc != 1 && bpc != 0 {
                return Err(PdfError::Corrupt(format!(
                    "CCITT image with {bpc} bits per component"
                ))
                .into());
            }
            source.bits_per_component = 1;
            source.encoding = ImageEncoding::Ccitt {
                scheme: params.scheme().to_owned(),
                k: params.k,
                black_is_1: params.black_is_1,
                byte_align: params.encoded_byte_align,
                end_of_line: params.end_of_line,
            };
            let dec = ccitt::decode(&data, width, height, &params)?;
            if dec.rows_decoded < height {
                notes.push(PageNote::RowsPadded {
                    decoded: dec.rows_decoded,
                    height,
                });
            }
            let mut img = dec.image;
            // Sample values of CCITT black and white pixels after /BlackIs1 and /Decode; a
            // sample shows as ink when it is 0 (DeviceGray black, stencil "paint") or when its
            // palette entry is dark.
            let v_black = u8::from(params.black_is_1) ^ u8::from(decode_inverted);
            let is_ink = |v: u8| match &cs.palette {
                Some(pal) => pal.get(usize::from(v)).copied().unwrap_or(255) < 128,
                None => v == 0,
            };
            match (is_ink(v_black), is_ink(1 - v_black)) {
                (true, false) => {}
                (false, true) => img.invert(),
                (true, true) => img = BitImage::from_fn(width, height, |_, _| true)?,
                (false, false) => img = BitImage::new(width, height)?,
            }
            Ok((PageRaster::Bilevel(img), source))
        }
        Some("DCTDecode") => {
            let data = prefiltered(
                &raw,
                pre,
                &parms[..pre.len()],
                (pixels * 4 + (1 << 20)) as usize,
                notes,
            )?;
            source.encoding = ImageEncoding::Dct;
            let info = jpeg_info(&data)?;
            let (jw, jh) = (
                info.width,
                if info.height == 0 {
                    height
                } else {
                    info.height
                },
            );
            if (jw, jh) != (width, height) {
                notes.push(PageNote::SofSizeMismatch {
                    dict: (width, height),
                    sof: (jw, jh),
                });
            }
            Ok((
                PageRaster::Jpeg(JpegStream {
                    data: data.into_owned(),
                    width: jw,
                    height: jh,
                    components: info.components,
                    progressive: info.progressive,
                    invert: decode_inverted,
                    adobe_transform: info.adobe_transform,
                }),
                source,
            ))
        }
        _ => {
            // Uncompressed samples, possibly behind Flate / ASCII / RunLength filters.
            if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
                return Err(PdfError::Corrupt(format!("{bpc} bits per component")).into());
            }
            let nc = usize::from(cs.components.max(1));
            let row_bytes = (width as usize * nc * usize::from(bpc)).div_ceil(8);
            let expected = row_bytes * height as usize;
            let samples: Vec<u8> = if filters.is_empty() {
                raw.to_vec()
            } else if filters.len() == 1 && filters[0] == "FlateDecode" {
                let p = parms.first().cloned().flatten();
                let pp = PredictorParams {
                    predictor: p
                        .as_ref()
                        .and_then(|p| p.get::<i32>(b"Predictor"))
                        .unwrap_or(1),
                    colors: p
                        .as_ref()
                        .and_then(|p| p.get::<u32>(b"Colors"))
                        .unwrap_or(1),
                    bits_per_component: p
                        .as_ref()
                        .and_then(|p| p.get::<u32>(b"BitsPerComponent"))
                        .unwrap_or(8),
                    columns: p
                        .as_ref()
                        .and_then(|p| p.get::<u32>(b"Columns"))
                        .unwrap_or(1),
                };
                source.encoding = ImageEncoding::Flate {
                    predictor: pp.predictor,
                };
                let out = flate::inflate(&raw, expected, &pp)?;
                if out.damaged {
                    notes.push(PageNote::DamagedStream);
                }
                out.data
            } else {
                if filters.iter().any(|f| f == "FlateDecode") {
                    source.encoding = ImageEncoding::Flate { predictor: 1 };
                }
                // Every stage is bounded (decompression bombs refuse the page, CAP-001).
                let limit = expected
                    .saturating_add(expected / 2)
                    .saturating_add(1 << 16);
                let out = stream::decode_filters(&raw, &filters, &parms, limit)
                    .map_err(PdfError::from)?;
                if out.damaged {
                    notes.push(PageNote::DamagedStream);
                }
                out.data
            };
            if samples.len() < expected {
                return Err(PdfError::Corrupt(format!(
                    "image data has {} of {expected} bytes",
                    samples.len()
                ))
                .into());
            }
            let raster = unpack(&samples[..expected], width, height, bpc, &cs, &decode)?;
            Ok((raster, source))
        }
    }
}

/// Applies the pre-filters in front of an image filter (Flate, ASCII, RunLength), each stage
/// bounded by `limit` bytes.
fn prefiltered<'b>(
    raw: &'b [u8],
    pre: &[String],
    parms: &[Option<Dict<'_>>],
    limit: usize,
    notes: &mut Vec<PageNote>,
) -> Result<std::borrow::Cow<'b, [u8]>, PdfError> {
    if pre.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(raw));
    }
    let out = stream::decode_filters(raw, pre, parms, limit).map_err(|e| match e {
        StreamError::Bound(_) => {
            PdfError::Corrupt("pre-filter output exceeds the image bound".into())
        }
        other => PdfError::from(other),
    })?;
    if out.damaged {
        notes.push(PageNote::DamagedStream);
    }
    Ok(std::borrow::Cow::Owned(out.data))
}

/// Unpacks raw samples to a bilevel or gray raster, applying `/Decode`.
fn unpack(
    s: &[u8],
    width: u32,
    height: u32,
    bpc: u8,
    cs: &ColorInfo,
    decode: &[f64],
) -> Result<PageRaster, PdfError> {
    let (w, h) = (width as usize, height as usize);
    let nc = usize::from(cs.components.max(1));
    let row_bytes = (w * nc * usize::from(bpc)).div_ceil(8);
    let inverted = decode.len() >= 2 && decode[0] > decode[1];
    let one_channel = nc == 1;

    // 1-bit gray or stencil mask → bilevel. Sample 0 (after /Decode) is black / painted.
    if bpc == 1 && one_channel && cs.palette.is_none() {
        let mut img = BitImage::new(width, height)?;
        for y in 0..h {
            let src = &s[y * row_bytes..(y + 1) * row_bytes];
            let dst = img.row_mut(y as u32);
            for (d, &b) in dst.iter_mut().zip(src) {
                *d = if inverted { b } else { !b };
            }
        }
        img.clear_padding();
        return Ok(PageRaster::Bilevel(img));
    }

    // Fast paths: 8-bit gray with a default or inverted /Decode, and 8-bit RGB.
    let default_decode = decode.len() < 2
        || (decode[0] == 0.0 && decode[1] == 1.0)
        || (decode[0] == 1.0 && decode[1] == 0.0);
    if bpc == 8 && one_channel && cs.palette.is_none() && default_decode {
        let mut data = s[..w * h].to_vec();
        if inverted {
            data.iter_mut().for_each(|v| *v = 255 - *v);
        }
        return GrayImage8::from_raw(width, height, data)
            .map(PageRaster::Gray)
            .ok_or_else(|| PdfError::Corrupt("gray buffer size".into()));
    }
    if bpc == 8 && nc == 3 && matches!(cs.kind, ColorSpaceKind::Rgb) && !inverted {
        let data: Vec<u8> = s[..w * h * 3]
            .chunks_exact(3)
            .map(|p| to_gray(p, &cs.kind))
            .collect();
        return GrayImage8::from_raw(width, height, data)
            .map(PageRaster::Gray)
            .ok_or_else(|| PdfError::Corrupt("rgb buffer size".into()));
    }

    let max = (1u32 << bpc) - 1;
    let sample = |row: &[u8], i: usize| -> u32 {
        match bpc {
            8 => u32::from(row[i]),
            16 => u32::from(row[2 * i]) << 8 | u32::from(row[2 * i + 1]),
            b => {
                let bit = i * usize::from(b);
                let byte = row[bit >> 3];
                u32::from(byte >> (8 - usize::from(b) - (bit & 7))) & ((1 << b) - 1)
            }
        }
    };
    // Linear /Decode map for gray components, else identity (inversion only).
    let (dmin, dmax) = if decode.len() >= 2 && cs.palette.is_none() {
        (decode[0], decode[1])
    } else {
        (0.0, 1.0)
    };
    let mut out = GrayImage8::new_white(width, height)?;
    let buf = out.as_raw_mut();
    let mut comps = [0u8; 4];
    for y in 0..h {
        let row = &s[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..w {
            let v = if let Some(pal) = &cs.palette {
                let mut idx = sample(row, x) as usize;
                if inverted {
                    idx = max as usize - idx;
                }
                pal.get(idx).copied().unwrap_or(255)
            } else if one_channel {
                let v = f64::from(sample(row, x)) / f64::from(max);
                let mapped = dmin + v * (dmax - dmin);
                (mapped.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            } else {
                for (c, slot) in comps.iter_mut().enumerate().take(nc) {
                    let raw = sample(row, x * nc + c);
                    let mut v8 = if bpc == 16 {
                        (raw >> 8) as u8
                    } else {
                        ((raw * 255 + max / 2) / max) as u8
                    };
                    if inverted {
                        v8 = 255 - v8;
                    }
                    *slot = v8;
                }
                to_gray(&comps[..nc], &cs.kind)
            };
            buf[y * w + x] = v;
        }
    }
    Ok(PageRaster::Gray(out))
}

struct JpegInfo {
    width: u32,
    height: u32,
    components: u8,
    progressive: bool,
    adobe_transform: Option<u8>,
}

/// Reads SOF dimensions and APP14 from a JPEG and checks it is complete (has an EOI marker).
fn jpeg_info(data: &[u8]) -> Result<JpegInfo, PdfError> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(PdfError::Corrupt(
            "DCT stream does not start with SOI".into(),
        ));
    }
    let tail = data.len().saturating_sub(64);
    if !data[tail..].windows(2).any(|w| w == [0xFF, 0xD9]) {
        return Err(PdfError::Corrupt(
            "JPEG missing EOI (truncated scan): rescan needed".into(),
        ));
    }
    let mut i = 2;
    let mut adobe = None;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = data[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0xD8 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        if len < 2 || i + 2 + len > data.len() {
            break;
        }
        let seg = &data[i + 4..i + 2 + len];
        match m {
            0xEE if seg.len() >= 12 && &seg[..5] == b"Adobe" => adobe = Some(seg[11]),
            0xC0..=0xCF if !matches!(m, 0xC4 | 0xC8 | 0xCC) => {
                if seg.len() < 6 {
                    break;
                }
                return Ok(JpegInfo {
                    height: u32::from(u16::from_be_bytes([seg[1], seg[2]])),
                    width: u32::from(u16::from_be_bytes([seg[3], seg[4]])),
                    components: seg[5],
                    progressive: matches!(m, 0xC2 | 0xC6 | 0xCA | 0xCE),
                    adobe_transform: adobe,
                });
            }
            0xDA => break,
            _ => {}
        }
        i += 2 + len;
    }
    Err(PdfError::Corrupt("JPEG without a frame header".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_product_order() {
        // Scale then translate.
        let s = [2.0, 0.0, 0.0, 3.0, 0.0, 0.0];
        let t = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        assert_eq!(mul(&s, &t), [2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
        assert_eq!(mul(&t, &s), [2.0, 0.0, 0.0, 3.0, 20.0, 60.0]);
    }

    #[test]
    fn jpeg_header_parsing() {
        let mut j = vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 0, 20, 0, 30, 1, 1, 0x11, 0,
        ];
        assert!(jpeg_info(&j).is_err(), "no EOI");
        j.extend_from_slice(&[0xFF, 0xD9]);
        let info = jpeg_info(&j).unwrap();
        assert_eq!((info.width, info.height, info.components), (30, 20, 1));
        assert!(!info.progressive);
        assert!(jpeg_info(&[0, 1, 2, 3]).is_err());
    }

    #[test]
    fn nominal_dpi_snaps() {
        let mut p = Placement {
            ctm: IDENTITY,
            page_box: [0.0; 4],
            rotate: 0,
            user_unit: 1.0,
            dpi_x: 599.4,
            dpi_y: 600.7,
            page_box_dpi: (600.0, 600.0),
            coverage: 1.0,
            orientation: Orientation::UPRIGHT,
            skew_degrees: 0.0,
        };
        assert_eq!(p.nominal_dpi(), Some(600));
        p.dpi_x = 287.0;
        p.dpi_y = 287.0;
        assert_eq!(p.nominal_dpi(), Some(287));
    }
}

#[cfg(test)]
mod thread_safety {
    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn document_and_outcomes_are_send_and_sync() {
        assert_send_sync::<super::PdfDocument>();
        assert_send_sync::<super::PageOutcome>();
    }
}
