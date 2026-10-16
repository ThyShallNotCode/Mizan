//! Rendering tests (feature `render`): the printed sheet must be machine-readable by the
//! detectors the recognizer uses (§6.9 L08 self-recognition, simplified): all four AprilTag
//! 36h11 tags found by `kornia-apriltag` 0.2.0 at 200 and 300 dpi with sub-pixel corners where
//! the layout says, and both page-code copies decoded by `rxing` with a valid MAC.
#![cfg(feature = "render")]

use image::GrayImage;
use kornia_apriltag::{AprilTagDecoder, DecodeTagsConfig, family::TagFamilyKind};
use kornia_image::{Image, ImageSize};
use omr_forms::pagecode::decode_verified;
use omr_forms::render::{
    composite_variable, encode_png, px_per_mm, rasterize_sheet, rasterize_static, sheet_svg,
    sheets_pdf,
};
use omr_forms::{
    Corner, ExamKey, LabelSet, Personalisation, SheetInstance, Template, TemplateSpec,
};
use omr_model::{BookletSerial, CancelMode, ExamId, FormCode, PageNo};

fn template(q: u16, labels: LabelSet) -> Template {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_aaaa_bbbb_7ccc_8ddd_eeee_ffff_0001,
        )),
        q,
        5,
        CancelMode::EraseOnly,
    );
    s.labels = labels;
    s.forms = 2;
    s.id_digits = 10;
    Template::build(s).expect("template")
}

fn key() -> ExamKey {
    ExamKey::from_bytes([0x42; 32])
}

struct Detected {
    id: u16,
    corners: Vec<[f32; 2]>,
}

fn detect_tags(img: &GrayImage) -> Vec<Detected> {
    let size = ImageSize {
        width: img.width() as usize,
        height: img.height() as usize,
    };
    let k = Image::<u8, 1>::new(size, img.as_raw().clone()).expect("kornia image");
    let cfg = DecodeTagsConfig::new(vec![TagFamilyKind::Tag36H11]).expect("config");
    let mut dec = AprilTagDecoder::new(cfg, size).expect("decoder");
    dec.decode(&k)
        .expect("decode")
        .into_iter()
        .map(|d| Detected {
            id: d.id,
            corners: d.quad.corners.iter().map(|c| c.to_array()).collect(),
        })
        .collect()
}

fn decode_qr(img: &GrayImage, rect: omr_forms::RectUm, dpi: f32) -> String {
    let k = px_per_mm(dpi);
    let r = rect.inflate(3_000);
    let x0 = (f64::from(r.x) / 1000.0 * k).floor() as u32;
    let y0 = (f64::from(r.y) / 1000.0 * k).floor() as u32;
    let w = (f64::from(r.w) / 1000.0 * k).ceil() as u32;
    let h = (f64::from(r.h) / 1000.0 * k).ceil() as u32;
    let crop = image::imageops::crop_imm(img, x0, y0, w, h).to_image();
    let res =
        rxing::helpers::detect_in_luma(crop.into_raw(), w, h, Some(rxing::BarcodeFormat::QR_CODE))
            .expect("QR decodes");
    res.getText().to_owned()
}

fn check_page(t: &Template, dpi: f32) {
    let k = key();
    let inst = SheetInstance::new(t, &k, FormCode(2), PageNo(1)).with_serial(BookletSerial(4_321));
    let img = rasterize_sheet(&inst, dpi).expect("raster");
    let g = t.page(PageNo(1)).unwrap();

    // All four tags, IDs 0..3 at the right corners. Rendering puts the black-square corners
    // exactly where the layout says; kornia's unrefined quad fit adds up to ~1.3 px of noise on
    // single corners at 200 dpi, so the bound is on the mean (< 0.6 px) and the worst (< 1.5 px).
    let det = detect_tags(&img);
    let mut ids: Vec<u16> = det.iter().map(|d| d.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![0, 1, 2, 3], "tags at {dpi} dpi");
    let ppm = px_per_mm(dpi);
    for d in &det {
        let tag = &g.tags[usize::from(d.id)];
        assert_eq!(tag.corner, Corner::ALL[usize::from(d.id)]);
        let errs: Vec<f64> = tag
            .corners_mm()
            .iter()
            .map(|c| {
                let (ex, ey) = (c.x * ppm, c.y * ppm);
                d.corners
                    .iter()
                    .map(|p| (f64::from(p[0]) - ex).hypot(f64::from(p[1]) - ey))
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let mean = errs.iter().sum::<f64>() / 4.0;
        let worst = errs.iter().copied().fold(0.0, f64::max);
        assert!(
            mean < 0.6 && worst < 1.5,
            "tag {} corners {errs:.2?} px at {dpi} dpi",
            d.id
        );
    }

    // Both page-code copies decode and verify; payloads identical.
    let expect = inst.payload().unwrap();
    for c in &g.codes {
        let text = decode_qr(&img, c.rect, dpi);
        assert_eq!(text, expect);
        let code = decode_verified(&text, &k).expect("MAC valid");
        assert_eq!(code.serial, BookletSerial(4_321));
        assert_eq!(code.form, FormCode(2));
        assert_eq!(code.page, PageNo(1));
    }
}

#[test]
fn req_a7_tags_and_codes_are_machine_readable_at_200_dpi() {
    check_page(&template(100, LabelSet::Latin), 200.0);
}

#[test]
fn req_a7_tags_and_codes_are_machine_readable_at_300_dpi() {
    check_page(&template(60, LabelSet::Arabic), 300.0);
}

#[test]
fn proof_sheets_use_the_proof_tag_set() {
    let t = template(20, LabelSet::Latin);
    let k = key();
    let mut inst = SheetInstance::new(&t, &k, FormCode(1), PageNo(1));
    inst.proof = true;
    let img = rasterize_sheet(&inst, 200.0).unwrap();
    let mut ids: Vec<u16> = detect_tags(&img).iter().map(|d| d.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![12, 13, 14, 15]);
}

#[test]
fn cached_static_raster_plus_overlay_equals_full_render() {
    let t = template(40, LabelSet::Latin);
    let k = key();
    let person = Personalisation::new("Noura Al-Harbi · نورة الحربي", "442001234").unwrap();
    let inst = SheetInstance::new(&t, &k, FormCode(1), PageNo(1))
        .with_serial(BookletSerial(99))
        .with_personal(person);
    let full = rasterize_sheet(&inst, 200.0).unwrap();
    let mut comp = rasterize_static(&inst, 200.0).unwrap();
    composite_variable(&mut comp, &inst, 200.0).unwrap();
    assert_eq!(full.dimensions(), comp.dimensions());
    let mut worst = 0u8;
    let mut differing = 0usize;
    for (a, b) in full.pixels().zip(comp.pixels()) {
        let d = a.0[0].abs_diff(b.0[0]);
        worst = worst.max(d);
        if d > 2 {
            differing += 1;
        }
    }
    assert!(differing < 50, "{differing} pixels differ (worst {worst})");
}

#[test]
fn long_personal_names_never_reach_the_page_code() {
    let t = template(40, LabelSet::Latin);
    let k = key();
    // 75 characters (the limit is 80): far wider than the name line at 8 pt.
    let name = "Abdulrahman Mohammed Abdullah Saleh Al-Qahtani Al-Dosari Al-Harbi Al-Otaibi";
    let person = Personalisation::new(name, "442001234").unwrap();
    let inst = SheetInstance::new(&t, &k, FormCode(1), PageNo(1))
        .with_serial(BookletSerial(3))
        .with_personal(person);
    let dpi = 200.0;
    let img = rasterize_sheet(&inst, dpi).unwrap();
    let ppm = px_per_mm(dpi);
    let g = t.page(PageNo(1)).unwrap();
    for c in &g.codes {
        // The ring 0.3–2.5 mm around each symbol (inside its 3 mm quiet zone) stays paper white.
        let (outer, inner) = (c.rect.inflate(2_500).to_mm(), c.rect.inflate(300).to_mm());
        let inside = |r: &omr_model::RectMm, x: f64, y: f64| {
            x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
        };
        let (x0, y0) = ((outer.x * ppm) as u32, (outer.y * ppm) as u32);
        let (x1, y1) = (
            ((outer.x + outer.w) * ppm) as u32,
            ((outer.y + outer.h) * ppm) as u32,
        );
        for py in y0..y1 {
            for px in x0..x1 {
                let (x, y) = ((f64::from(px) + 0.5) / ppm, (f64::from(py) + 0.5) / ppm);
                if inside(&outer, x, y) && !inside(&inner, x, y) {
                    let v = img.get_pixel(px, py).0[0];
                    assert!(v > 200, "ink at ({x:.2}, {y:.2}) mm near code {}", c.copy);
                }
            }
        }
        let text = decode_qr(&img, c.rect, dpi);
        assert_eq!(text, inst.payload().unwrap());
    }
}

#[test]
fn svg_is_deterministic_and_never_contains_the_key() {
    let t = template(30, LabelSet::Latin);
    let k = key();
    let inst = SheetInstance::new(&t, &k, FormCode(1), PageNo(1)).with_serial(BookletSerial(5));
    let a = sheet_svg(&inst).unwrap();
    let b = sheet_svg(&inst).unwrap();
    assert_eq!(a, b);
    assert!(
        a.contains(&inst.payload().unwrap()[1..9]),
        "echo line carries the exam ID"
    );
    assert!(!a.contains("4242424242"), "no key bytes");
    let png = encode_png(&rasterize_sheet(&inst, 100.0).unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn pdf_is_multi_page_with_print_scaling_none_and_embedded_fonts() {
    let t = template(300, LabelSet::Latin);
    assert!(t.page_count() >= 2);
    let k = key();
    let insts: Vec<SheetInstance<'_>> = (1..=t.page_count())
        .map(|p| SheetInstance::new(&t, &k, FormCode(1), PageNo(p)).with_serial(BookletSerial(7)))
        .collect();
    let bytes = sheets_pdf(&insts, "Test").unwrap();
    let doc = lopdf::Document::load_mem(&bytes).expect("valid PDF");
    assert_eq!(doc.get_pages().len(), usize::from(t.page_count()));
    let cat = doc.catalog().unwrap();
    let vp = cat.get(b"ViewerPreferences").unwrap();
    let vp = match vp {
        lopdf::Object::Reference(r) => doc.get_dictionary(*r).unwrap(),
        o => o.as_dict().unwrap(),
    };
    assert_eq!(vp.get(b"PrintScaling").unwrap().as_name().unwrap(), b"None");
    let has_font_file = doc.objects.values().any(|o| {
        let d = match o {
            lopdf::Object::Dictionary(d) => d,
            lopdf::Object::Stream(s) => &s.dict,
            _ => return false,
        };
        d.has(b"FontFile2") || d.has(b"FontFile3")
    });
    assert!(has_font_file, "fonts are embedded");
    for (_, id) in doc.get_pages() {
        let page = doc.get_dictionary(id).unwrap();
        assert!(!page.has(b"Group"), "no page transparency group");
        let mb = page.get(b"MediaBox").unwrap().as_array().unwrap();
        let w = mb[2].as_float().unwrap();
        assert!((w - 595.276).abs() < 0.01, "A4 width {w}");
    }
}
