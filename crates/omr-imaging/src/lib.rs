//! `omr-imaging`: image decode, normalisation, sampling and geometric primitives used by
//! recognition (spec §7.3, §8.2, §8.4, §8.5, §8.6, §21.3).
//!
//! - [`gray`]: the 8-bit [`GrayImage`] (255 = white; pixel `(x, y)` covers `[x, x+1) × [y, y+1)`).
//! - [`decode`]: PNG / JPEG / TIFF bytes → luma, with decoder limits (no file I/O).
//! - [`reduce`]: integer area reduction, exact fractional area resampling (any ratio, e.g.
//!   300 → 200 dpi), packed 1-bit [`Bitmap`] → coverage gray (600 → 200 dpi = 3 × 3), capture
//!   classification (bitonal → CAP-003).
//! - [`ncc`]: exact-integer NCC with the zero-variance guard, Gaussian ring templates, a fast
//!   ring-candidate map, local maxima with parabolic sub-pixel refinement.
//! - [`sample`]: integer bilinear sampling, affine resampling, disk sums.
//! - [`warp`], [`fit`]: affine / quadratic polynomial / homography warps; weighted least
//!   squares, LMedS, normalised DLT and seeded RANSAC.
//! - [`knn`]: grid-hashed nearest neighbours.
//!
//! Rule R2: never depends on scoring, outcomes, analytics, store or AI crates.
//! Rule R4: no file or network I/O, no thread pools. Determinism (§8.11): correlation sums are
//! exact integers, sampling uses 1/256-px fixed point, and robust fits use a seeded generator.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod decode;
pub mod fit;
pub mod gray;
pub mod knn;
pub mod linalg;
pub mod ncc;
pub mod reduce;
pub mod rng;
pub mod sample;
pub mod warp;

pub use decode::{DecodeLimits, Decoded, SourceFormat, decode, decode_with_limits};
pub use fit::{
    RobustFit, fit_affine, fit_affine_lmeds, fit_homography, fit_quadratic, ransac_homography,
};
pub use gray::GrayImage;
pub use knn::PointGrid;
pub use ncc::{
    FLAT_VARIANCE, Integral, NccGrid, PatchTemplate, Peak, ResponseMap, RingHit, RingKernel,
    local_maxima, match_patch, ncc_patch, parabolic_offset, ring_map_fast,
};
pub use reduce::{
    Bitmap, CaptureKind, classify_capture, half, reduce_area, reduce_bitmap, resample_area,
    resample_to_dpi, resampled_size,
};
pub use rng::SplitMix64;
pub use sample::{DiskSum, bilinear, disk_sum, resample_affine, resample_affine_offset};
pub use warp::{Affine, Decomposition, Homography, QuadWarp, Warp};

use thiserror::Error;

/// Imaging errors.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ImagingError {
    /// Pixel buffer length does not match the dimensions.
    #[error("buffer of {len} bytes does not match {width}x{height}")]
    BadDimensions {
        /// Width.
        width: usize,
        /// Height.
        height: usize,
        /// Buffer length.
        len: usize,
    },
    /// The bytes are not a supported image.
    #[error("unsupported image: {0}")]
    Unsupported(String),
    /// Decoding failed (corrupt or truncated data).
    #[error("decode failed: {0}")]
    Decode(String),
    /// The image exceeds the decoder limits (decompression-bomb guard).
    #[error("image exceeds decoder limits: {0}")]
    TooLarge(String),
}
