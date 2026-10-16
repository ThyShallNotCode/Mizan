//! `omr-imaging`: Image decode, normalisation, illumination correction, sampling and geometric primitives
//! (homography, RANSAC) used by recognition (spec §8.2, §8.4, §8.5, §21.3).
//!
//! Rule R2: never depends on scoring, outcomes, analytics, store or AI crates.
//! Rule R4: no file or network I/O, no thread pools.
#![forbid(unsafe_code)]
