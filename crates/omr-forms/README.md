# omr-forms

Template model, sheet geometry, page code and sheet rendering for the OMR Correction app
(spec §6.1–§6.5, §6.8, §6.9 lint basics). Engine crate: no file or network I/O, no threads
(R4), key-blind (R2). Rendering lives behind the `render` feature.

## Public API outline

```rust
// Specification → validated, laid-out, linted, hashed template.
let spec = TemplateSpec::standard(exam_id, 100, 5, CancelMode::EraseOnly); // or build the struct
let t: Template = Template::build(spec)?;          // BuildError::{Spec, Lint(L14…), …}

t.pages() -> &[PageGeometry]                         // per side, 1-based `page`
t.page(PageNo) / t.question(q) / t.question_bubbles(q) -> &[AnswerBubble]
t.manifest() -> &LayoutManifest                      // the hashed layout manifest (Serialize + Deserialize)
t.hash() / t.hash_hex()                              // SHA-384 of the canonical JSON (RFC 8785 subset)
t.cancel_mode(), t.direction(), t.forms(), t.page_count(), t.warnings(), t.lint_at_dpi(dpi)
t.page_code(form, page, serial) -> PageCode, t.exam_id40()

// Geometry is integer micrometres (Um = i32); every type has *_mm() accessors (omr_model::PointMm).
PageGeometry { page, tags: Vec<FiducialTag>, codes: Vec<CodeSlot>, blocks, bubbles: Vec<AnswerBubble>,
               id_bubbles: Vec<IdBubble>, form_bubbles: Vec<FormBubble>, ticks: Vec<RowTick> }
AnswerBubble { question, option, centre: PtUm, radius, label }   .centre_mm() .radius_mm() .bubble_id()
IdBubble { column /*0 = leftmost*/, digit, centre, radius, label }
FiducialTag { id, corner, outer /*8 mm*/, black /*6.4 mm*/ }      .corners()/.corners_mm() (TL,TR,BR,BL) .id_in_set(k)
CodeSlot { copy, corner, rect /*QR symbol*/, module, modules, echo, echo_align }
RowTick { question, side: Left|Right, rect /*1.2 × 0.6 mm*/ }
geometry::*  // §6.3 constants: SAFE_BOX_A4, BUBBLE_DIAMETER, PITCH_X/Y, QUIET_ZONE, TAG_*, QR_*, …

// Page code (§6.4).
let key = ExamKey::from_bytes(k_exam);               // Debug is redacted; never serialised
let payload: String = code.encode(&key)?;            // 41 Crockford Base32 chars
pagecode::parse(&s)?.verify(&key)?                   // structure, zero pad, constant-time 96-bit tag
pagecode::decode_verified(&s, &key)?, short_form(&s), echo_code(&s), echo_line(prefix, &s),
pagecode::check_echo_code(ocr_text, &key)            // misread checksum only (IDN-001 echo_line_recovered)
PageCodeError::exception() -> omr_model::Exception   // IDN-002, BLOCKER, Page

// Lint (§6.9): lint::lint_layout(manifest, arts, dpi) -> Vec<LintFinding>; rules L01–L05, L14.

// Instances (one printed side of one booklet).
SheetInstance::new(&t, &key, FormCode(1), PageNo(1)).with_serial(BookletSerial(42))
    .with_personal(Personalisation::new(name, student_id)?)   // refuses national ID / Iqama shapes
inst.payload(), inst.echo_prefix(), inst.form_page_text(), inst.tag_set()

// Feature "render".
render::sheet_svg(&inst) / render::layer_svg(&inst, Layer::{All, Static, Variable})
render::rasterize_sheet(&inst, dpi) -> image::GrayImage     // any dpi; see pixel convention below
render::rasterize_static(&inst, dpi) + render::composite_variable(&mut img, &inst, dpi)  // cacheable fast path
render::sheets_pdf(&[inst…], title) -> Vec<u8>              // multi-page PDF, /PrintScaling /None
render::encode_png(&img), render::page_size_px(dpi), render::px_per_mm(dpi)
```

**Pixel convention.** A page point at `(x, y)` mm is at `(x · dpi / 25.4, y · dpi / 25.4)` in
image coordinates; pixel `(i, j)` covers `[i, i+1) × [j, j+1)`. Images are
`round(210 · dpi / 25.4) × round(297 · dpi / 25.4)` (1654 × 2339 at 200 dpi).

## What the layout does (§6.3 geometry, A4)

- Safe box x 7–203 mm, y 24.6–272.4 mm; every printed element is inside it (lint L04).
- Four AprilTag 36h11 tags, 8 mm (10 × 10 modules of 0.8 mm incl. the white border) on the
  safe-box corners; tag set 0 = IDs 0–3 (TL, TR, BR, BL); proofs print tag set 3 (IDs 12–15) and
  a PROOF watermark. The 36h11 code table is embedded (`apriltag.rs`).
- QR v3-M page code (29 × 29 modules of 0.65 mm = 18.85 mm), printed twice on the TR + BL
  diagonal (TL + BR optional), each with a two-line echo line (friendly prefix; chars 1–21 and the
  7-character short form) in IBM Plex Mono.
- Bubbles Ø 4.2 mm, 0.6 pt outline, labels inside at 6 pt / 45 % K; horizontal pitch 7.0 mm; row
  pitch 6.35 mm with a 2 mm gap every 5 rows; row ticks 1.2 × 0.6 mm at both ends of every
  answer row; 3 mm quiet zones around blocks, tags and codes (lint L01).
- Page 1 header: student-ID grid (digits × 0–9 under write-in combs, top left), title, course,
  "Form B · Page 1/2", name and student-ID write-in lines (personalised text goes there),
  instructions (EN + AR), the cancel-mode instruction (§6.5 table, EN + AR), the "how to mark"
  row (correct fill, tick, X, circle, dot, with ✓/✗ icons) and the optional form grid.
- Answer columns: count from the widest row (4 columns for 4–5 options, 2 for 10), flowing
  around header elements and the codes; questions fill column-major and are balanced across
  columns and pages. 100 five-option items plus a 10-digit ID fit one A4 side.
- Arabic label set (أ ب ج د هـ و ز ح ط ي) switches rows and columns to right-to-left: Q1 top
  right, option 0 rightmost. Latin A–J and numeric 1–10 are left-to-right.
- Template hash: SHA-384 of the canonical JSON of `LayoutManifest` (tags, codes, ticks, every
  bubble with centre, radius, label; bubble spec; label set; direction; cancel mode; generator
  version). Display text (title, course) is not hashed; text that reflows geometry changes the
  hash through the geometry.

## Spec sections implemented

§6.1 (integer-µm model, bubble IDs, canonical manifest hash), §6.3 (geometry table, A4 safe box,
`/PrintScaling /None`), §6.4 (tags, tag sets, page code fields, exam-ID derivation, 96-bit
HMAC-SHA384 tag with zero pad, constant-time verify, short form, echo line), §6.5 (MCQ, ID grid,
form grid, label sets, cancel-mode instructions, how-to-mark row), §6.7 (serials, personalisation
without national IDs), §6.8 (vector bubbles, embedded subset fonts, no page transparency group,
100 % K), §6.9 lint L01–L05 and L14.

## Tests

- Unit tests per module, `req_a7_*` page-code tests (round trip, length 41, alphabet, wrong key,
  tampered serial, non-zero pad, structural rejections).
- `tests/properties.rs` (proptest): encode/decode round trip, any single-character change rejected,
  other keys never verify, parser never panics; layout invariants over random specs (safe box,
  no bubble overlaps, pitch and band rules, ticks, block quiet zones, each question exactly once,
  lint clean) and determinism.
- `tests/render.rs` (feature `render`): all four tags found by `kornia-apriltag` 0.2.0 at 200 and
  300 dpi with corners where the layout says (mean error < 0.6 px), both QR copies decoded by
  `rxing` and MAC-verified, proof tag set, cached static raster + overlay equals a full render,
  deterministic SVG without key material, multi-page PDF with `/PrintScaling /None`, embedded
  fonts, A4 media box and no page group.
- `cargo run -p omr-forms --features render --example render_sample -- <dir>` writes sample SVG,
  PNG and PDF sheets.

## Fonts

`assets/fonts/`: IBM Plex Sans Arabic Regular and Bold (Arabic + Latin) and IBM Plex Mono
Regular, from github.com/google/fonts, under the SIL Open Font License 1.1 (`OFL.txt`). They are
compiled in (`include_bytes!`), so rendering does no font I/O and is identical on every machine.

## Deviations and deferrals

- **Renderer:** SVG → resvg/svg2pdf instead of Typst (deferred per IMPLEMENTATION.md §2); same
  model and manifest, so a Typst back end can replace it behind the same calls.
- **ID-grid row pitch 5.5 mm** (low end of the §6.3 5.5–7.0 mm range; answer rows keep 6.35 mm)
  so that the common case (100 items, 10-digit ID) fits one side.
- **Paper:** A4 only (Letter safe box not laid out). Bubble shape: circles only (no squares for
  multi-select, no dense ovals). Options per item 2–10 (spec allows up to 26 with wrapping).
- **Fields:** single-answer MCQ, ID grid and form grid only. True/false, multi-select, grid-in,
  Latin name combs, signature, score boxes, Likert, trackers and admin bubbles are deferred.
- **Lint:** L01–L05 and L14 only. L06 (mirror separation), L07 (duplex padding / generic
  multi-page), L08 (self-recognition with the production recognizer; the render tests cover the
  detector part), L09–L13, L15–L17 are deferred.
- **Lifecycle:** no Draft/Frozen/Retired state machine; a `proof` flag prints the proof tag set
  and watermark. Per-student shuffling (§6.6) is not implemented: all forms share one geometry
  and differ in the page code and header text.
- **PDF:** one svg2pdf XObject per page; no PDF/A-2b archival copy, no shared Form XObject for
  per-booklet overlays, no duplex padding pages.
- **Manifest hash** also covers `cancel_mode` and the generator version (both change decisions).
- **Personalisation guard** refuses any 10-digit, Luhn-valid number starting with 1 or 2 (the
  national ID / Iqama shape); a real student ID of that exact shape would also be refused.
- **Bubble Ø** is measured on the outline's centre line (r = 2.1 mm).
- **Text metrics** used for layout and lint are estimates; the renderer uses real shaping.
  usvg lays paragraphs out left to right, so Arabic-first lines are wrapped in an RLE … PDF
  embedding.
