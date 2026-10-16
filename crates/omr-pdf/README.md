# omr-pdf

PDF ingest for scanner and MFP PDFs (spec §7.3, "PDF, one full-page image"): enumerates pages
and extracts each page's embedded scan image **without rasterising**, with its effective dpi,
orientation and provenance. Input is PDF bytes; the crate does no file or network I/O and
creates no threads. Every page is isolated: a corrupt or unsupported page is refused with an
Appendix A exception while the other pages extract normally (decoder panics are caught per page).

## Public API outline

```rust
// Open (lazy parse; document-level failures are PdfError → CAP-001 via .exception()).
let doc = PdfDocument::open(bytes /* Vec<u8> | Arc<[u8]> */, ExtractOptions::default())?;
let doc = PdfDocument::open_with_password(bytes, pw, opts)?;   // never brute-forced (§7.3)
doc.page_count();
doc.extract_page(i) -> PageOutcome;  doc.extract_all() -> Vec<PageOutcome>
// PdfDocument and PageOutcome are Send + Sync (omr-app may extract pages in parallel).

enum PageOutcome { Extracted(Box<ExtractedPage>), Refused(RefusedPage) }
RefusedPage { index, reason: Refusal::{Jbig2 { globals }, Mrc, Error(PdfError)}, exception }
//   JBIG2 / MRC → CAP-004 (ERROR, Page); everything else → CAP-001 (BLOCKER, Page)

ExtractedPage {
    index,
    raster: PageRaster::{ Bilevel(BitImage), Gray(GrayImage8), Jpeg(JpegStream) },
    source: ImageSource { page_index, xobject, object_id, filters, encoding: ImageEncoding::
              {Ccitt{scheme,k,black_is_1,byte_align,end_of_line} | Dct | Flate{predictor} | Raw},
              width, height, bits_per_component, color_space: ColorSpaceKind, decode_inverted,
              stream_bytes },
    placement: Placement { ctm, page_box, rotate, user_unit, dpi_x, dpi_y, page_box_dpi,
              coverage, orientation: Orientation { quarter_turns_cw, mirrored }, skew_degrees },
    notes: Vec<PageNote>,   // RowsPadded, ColumnsDefaulted, DamagedStream, ExtraImagesIgnored,
                            // VectorContentIgnored, InvisibleTextIgnored, VisibleTextIgnored,
                            // SofSizeMismatch, SkewedPlacement
}
page.nominal_dpi()            // CTM dpi snapped to 72…1200 within 1.5 %, e.g. Some(600)
page.reduced(200)?            // Option<GrayImage8>, stored orientation (None for JPEG pages)
page.working_gray(200)?       // reduced, then rotated/mirrored to the displayed page

// Rasters. BitImage: 1 bit/px, **1 = black**, MSB first, rows padded to 8-byte words.
BitImage::{new, from_fn, width, height, stride, data, row, get, set, black_count,
           to_packed_rows, invert, to_gray, oriented}
GrayImage8::{from_raw, new_white, width, height, as_raw, into_raw, get, oriented} // 0 = black
JpegStream { data, width, height, components, progressive, invert, adobe_transform }

// Coverage reduction (1-bit → 8-bit gray, 255 = white; round(255·white/area), half up).
reduce_bilevel(&bit, f)               // integer factor; f = 3 has a SWAR fast path
reduce_bilevel_to_dpi(&bit, src, dst) // integer ratios → box; others → exact area weights
reduce_gray(&gray, f)                 // box average of an 8-bit image

// Lower level: ccitt::decode(data, w, h, &CcittParams), flate::inflate(data, expected, &PredictorParams)
```

## Decode paths (§7.3)

| Image | Handling |
|---|---|
| `CCITTFaxDecode` | Native decode with `hayro-ccitt` 0.4 into a packed bitmap: G4 (`/K < 0`), G3 1-D (`/K = 0`), G3 mixed 2-D (`/K > 0`), `/EndOfLine`, `/EncodedByteAlign`, `/EndOfBlock`, `/Rows`, `/Columns` (absent → `/Width`, noted). `/BlackIs1` and `/Decode` set the polarity; image masks paint 0 samples. A stream that breaks before the last row is CAP-001 ("rescan needed"); an early end-of-block pads white and is noted. |
| `DCTDecode` | Passed through byte-identical with SOF size, components, progressive flag, `/Decode` inversion and Adobe APP14 transform; a JPEG without EOI is CAP-001. Decoding is `omr-imaging`'s job. |
| `FlateDecode` / none | Bounded inflate (`miniz_oxide`, output ≤ declared size + one row: decompression bombs cost nothing), PNG predictors 10–15 and TIFF predictor 2 (degenerate predictor geometry is a typed CAP-001, never a panic); 1-bit gray and masks → `Bilevel`; 2/4/8/16-bit gray, RGB (BT.601 luma), CMYK (naive), Indexed → `Gray`; `/Decode` honoured. ASCIIHex/ASCII85/RunLength/Flate chains (also in front of CCITT and DCT) are decoded by the crate's own bounded chain decoder (`src/stream.rs`). |
| `JBIG2Decode` | Refused, CAP-004 (lossy symbol substitution; detail notes `/JBIG2Globals`). |
| Mask + layers (MRC) | `/SMask` or stencil `/Mask` on the page image, a stencil-mask page image over other images, or two large overlapping images: CAP-004. |
| `JPXDecode`, `LZWDecode`, `Crypt`, Separation/DeviceN/Lab | CAP-001 (unsupported in the MVP). |
| Vector page, image < 50 % of the page | CAP-001 (needs the rasterising worker, not in the MVP). |
| Limits | ≤ 5,000 pages, ≤ 20,000 px per side, ≤ 100 MP, ≤ 2 GB file, checked from headers before allocation; allocations use `try_reserve`. Every stream the crate decodes (page content, Form XObjects, Indexed palettes, image filter chains) goes through a bounded filter-chain decoder: content plus forms ≤ `Limits::max_stream_bytes` (64 MiB) per page, images ≤ 1.5 × their declared size; a decompression bomb refuses the page with CAP-001 (§8.12 row 31) instead of exhausting memory. `hayro-syntax`'s unbounded `Stream::decoded()` is never called. |

**Placement.** The content stream is walked (`q`/`Q`, `cm`, `Do`, nested Form XObjects with
`/Matrix`, up to depth 8, cycle-safe). The page image is the largest image drawn; smaller ones
(logos) are ignored and noted, as are invisible OCR text (Canon "searchable PDF") and vector
paint. `dpi_x`/`dpi_y` come from the CTM (`width · 72 / (|CTM column| · UserUnit)`);
`page_box_dpi` assumes the image fills CropBox ∩ MediaBox in the displayed orientation.
`/Rotate` and the CTM give `orientation` (clockwise quarter turns, then a mirror for reflected
placements) and `skew_degrees` for non-axis-aligned placements. Pixels are never rotated at
extraction (rotation is metadata, §7.4); `working_gray` rotates the cheap 200-dpi image.

## Why `hayro-ccitt` (and `hayro-syntax`)

- **CCITT.** `hayro-ccitt` 0.4 covers every PDF `CCITTFaxDecode` parameter combination and
  reports truncation; `fax` 0.3 decodes G4 and G3 1-D only (no `/K > 0`, no byte-aligned rows
  without EOL) and pads missing rows silently. On the 143 real IAU G4 pages both are bit-identical
  and hayro-ccitt is slightly faster (median 12.8 ms vs 14.8 ms per 36-MP page). `fax` remains a
  dev-dependency to encode synthetic G4 fixtures and as the cross-decoder in the real-data test.
  One quirk is worked around: with `/EndOfLine` **and** `/EncodedByteAlign`, row alignment is
  left to the EOL fill-bit reader (hayro-ccitt would otherwise eat the EOL's zeros when an EOL
  starts mid-byte).
- **Parsing.** `hayro-syntax` 0.8 (`default-features = false, features = ["std"]`): lazy object
  access, xref repair, typed content operators, no threads. `lopdf`'s default features pull in
  `rayon` and parse eagerly in parallel, which conflicts with "parallelism belongs to omr-app";
  `lopdf` is a dev-dependency to build test PDFs.

## Real-data differential test (`tests/real_iau.rs`)

Skips unless `OMR_IAU_PDF_DIR` (the scanner PDFs) and `OMR_IAU_REF_1BIT` (fingerprints written
by `tests/support/ref_fingerprints.py`, an independent pypdf + Pillow/libtiff decode) are set;
optional `OMR_IAU_DIR` (folder with `pages.json` and the 200-dpi `pages/*.png`) and `OMR_IAU_OUT`
(scratch folder for the aggregate report). Nothing restricted is printed or committed.

Results on the 5 IAU PDFs (143 pages, 600-dpi G4, ≈ 35.5 MP each), release build, one thread:

| Check | Result |
|---|---|
| 1-bit decode vs reference (SHA-256 of packed rows + black count) | 143/143 pixel-exact |
| `fax` crate decode vs `hayro-ccitt` | 143/143 bit-identical |
| 200-dpi reduction vs `pages/*.png` | max \|diff\| = 1; 124/143 byte-identical; all 106 differing pixels are in partial right/bottom edge blocks (Pillow's fixed-point corner rounding), interior 3×3 blocks exact |
| Effective dpi | 600 on every page (CTM and page box agree) |
| Decode + reduce per page | median 19 ms, p95 30 ms, max 31 ms (target ≤ 60 ms); decode median 12.8 ms, reduce median 6.2 ms. Independent review re-run (after the bounded-decoding fix): median 17.9 ms, p95 23.9 ms, max 27.0 ms; decode median 11.9 ms, reduce median 6.2 ms |

`omr_imaging::reduce_bitmap` (added in parallel) gives identical output on the real pages
(20/20 checked) but takes ≈ 26 ms per page against ≈ 7.6 ms here, so the SWAR kernel stays local;
`omr-imaging` could adopt it.

## Tests

- Unit tests per module (bit runs, padding, orientation maps, 3× SWAR vs naive, rational area
  weights, PIL rounding table, bounded inflate and bombs, PNG/TIFF predictors, JPEG headers).
- `tests/synthetic.rs`: lopdf-built PDFs (G4 via `fax`, G3 hand-encoded with fax's code tables,
  JPEG via `image`): pixel-exact G4/G3 variants, `/BlackIs1` × `/Decode`, image masks, rotation
  and CTM orientation, Form XObjects and xref streams, scaled placements, skew, invisible text
  and logos, Flate gray/RGB/1-bit/raw/4-bit/Indexed, DCT pass-through and truncation, JBIG2 and
  MRC → CAP-004, JPX/oversize/vector/composite → CAP-001, page isolation, document errors, byte
  corruption never panics; proptest G4 round trip + reduction; decompression bombs in content
  streams, Form XObjects (including repeated draws) and image filter chains → CAP-001; ASCIIHex
  + Flate image chains decode; degenerate predictor geometry is a typed refusal.

## Deviations

- JPX (`hayro-jpeg2000`), lossless JBIG2 without globals, LZW and inline-image pages are refused
  (CAP-001 / CAP-004) instead of decoded; scanners in scope emit G4, JPEG or Flate.
- Vector/composite pages are refused (CAP-001) because the PDFium worker (§7.3, §4.4) is not in
  the MVP.
- Colour images are reduced to luma (BT.601) without ICC (`moxcms`, §7.3); CMYK uses a naive
  formula. Dropout-channel selection (§8.2) therefore needs the JPEG path or a later change.
- The coverage reduction lives here (see above) rather than in `omr-imaging`.
- `reduced()` / `working_gray()` reduce 8-bit gray pages only by integer ratios; other ratios
  (e.g. 300 → 200 dpi) return the image at its native resolution (compare its size with the
  page's `nominal_dpi()`). Bilevel pages use exact area weights at any ratio.
- `hayro-syntax` decodes cross-reference and object streams inside its parser without an output
  bound; that cannot be limited from this crate. `omr-app` should parse untrusted PDFs in a
  memory-capped worker (§4.4) until the parser offers a limit. LZW-compressed content streams
  are skipped (not decoded by the bounded decoder).

## Review fixes (independent review)

- Decompression bombs: page content streams, Form XObjects, Indexed palettes and image filter
  chains were decoded with `hayro-syntax`'s unbounded `Stream::decoded()`, so a few KiB of Flate
  could allocate gigabytes and abort the process (not catchable per page). They now use the
  bounded chain decoder (`src/stream.rs`) and refuse the page with CAP-001.
- A TIFF predictor with `/Columns 0` (zero-byte rows) panicked in `chunks_mut(0)` (caught as a
  "decoder panic"); predictor geometry is now validated (`flate::validate_predictor`).
- The ASCIIHex image-chain path no longer refuses large legitimate images (the old "encoded
  larger than 2 × pixels" pre-check rejected ASCIIHex, which is 2 × by construction).
