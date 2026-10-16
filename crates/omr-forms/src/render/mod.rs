//! Sheet rendering (feature `render`): SVG ([`svg`]), gray rasters via `resvg` ([`raster`]) and
//! multi-page PDF via `svg2pdf` ([`pdf`]), with vendored OFL fonts ([`fonts`]).
//!
//! No file or network I/O (R4): the fonts are compiled in and outputs are returned as values.

pub mod fonts;
pub mod pdf;
pub mod raster;
pub mod svg;

pub use pdf::sheets_pdf;
pub use raster::{
    composite_variable, encode_png, page_size_px, px_per_mm, rasterize_sheet, rasterize_static,
    rasterize_svg,
};
pub use svg::{Layer, layer_svg, sheet_svg, variable_regions};

use crate::instance::InstanceError;
use crate::pagecode::PageCodeError;
use thiserror::Error;

/// Why a sheet could not be rendered.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum RenderError {
    /// The instance is not valid for its template.
    #[error(transparent)]
    Instance(#[from] InstanceError),
    /// The page code could not be encoded.
    #[error(transparent)]
    PageCode(#[from] PageCodeError),
    /// QR encoding failed.
    #[error("QR encoding failed: {0}")]
    Qr(String),
    /// SVG parsing failed.
    #[error("SVG error: {0}")]
    Svg(String),
    /// PDF conversion failed.
    #[error("PDF error: {0}")]
    Pdf(String),
    /// Unsupported resolution.
    #[error("unsupported resolution {0} dpi")]
    Dpi(f32),
    /// A raster has the wrong size for the requested resolution.
    #[error("raster is {got:?}, expected {expected:?}")]
    Size {
        /// Expected (width, height).
        expected: (u32, u32),
        /// Actual (width, height).
        got: (u32, u32),
    },
    /// Image encoding failed.
    #[error("image encoding failed: {0}")]
    Encode(String),
}
