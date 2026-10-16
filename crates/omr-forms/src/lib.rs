//! `omr-forms`: Template model (§6.1, §6.3, §6.5), page code payload and HMAC-SHA384 96-bit tag (§6.4),
//! fiducial layout and sheet rendering to SVG/PNG/PDF behind the `render` feature (§6.2, §6.8).
//!
//! Rule R2: knows nothing about answer keys.
#![forbid(unsafe_code)]
