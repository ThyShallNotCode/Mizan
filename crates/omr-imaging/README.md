# omr-imaging

Image primitives for recognition (spec §7.3, §8.2, §8.4, §8.5, §8.6, §21.3): decode,
bitonal → coverage-gray reduction, exact-integer normalised cross-correlation (NCC), ring
templates, sampling, warps and robust fits. Engine crate: no file or network I/O, no threads
(R4); key-blind (R2); `#![forbid(unsafe_code)]` (R6).

## Public API outline

```rust
// Images. 255 = white. Pixel (x, y) covers [x, x+1) × [y, y+1): its centre is (x + 0.5, y + 0.5).
GrayImage::{new, from_raw, from_fn, width, height, row, get, get_or, set, crop, inverted, histogram}

// Decode (bytes in, no file I/O). Y = 0.299 R + 0.587 G + 0.114 B; alpha over white; 16 → 8 bit.
decode(&bytes) / decode_with_limits(&bytes, DecodeLimits { max_width, max_height, max_alloc })
  -> Decoded { image, format: Png|Jpeg|Tiff, source_bits, had_colour, capture: CaptureKind }
ImagingError::{BadDimensions, Unsupported, Decode, TooLarge}   // TooLarge / Decode → caller raises CAP-001

// Reduction and capture mode (§7.2, §8.5 bitonal mode, CAP-003).
reduce_area(&img, f)          // f × f box mean, round half up; ceil(w/f) × ceil(h/f); edge blocks partial
half(&img)                    // 2 × 2 fast path (one pyramid level)
Bitmap::new(w, h, stride, bits, black_is_one)   // packed 1-bit rows (CCITT G4 output, PDF BlackIs1)
reduce_bitmap(&bitmap, 3)     // 600 → 200 dpi coverage gray: round(255 · white / 9)
classify_capture(&img) -> CaptureKind::{Gray, Bitonal, ReducedBitonal { factor }}; .is_bitonal()

// Correlation (§8.4 Stage D). Sums are exact integers; flat windows score 0, never 1.
FLAT_VARIANCE (2e-3 on a 0–1 scale), Integral (Σv, Σv²), PatchTemplate, ncc_patch, match_patch -> NccGrid
RingKernel::gaussian(radius, width)  // the prototype's ring template, quantised, sparse taps
  .ncc_dark_at / .ncc_dark_grid / .refine(img, guess, flat, moves) -> RingHit { centre, score }
ring_map_fast(&half_res, radius, flat) -> ResponseMap   // blurred-circle ring NCC for candidates
local_maxima(&map, window, threshold) -> Vec<Peak>      // window max + parabolic sub-pixel
circle_offsets(r), parabolic_offset(l, c, r)

// Sampling (§8.6): 1/256-px coordinates, 8-bit integer bilinear weights → identical on every ISA.
bilinear(&img, x, y, outside), resample_affine(_offset)(&src, w, h, &map, …)
disk_sum(&img, cx, cy, r) -> DiskSum { ink, count, expected }   // .clipped(), .darkness_q10()

// Warps and fits (§8.4).
trait Warp { map, jacobian, local_scale }
Affine { m: [a, b, tx, c, d, ty] }  ::similarity/translation/then/inverse/decompose -> Decomposition
QuadWarp { centre, norm, cx: [f64; 6], cy: [f64; 6] }   // basis [1, x, y, x², xy, y²], 12 parameters
Homography { h: [f64; 9] }
fit_affine(src, dst, weights), fit_affine_lmeds(src, dst, iters, seed) -> RobustFit<Affine>
fit_quadratic(src, dst, weights, centre, norm), fit_homography(src, dst)   // normalised DLT
ransac_homography(src, dst, threshold, confidence, max_iters, seed) -> RobustFit<Homography>
PointGrid::{new, nearest_within, k_nearest_within, for_each_within}     // grid-hashed NN
SplitMix64 (seeded sampling), linalg::{solve, sym_eigen}
```

## Determinism (§8.11)

- NCC numerators, window sums and ring sums are integer (u16/u32/i32/i64) and exact; only the
  final square roots and divisions are floating point (IEEE correctly rounded everywhere).
- Sampling quantises coordinates to 1/256 px with 8-bit weights; affine resampling steps in
  1/65,536-px fixed point.
- Robust fits use a seeded SplitMix64; ties in nearest-neighbour queries go to the lower index;
  no `HashMap` and no fused multiply-add.

## Performance (release, one thread, 4-vCPU container under load)

On the real 200-dpi IAU pages (2,376 × 1,676): `half` ≈ 1.0 ms, `ring_map_fast` at 100 dpi
≈ 6.5–7 ms, `local_maxima` ≈ 0.5 ms, 940 `RingKernel::refine` calls ≈ 8.5 ms, PNG decode
≈ 15 ms (outside the registration budget).

## Spec sections

§7.2 / CAP-003 (bitonal detection and coverage-gray mode), §7.3 (decode by bytes), §8.2
(pyramid by box reduction; luma weights), §8.4 (NCC ring search with parabolic refinement;
affine/homography fits, LMedS, seeded RANSAC; the polynomial warp used in place of the TPS
for legacy forms), §8.6 (integer bilinear sampling, disk regions), §8.11 (determinism).

## Deviations and deferrals

- **No runtime SIMD dispatch** (`forbid(unsafe_code)`): loops are written so LLVM
  auto-vectorises them at the baseline target; results do not depend on the ISA.
- **Homography DLT** finds the null vector with a cyclic Jacobi eigen-solver on AᵀA (no SVD
  crate); RANSAC uses SplitMix64 instead of the BLAKE3-derived seed of §8.4 (the caller passes
  the seed). LM refinement, ECC (Stage B), the TPS (Stage C), ICC/colour dropout channels,
  illumination B/K maps (§8.5) and JPEG-quality estimation (§8.2) are not implemented yet.
- **`ring_map_fast`** approximates the Gaussian ring by a binomial-blurred integer circle
  (σ ≈ 0.75 px at 100 dpi); it only proposes candidates — detections are re-scored with the
  exact ring template at full resolution.
- Ring templates keep taps ≥ 1/128 of the peak (the dropped tail changes NCC by < 1 %).
