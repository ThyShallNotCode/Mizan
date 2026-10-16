//! `omr-sim`: Semi-synthetic generator (§23.3): renders filled sheets with realistic marks (fills, light
//! fills, erasures, ticks, crosses, stray marks) and simulates capture (rotation, skew, scale,
//! blur, noise, illumination, JPEG) with ground truth known by construction. Test/bench only
//! (rule R8: never shipped in release artefacts).
#![forbid(unsafe_code)]
