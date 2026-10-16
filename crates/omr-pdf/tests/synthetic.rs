//! Synthetic PDFs built with `lopdf` (CCITT data encoded with the `fax` crate, JPEG with
//! `image`): every decode path, placement case and refusal of spec §7.3.

use fax::{Color, VecWriter, encoder::Encoder};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};
use omr_pdf::{
    BitImage, ColorSpaceKind, ExtractOptions, ImageEncoding, Orientation, PageNote, PageOutcome,
    PageRaster, PdfDocument, PdfError, Refusal, reduce_bilevel,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// A sheet-like test pattern: frame, ring "bubbles", a few filled ones, a diagonal stroke.
fn sheet(w: u32, h: u32) -> BitImage {
    BitImage::from_fn(w, h, |x, y| {
        let (xf, yf) = (x as f64, y as f64);
        if x < 3 || y < 3 || x + 3 >= w || y + 3 >= h {
            return true;
        }
        let (cx, cy) = ((x / 40) * 40 + 20, (y / 40) * 40 + 20);
        let r = ((xf - cx as f64).powi(2) + (yf - cy as f64).powi(2)).sqrt();
        let filled = (x / 40 + 3 * (y / 40)) % 7 == 0;
        if (filled && r < 13.0) || (11.5..13.0).contains(&r) {
            return true;
        }
        (x as i64 - 2 * y as i64).abs() < 2
    })
    .unwrap()
}

fn encode_g4(img: &BitImage) -> Vec<u8> {
    let w = img.width();
    let mut enc = Encoder::new(VecWriter::new());
    for y in 0..img.height() {
        enc.encode_line(
            (0..w).map(|x| {
                if img.get(x, y) {
                    Color::Black
                } else {
                    Color::White
                }
            }),
            w,
        )
        .unwrap();
    }
    enc.finish().unwrap().finish()
}

/// Minimal MSB-first bit writer for hand-built Group 3 streams.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u8,
}

impl Bits {
    fn put(&mut self, data: u16, len: u8) {
        for i in (0..len).rev() {
            self.acc = (self.acc << 1) | u32::from((data >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    fn align(&mut self) {
        while self.n != 0 {
            self.put(0, 1);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

fn put_run(b: &mut Bits, black: bool, mut n: u32) {
    let table = if black {
        &fax::maps::black::ENTRIES
    } else {
        &fax::maps::white::ENTRIES
    };
    let mut write = |n: u32| {
        let idx = if n >= 64 { 63 + n / 64 } else { n } as usize;
        let (v, bits) = table[idx];
        assert_eq!(u32::from(v), n);
        b.put(bits.data, bits.len);
    };
    while n >= 2560 {
        write(2560);
        n -= 2560;
    }
    if n >= 64 {
        write(n & !63);
        n &= 63;
    }
    write(n);
}

/// Group 3 1-D (MH) stream. `k_tag`: emit the 1-D tag bit after each EOL (`/K > 0` streams
/// whose lines are all coded 1-D); `byte_align`: pad so each EOL ends on a byte boundary.
fn encode_g3_1d(img: &BitImage, eol: bool, byte_align: bool, k_tag: bool) -> Vec<u8> {
    let mut b = Bits::default();
    for y in 0..img.height() {
        if eol {
            if byte_align {
                // Fill so that the EOL's final 1 bit is the last bit of a byte.
                while !(b.out.len() * 8 + b.n as usize + 12).is_multiple_of(8) {
                    b.put(0, 1);
                }
            }
            b.put(1, 12);
            if k_tag {
                b.put(1, 1);
            }
        } else if byte_align {
            b.align();
        }
        let mut x = 0;
        let mut black = false;
        while x < img.width() {
            let mut run = 0;
            while x + run < img.width() && img.get(x + run, y) == black {
                run += 1;
            }
            put_run(&mut b, black, run);
            x += run;
            black = !black;
        }
    }
    if eol {
        for _ in 0..6 {
            b.put(1, 12);
            if k_tag {
                b.put(1, 1);
            }
        }
    }
    b.finish()
}

struct ImageXObject {
    dict: Dictionary,
    data: Vec<u8>,
}

struct PageSpec {
    media: [f64; 4],
    rotate: Option<i64>,
    content: String,
    images: Vec<(String, ImageXObject)>,
    form: Option<(String, String, Vec<f64>)>,
}

fn page(w_pt: f64, h_pt: f64, content: &str, images: Vec<(&str, ImageXObject)>) -> PageSpec {
    PageSpec {
        media: [0.0, 0.0, w_pt, h_pt],
        rotate: None,
        content: content.to_owned(),
        images: images.into_iter().map(|(n, i)| (n.to_owned(), i)).collect(),
        form: None,
    }
}

fn build(pages: Vec<PageSpec>, modern: bool) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for p in pages {
        let mut xobjects = Dictionary::new();
        for (name, img) in p.images {
            let id = doc.add_object(Stream::new(img.dict, img.data));
            xobjects.set(name.as_bytes().to_vec(), Object::Reference(id));
        }
        if let Some((form_name, form_content, matrix)) = p.form {
            let inner = std::mem::take(&mut xobjects);
            let fid = doc.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), 1.into(), 1.into()],
                    "Matrix" => matrix.into_iter().map(|v| Object::Real(v as f32)).collect::<Vec<_>>(),
                    "Resources" => dictionary! { "XObject" => inner },
                },
                form_content.into_bytes(),
            ));
            xobjects.set(form_name.as_bytes().to_vec(), Object::Reference(fid));
        }
        let content_id = doc.add_object(Stream::new(Dictionary::new(), p.content.into_bytes()));
        let mut pd = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => p.media.iter().map(|v| Object::Real(*v as f32)).collect::<Vec<_>>(),
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => xobjects },
        };
        if let Some(r) = p.rotate {
            pd.set("Rotate", r);
        }
        kids.push(Object::Reference(doc.add_object(pd)));
    }
    let n = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    if modern {
        doc.save_modern(&mut out).unwrap();
    } else {
        doc.save_to(&mut out).unwrap();
    }
    out
}

fn ccitt_xobject(img: &BitImage, data: Vec<u8>, parms: Dictionary) -> ImageXObject {
    ImageXObject {
        dict: dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => img.width() as i64,
            "Height" => img.height() as i64,
            "BitsPerComponent" => 1,
            "ColorSpace" => "DeviceGray",
            "Filter" => "CCITTFaxDecode",
            "DecodeParms" => parms,
        },
        data,
    }
}

fn g4_xobject(img: &BitImage) -> ImageXObject {
    let data = encode_g4(img);
    ccitt_xobject(
        img,
        data,
        dictionary! { "K" => -1, "Columns" => img.width() as i64 },
    )
}

/// `W 0 0 H 0 0 cm` full-page placement of `/Im0`.
fn full_page_content(w_pt: f64, h_pt: f64) -> String {
    format!("q\n{w_pt} 0 0 {h_pt} 0 0 cm\n/Im0 Do\nQ\n")
}

fn open(bytes: Vec<u8>) -> PdfDocument {
    PdfDocument::open(bytes, ExtractOptions::default()).unwrap()
}

fn bilevel(o: &PageOutcome) -> &BitImage {
    let Some(p) = o.extracted() else {
        panic!("expected an extracted page, got {:?}", o.refused());
    };
    match &p.raster {
        PageRaster::Bilevel(b) => b,
        other => panic!("expected bilevel, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// CCITT
// ---------------------------------------------------------------------------------------------

#[test]
fn g4_full_page_is_pixel_exact_with_effective_dpi() {
    let img = sheet(1200, 840); // 600 dpi over 144 × 100.8 pt
    let pdf = build(
        vec![page(
            144.0,
            100.8,
            &full_page_content(144.0, 100.8),
            vec![("Im0", g4_xobject(&img))],
        )],
        false,
    );
    let doc = open(pdf);
    assert_eq!(doc.page_count(), 1);
    let out = doc.extract_page(0);
    assert_eq!(bilevel(&out), &img);
    let p = out.extracted().unwrap();
    assert!((p.placement.dpi_x - 600.0).abs() < 1e-3 && (p.placement.dpi_y - 600.0).abs() < 1e-3);
    assert_eq!(p.nominal_dpi(), Some(600));
    assert_eq!(p.placement.orientation, Orientation::UPRIGHT);
    assert!((p.placement.coverage - 1.0).abs() < 1e-5);
    assert!(matches!(&p.source.encoding, ImageEncoding::Ccitt { scheme, .. } if scheme == "G4"));
    assert_eq!(p.source.color_space, ColorSpaceKind::Gray);
    // 200-dpi working image equals the 3×3 reduction.
    let g = p.working_gray(200).unwrap().unwrap();
    assert_eq!(g, reduce_bilevel(&img, 3).unwrap());
    assert_eq!((g.width(), g.height()), (400, 280));
}

#[test]
fn g4_black_is_1_and_decode_arrays_are_honoured() {
    let img = sheet(320, 200);
    let inverted = {
        let mut i = img.clone();
        i.invert();
        i
    };
    let data = encode_g4(&img);
    let cases: Vec<(Dictionary, Option<Vec<Object>>, &BitImage)> = vec![
        // BlackIs1 true, default /Decode: CCITT black shows white.
        (
            dictionary! { "K" => -1, "Columns" => 320, "BlackIs1" => true },
            None,
            &inverted,
        ),
        // BlackIs1 true with /Decode [1 0]: shows as encoded.
        (
            dictionary! { "K" => -1, "Columns" => 320, "BlackIs1" => true },
            Some(vec![1.into(), 0.into()]),
            &img,
        ),
        // BlackIs1 false with /Decode [1 0]: inverted.
        (
            dictionary! { "K" => -1, "Columns" => 320 },
            Some(vec![1.into(), 0.into()]),
            &inverted,
        ),
    ];
    for (parms, decode, expect) in cases {
        let mut x = ccitt_xobject(&img, data.clone(), parms);
        if let Some(d) = decode {
            x.dict.set("Decode", d);
        }
        let pdf = build(
            vec![page(
                38.4,
                24.0,
                &full_page_content(38.4, 24.0),
                vec![("Im0", x)],
            )],
            false,
        );
        assert_eq!(bilevel(&open(pdf).extract_page(0)), expect);
    }
}

#[test]
fn image_mask_paints_zero_samples() {
    let img = sheet(160, 96);
    let mut x = g4_xobject(&img);
    x.dict.remove(b"ColorSpace");
    x.dict.set("ImageMask", true);
    let pdf = build(
        vec![page(
            19.2,
            11.52,
            &full_page_content(19.2, 11.52),
            vec![("Im0", x)],
        )],
        false,
    );
    let out = open(pdf).extract_page(0);
    assert_eq!(bilevel(&out), &img);
    assert_eq!(
        out.extracted().unwrap().source.color_space,
        ColorSpaceKind::StencilMask
    );
}

#[test]
fn group3_one_and_two_dimensional_variants_decode() {
    let img = sheet(203, 61);
    let variants = [
        // (K, EndOfLine, EncodedByteAlign)
        (0, false, false),
        (0, true, false),
        (0, true, true),
        (0, false, true),
        (4, true, false),
        (4, true, true),
    ];
    for (k, eol, align) in variants {
        let data = encode_g3_1d(&img, eol, align, k > 0);
        let x = ccitt_xobject(
            &img,
            data,
            dictionary! {
                "K" => k,
                "Columns" => 203,
                "EndOfLine" => eol,
                "EncodedByteAlign" => align,
            },
        );
        let pdf = build(
            vec![page(
                24.36,
                7.32,
                &full_page_content(24.36, 7.32),
                vec![("Im0", x)],
            )],
            false,
        );
        let out = open(pdf).extract_page(0);
        assert_eq!(bilevel(&out), &img, "K={k} eol={eol} align={align}");
        let enc = &out.extracted().unwrap().source.encoding;
        assert!(matches!(enc, ImageEncoding::Ccitt { k: kk, .. } if *kk == k));
    }
}

#[test]
fn truncated_ccitt_is_cap001_and_other_pages_still_extract() {
    let img = sheet(400, 300);
    let mut bad = g4_xobject(&img);
    bad.data.truncate(bad.data.len() / 3);
    let pdf = build(
        vec![
            page(
                48.0,
                36.0,
                &full_page_content(48.0, 36.0),
                vec![("Im0", g4_xobject(&img))],
            ),
            page(
                48.0,
                36.0,
                &full_page_content(48.0, 36.0),
                vec![("Im0", bad)],
            ),
            page(
                48.0,
                36.0,
                &full_page_content(48.0, 36.0),
                vec![("Im0", g4_xobject(&img))],
            ),
        ],
        false,
    );
    let all = open(pdf).extract_all();
    assert_eq!(bilevel(&all[0]), &img);
    assert_eq!(bilevel(&all[2]), &img);
    let r = all[1].refused().expect("page 2 refused");
    assert_eq!(r.exception.code, "CAP-001");
    assert_eq!(r.exception.severity, omr_model::Severity::Blocker);
    assert!(
        matches!(r.reason, Refusal::Error(PdfError::Corrupt(_))),
        "{:?}",
        r.reason
    );
}

#[test]
fn missing_columns_defaults_to_width() {
    let img = sheet(240, 120);
    let x = ccitt_xobject(&img, encode_g4(&img), dictionary! { "K" => -1 });
    let pdf = build(
        vec![page(
            28.8,
            14.4,
            &full_page_content(28.8, 14.4),
            vec![("Im0", x)],
        )],
        false,
    );
    let out = open(pdf).extract_page(0);
    assert_eq!(bilevel(&out), &img);
    assert!(
        out.extracted()
            .unwrap()
            .notes
            .contains(&PageNote::ColumnsDefaulted)
    );
}

// ---------------------------------------------------------------------------------------------
// Placement: rotation, scaling, forms, xref streams
// ---------------------------------------------------------------------------------------------

#[test]
fn page_rotate_and_ctm_rotation_are_reported_and_applied() {
    // A landscape 600-dpi scan (W×H px) drawn rotated 90° CCW onto a portrait page:
    // CTM [0 W' -H' 0 H' 0] maps the image x axis to page +y.
    let img = sheet(240, 160);
    let (wpt, hpt) = (28.8, 19.2);
    let content = format!("q 0 {wpt} -{hpt} 0 {hpt} 0 cm /Im0 Do Q");
    let mut spec = page(hpt, wpt, &content, vec![("Im0", g4_xobject(&img))]);
    let pdf = build(vec![spec], false);
    let out = open(pdf).extract_page(0);
    let p = out.extracted().unwrap();
    assert_eq!(p.nominal_dpi(), Some(600));
    // Image x axis points up on the page: displayed after a 270° clockwise turn.
    assert_eq!(p.placement.orientation.quarter_turns_cw, 3);
    assert!(!p.placement.orientation.mirrored);
    let disp = p.working_gray(600).unwrap().unwrap();
    assert_eq!((disp.width(), disp.height()), (160, 240));
    // The stored top-left pixel lands bottom-left after the turn.
    let o = p.placement.orientation;
    assert_eq!(o.map(0, 0, 240, 160), (0, 239));

    // /Rotate 90 on an upright placement: one clockwise quarter turn.
    spec = page(
        28.8,
        19.2,
        &full_page_content(28.8, 19.2),
        vec![("Im0", g4_xobject(&img))],
    );
    spec.rotate = Some(90);
    let p2 = open(build(vec![spec], false)).extract_page(0);
    let p2 = p2.extracted().unwrap();
    assert_eq!(p2.placement.rotate, 90);
    assert_eq!(p2.placement.orientation.quarter_turns_cw, 1);
    assert!((p2.placement.page_box_dpi.0 - 600.0).abs() < 1e-3);
}

#[test]
fn scaled_placement_gives_effective_dpi_and_form_xobjects_are_followed() {
    // 300-dpi image drawn at 2× via a Form XObject whose /Matrix scales by 2.
    let img = sheet(300, 150);
    let mut spec = page(144.0, 72.0, "q /Fm0 Do Q", vec![("Im0", g4_xobject(&img))]);
    spec.form = Some((
        "Fm0".into(),
        "q 72 0 0 36 0 0 cm /Im0 Do Q".into(),
        vec![2.0, 0.0, 0.0, 2.0, 0.0, 0.0],
    ));
    let out = open(build(vec![spec], true)).extract_page(0);
    let p = out.extracted().expect("image found through the form");
    assert_eq!(p.nominal_dpi(), Some(150));
    assert_eq!(bilevel(&out), &img);
}

#[test]
fn skewed_placement_is_noted() {
    let img = sheet(120, 120);
    let (c, s) = (
        2f64.to_radians().cos() * 14.4,
        2f64.to_radians().sin() * 14.4,
    );
    let content = format!("q {c} {s} {} {c} 0.5 0 cm /Im0 Do Q", -s);
    let out = open(build(
        vec![page(15.0, 15.0, &content, vec![("Im0", g4_xobject(&img))])],
        false,
    ))
    .extract_page(0);
    let p = out.extracted().unwrap();
    assert!(
        (p.placement.skew_degrees + 2.0).abs() < 0.01,
        "{}",
        p.placement.skew_degrees
    );
    assert!(
        p.notes
            .iter()
            .any(|n| matches!(n, PageNote::SkewedPlacement { .. }))
    );
}

#[test]
fn invisible_ocr_text_and_small_logos_are_ignored() {
    let img = sheet(240, 160);
    let logo = sheet(16, 16);
    let content = format!(
        "{}BT 3 Tr /F1 6 Tf 10 10 Td (hidden) Tj ET q 2 0 0 2 1 1 cm /Im1 Do Q 0 0 1 1 re f",
        full_page_content(28.8, 19.2)
    );
    let out = open(build(
        vec![page(
            28.8,
            19.2,
            &content,
            vec![("Im0", g4_xobject(&img)), ("Im1", g4_xobject(&logo))],
        )],
        false,
    ))
    .extract_page(0);
    let p = out.extracted().unwrap();
    assert_eq!(bilevel(&out), &img);
    assert!(p.notes.contains(&PageNote::InvisibleTextIgnored));
    assert!(p.notes.contains(&PageNote::ExtraImagesIgnored { count: 1 }));
    assert!(p.notes.contains(&PageNote::VectorContentIgnored { ops: 1 }));
}

// ---------------------------------------------------------------------------------------------
// Flate, raw, JPEG
// ---------------------------------------------------------------------------------------------

fn zlib(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

fn gray_pattern(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .map(|i| ((i % w) * 7 + (i / w) * 13) as u8)
        .collect()
}

#[test]
fn flate_gray_with_png_predictor_and_rgb_to_luma() {
    let (w, h) = (50u32, 30u32);
    let gray = gray_pattern(w, h);
    // PNG "Up" predictor rows.
    let mut pred = Vec::new();
    for y in 0..h as usize {
        pred.push(2u8);
        for x in 0..w as usize {
            let up = if y == 0 {
                0
            } else {
                gray[(y - 1) * w as usize + x]
            };
            pred.push(gray[y * w as usize + x].wrapping_sub(up));
        }
    }
    let x = ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray", "Filter" => "FlateDecode",
            "DecodeParms" => dictionary! { "Predictor" => 12, "Colors" => 1, "Columns" => w as i64 },
        },
        data: zlib(&pred),
    };
    let out = open(build(
        vec![page(
            18.0,
            10.8,
            &full_page_content(18.0, 10.8),
            vec![("Im0", x)],
        )],
        false,
    ))
    .extract_page(0);
    let p = out.extracted().unwrap();
    assert_eq!(p.nominal_dpi(), Some(200));
    match &p.raster {
        PageRaster::Gray(g) => assert_eq!(g.as_raw(), &gray[..]),
        o => panic!("{o:?}"),
    }
    assert_eq!(p.source.encoding, ImageEncoding::Flate { predictor: 12 });

    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| [(i % 251) as u8, (i % 97) as u8, 200])
        .collect();
    let x = ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "BitsPerComponent" => 8, "ColorSpace" => "DeviceRGB", "Filter" => "FlateDecode",
        },
        data: zlib(&rgb),
    };
    let out = open(build(
        vec![page(
            18.0,
            10.8,
            &full_page_content(18.0, 10.8),
            vec![("Im0", x)],
        )],
        false,
    ))
    .extract_page(0);
    match &out.extracted().unwrap().raster {
        PageRaster::Gray(g) => {
            for (i, v) in g.as_raw().iter().enumerate() {
                let (r, gg, b) = (
                    rgb[3 * i] as u32,
                    rgb[3 * i + 1] as u32,
                    rgb[3 * i + 2] as u32,
                );
                assert_eq!(*v as u32, (299 * r + 587 * gg + 114 * b + 500) / 1000);
            }
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn flate_and_raw_one_bit_become_bilevel() {
    let img = sheet(77, 41);
    // PDF 1-bit DeviceGray: 0 = black, rows byte-aligned.
    let rb = 77usize.div_ceil(8);
    let mut raw = vec![0xFFu8; rb * 41];
    for y in 0..41u32 {
        for x in 0..77u32 {
            if img.get(x, y) {
                raw[y as usize * rb + (x / 8) as usize] &= !(0x80 >> (x % 8));
            }
        }
    }
    for (filter, data) in [(Some("FlateDecode"), zlib(&raw)), (None, raw.clone())] {
        let mut dict = dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 77, "Height" => 41,
            "BitsPerComponent" => 1, "ColorSpace" => "DeviceGray",
        };
        if let Some(f) = filter {
            dict.set("Filter", f);
        }
        let x = ImageXObject { dict, data };
        let out = open(build(
            vec![page(
                9.24,
                4.92,
                &full_page_content(9.24, 4.92),
                vec![("Im0", x)],
            )],
            false,
        ))
        .extract_page(0);
        assert_eq!(bilevel(&out), &img, "{filter:?}");
    }
}

#[test]
fn indexed_and_four_bit_gray_unpack() {
    // 4-bit gray: 0..15 scaled to 0..255.
    let (w, h) = (6u32, 2u32);
    let samples: Vec<u8> = vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xAB];
    let x = ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "BitsPerComponent" => 4, "ColorSpace" => "DeviceGray",
        },
        data: samples,
    };
    let out = open(build(
        vec![page(
            6.0,
            2.0,
            &full_page_content(6.0, 2.0),
            vec![("Im0", x)],
        )],
        false,
    ))
    .extract_page(0);
    match &out.extracted().unwrap().raster {
        PageRaster::Gray(g) => {
            assert_eq!(&g.as_raw()[..6], &[0, 17, 34, 51, 68, 85]);
        }
        o => panic!("{o:?}"),
    }
    // Indexed over DeviceRGB, 2 entries: black and white.
    let x = ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 8, "Height" => 1,
            "BitsPerComponent" => 8,
            "ColorSpace" => vec![
                Object::Name(b"Indexed".to_vec()),
                Object::Name(b"DeviceRGB".to_vec()),
                1.into(),
                Object::String(vec![0, 0, 0, 255, 255, 255], lopdf::StringFormat::Hexadecimal),
            ],
        },
        data: vec![0, 1, 1, 0, 1, 0, 0, 1],
    };
    let out = open(build(
        vec![page(
            8.0,
            1.0,
            &full_page_content(8.0, 1.0),
            vec![("Im0", x)],
        )],
        false,
    ))
    .extract_page(0);
    let p = out.extracted().unwrap();
    assert_eq!(p.source.color_space, ColorSpaceKind::Indexed { entries: 2 });
    match &p.raster {
        PageRaster::Gray(g) => assert_eq!(g.as_raw(), &[0, 255, 255, 0, 255, 0, 0, 255]),
        o => panic!("{o:?}"),
    }
}

fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
    let px = gray_pattern(w, h);
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode(&px, w, h, image::ExtendedColorType::L8)
        .unwrap();
    out
}

#[test]
fn dct_is_passed_through_byte_identical() {
    let jpg = jpeg_bytes(64, 48);
    let x = ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 64, "Height" => 48,
            "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray", "Filter" => "DCTDecode",
            "Decode" => vec![1.into(), 0.into()],
        },
        data: jpg.clone(),
    };
    let out = open(build(
        vec![page(
            23.04,
            17.28,
            &full_page_content(23.04, 17.28),
            vec![("Im0", x)],
        )],
        true,
    ))
    .extract_page(0);
    let p = out.extracted().unwrap();
    match &p.raster {
        PageRaster::Jpeg(j) => {
            assert_eq!(j.data, jpg);
            assert_eq!((j.width, j.height, j.components), (64, 48, 1));
            assert!(j.invert, "/Decode [1 0] is reported");
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(p.nominal_dpi(), Some(200));
    assert_eq!(
        p.reduced(200).unwrap(),
        None,
        "JPEG decode belongs to omr-imaging"
    );

    // A truncated JPEG (no EOI) is CAP-001.
    let mut cut = jpeg_bytes(64, 48);
    cut.truncate(cut.len() - 200);
    let x = ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 64, "Height" => 48,
            "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray", "Filter" => "DCTDecode",
        },
        data: cut,
    };
    let out = open(build(
        vec![page(
            23.04,
            17.28,
            &full_page_content(23.04, 17.28),
            vec![("Im0", x)],
        )],
        false,
    ))
    .extract_page(0);
    assert_eq!(out.refused().unwrap().exception.code, "CAP-001");
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

fn refusal_of(x: ImageXObject) -> omr_pdf::RefusedPage {
    let out = open(build(
        vec![page(
            10.0,
            10.0,
            &full_page_content(10.0, 10.0),
            vec![("Im0", x)],
        )],
        false,
    ))
    .extract_page(0);
    out.refused().cloned().expect("refused")
}

#[test]
fn jbig2_pages_are_cap004() {
    // A dangling reference is enough for detection: the key is what matters.
    let doc_globals = dictionary! { "JBIG2Globals" => Object::Reference((999, 0)) };
    for (parms, with_globals) in [(Some(doc_globals), true), (None, false)] {
        let mut dict = dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 10, "Height" => 10,
            "BitsPerComponent" => 1, "ColorSpace" => "DeviceGray", "Filter" => "JBIG2Decode",
        };
        if let Some(p) = parms {
            dict.set("DecodeParms", p);
        }
        let r = refusal_of(ImageXObject {
            dict,
            data: vec![0; 16],
        });
        assert_eq!(r.exception.code, "CAP-004");
        assert_eq!(r.exception.severity, omr_model::Severity::Error);
        assert_eq!(
            r.reason,
            Refusal::Jbig2 {
                globals: with_globals
            }
        );
    }
}

#[test]
fn mrc_layered_pages_are_cap004() {
    // A page image with a soft mask.
    let img = sheet(40, 40);
    let mut x = g4_xobject(&img);
    let mask_dict = dictionary! {
        "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1,
        "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray",
    };
    // The simple builder has no mask objects; build this page by hand.
    let mut d = Document::with_version("1.5");
    let pages_id = d.new_object_id();
    let mask = d.add_object(Stream::new(mask_dict, vec![255]));
    x.dict.set("SMask", Object::Reference(mask));
    let im = d.add_object(Stream::new(x.dict.clone(), x.data.clone()));
    let content = d.add_object(Stream::new(
        Dictionary::new(),
        full_page_content(4.8, 4.8).into_bytes(),
    ));
    let pg = d.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(4.8), Object::Real(4.8)],
        "Contents" => content,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => im } },
    });
    d.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![pg.into()], "Count" => 1 },
        ),
    );
    let cat = d.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    d.trailer.set("Root", cat);
    let mut bytes = Vec::new();
    d.save_to(&mut bytes).unwrap();
    let out = open(bytes).extract_page(0);
    let r = out.refused().unwrap();
    assert_eq!(r.exception.code, "CAP-004");
    assert_eq!(r.reason, Refusal::Mrc);

    // Two overlapping full-page images (background + foreground layers).
    let content = format!(
        "{}{}",
        full_page_content(4.8, 4.8),
        "q 4.8 0 0 4.8 0 0 cm /Im1 Do Q"
    );
    let out = open(build(
        vec![page(
            4.8,
            4.8,
            &content,
            vec![("Im0", g4_xobject(&img)), ("Im1", g4_xobject(&img))],
        )],
        false,
    ))
    .extract_page(0);
    assert_eq!(out.refused().unwrap().reason, Refusal::Mrc);
}

#[test]
fn unsupported_and_vector_pages_are_cap001() {
    // JPX.
    let r = refusal_of(ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 10, "Height" => 10,
            "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray", "Filter" => "JPXDecode",
        },
        data: vec![0; 8],
    });
    assert_eq!(r.exception.code, "CAP-001");
    // Oversized header (refused before allocation).
    let r = refusal_of(ImageXObject {
        dict: dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 30_000, "Height" => 10,
            "BitsPerComponent" => 1, "ColorSpace" => "DeviceGray",
        },
        data: vec![0; 8],
    });
    assert!(matches!(
        r.reason,
        Refusal::Error(PdfError::TooLarge { .. })
    ));
    // Vector-only page.
    let out = open(build(vec![page(10.0, 10.0, "0 0 5 5 re f", vec![])], false)).extract_page(0);
    assert_eq!(out.refused().unwrap().exception.code, "CAP-001");
    // An image covering a small part of the page (composite page).
    let img = sheet(40, 40);
    let out = open(build(
        vec![page(
            100.0,
            100.0,
            "q 20 0 0 20 0 0 cm /Im0 Do Q",
            vec![("Im0", g4_xobject(&img))],
        )],
        false,
    ))
    .extract_page(0);
    assert_eq!(out.refused().unwrap().exception.code, "CAP-001");
}

#[test]
fn document_level_errors() {
    assert_eq!(
        PdfDocument::open(Vec::new(), ExtractOptions::default()).unwrap_err(),
        PdfError::Empty
    );
    let e = PdfDocument::open(b"hello world".to_vec(), ExtractOptions::default()).unwrap_err();
    assert!(matches!(e, PdfError::Invalid(_)));
    assert_eq!(e.exception().code, "CAP-001");
    let img = sheet(40, 40);
    let pdf = build(
        vec![
            page(
                4.8,
                4.8,
                &full_page_content(4.8, 4.8),
                vec![("Im0", g4_xobject(&img))],
            ),
            page(
                4.8,
                4.8,
                &full_page_content(4.8, 4.8),
                vec![("Im0", g4_xobject(&img))],
            ),
        ],
        false,
    );
    let opts = ExtractOptions {
        limits: omr_pdf::Limits {
            max_pages: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        PdfDocument::open(pdf, opts).unwrap_err(),
        PdfError::TooManyPages(2)
    );
    // Out-of-range page index is a refusal, not a panic.
    let doc = open(build(
        vec![page(
            4.8,
            4.8,
            &full_page_content(4.8, 4.8),
            vec![("Im0", g4_xobject(&img))],
        )],
        false,
    ));
    assert_eq!(
        doc.extract_page(7).refused().unwrap().exception.code,
        "CAP-001"
    );
}

#[test]
fn garbage_after_header_never_panics() {
    let img = sheet(64, 64);
    let good = build(
        vec![page(
            7.68,
            7.68,
            &full_page_content(7.68, 7.68),
            vec![("Im0", g4_xobject(&img))],
        )],
        false,
    );
    // Corrupt bytes at many positions: every outcome is Ok or a typed error, never a panic.
    for step in [97usize, 211, 499] {
        let mut b = good.clone();
        let mut i = 20;
        while i < b.len() {
            b[i] ^= 0x5A;
            i += step;
        }
        if let Ok(doc) = PdfDocument::open(b, ExtractOptions::default()) {
            let _ = doc.extract_all();
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    /// Any bitmap survives G4 encode → PDF → extract bit-exactly, and its 3× reduction equals
    /// the naive area average.
    #[test]
    fn g4_round_trip_property(w in 1u32..90, h in 1u32..40, seed in any::<u64>()) {
        let img = BitImage::from_fn(w, h, |x, y| {
            let v = ((u64::from(x) * 0x9E37_79B9) ^ (u64::from(y) * 0x85EB_CA6B) ^ seed).wrapping_mul(0xC2B2_AE35);
            (v >> 29) % 3 == 0
        }).unwrap();
        let (wpt, hpt) = (f64::from(w) * 0.12, f64::from(h) * 0.12);
        let pdf = build(vec![page(wpt, hpt, &full_page_content(wpt, hpt), vec![("Im0", g4_xobject(&img))])], false);
        let out = open(pdf).extract_page(0);
        prop_assert_eq!(bilevel(&out), &img);
        let g = reduce_bilevel(&img, 3).unwrap();
        for oy in 0..g.height() {
            for ox in 0..g.width() {
                let (mut b, mut n) = (0u32, 0u32);
                for y in 3 * oy..(3 * oy + 3).min(h) {
                    for x in 3 * ox..(3 * ox + 3).min(w) {
                        n += 1;
                        b += u32::from(img.get(x, y));
                    }
                }
                let expect = ((510 * (n - b) + n) / (2 * n)) as u8;
                prop_assert_eq!(g.get(ox, oy), expect);
            }
        }
    }
}
