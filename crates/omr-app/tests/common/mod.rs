//! Shared fixtures of the real-world case tests (synthetic data only): the committed blank IAU
//! CCSIT form (layout only, no PII) rendered with synthetic pencil marks, capture transforms
//! (resolution, bitonal thresholding, quarter turns, bleed-through), image encoders and a
//! minimal multi-page PDF writer.
#![allow(dead_code)]

use image::ImageEncoder;
use omr_app::{ExamDefinition, ExamProject, GradeOptions, KeySource, TemplateChoice};
use omr_forms::legacy::LegacyField;
use omr_imaging::{Affine, GrayImage, SplitMix64, Warp, bilinear};
use omr_model::{CancelMode, PointPx};
use omr_recog::LegacyEngine;
use std::path::{Path, PathBuf};

/// The IAU CCSIT legacy engine (template + blank form).
pub fn iau() -> LegacyEngine {
    LegacyEngine::iau_ccsit_v1().unwrap()
}

/// Index of a bubble in the template's bubble list.
pub fn bubble(e: &LegacyEngine, field: LegacyField, group: u16, option: u8) -> usize {
    e.template()
        .bubbles()
        .iter()
        .position(|b| b.field == field && b.group == group && b.option == option)
        .expect("bubble")
}

/// What a synthetic student marks on an IAU sheet.
#[derive(Clone, Debug, Default)]
pub struct Marks {
    /// Per question (0-based): the options filled (empty: left blank).
    pub answers: Vec<Vec<u8>>,
    /// Per student-number column: the digits filled (empty: column left blank).
    pub id: Vec<Vec<u8>>,
    /// Bubbled version (0 = A), if any.
    pub version: Option<u8>,
    /// Further bubbles (template indices), e.g. stray marks in unused rows.
    pub extra: Vec<usize>,
    /// Bubbles marked faintly (template indices): a light pencil touch.
    pub light: Vec<usize>,
}

impl Marks {
    /// One answer per question (`None` = blank), a full student number, a version.
    pub fn new(answers: &[Option<u8>], id: &str, version: Option<u8>) -> Self {
        Self {
            answers: answers
                .iter()
                .map(|a| a.iter().copied().collect())
                .collect(),
            id: id.bytes().map(|c| vec![c - b'0']).collect(),
            version,
            extra: Vec::new(),
            light: Vec::new(),
        }
    }

    /// Template bubble indices to fill.
    pub fn fills(&self, e: &LegacyEngine) -> Vec<usize> {
        let mut v = Vec::new();
        for (q, opts) in self.answers.iter().enumerate() {
            for &o in opts {
                v.push(bubble(e, LegacyField::Answers, q as u16, o));
            }
        }
        for (c, digits) in self.id.iter().enumerate() {
            for &d in digits {
                v.push(bubble(e, LegacyField::StudentNumber, c as u16, d));
            }
        }
        if let Some(ver) = self.version {
            v.push(bubble(e, LegacyField::Version, 0, ver));
        }
        v.extend(&self.extra);
        v
    }
}

/// The blank form at 200 dpi, slightly rotated and shifted per `seed`, with pencil fills.
pub fn render(e: &LegacyEngine, fills: &[usize], seed: u64) -> GrayImage {
    let t = e.template();
    let (tw, th) = t.frame_size();
    let (w, h) = (tw as usize + 40, th as usize + 30);
    let angle: f64 = 0.006 * (seed % 3) as f64 - 0.004;
    let fwd = Affine {
        m: [
            angle.cos(),
            -angle.sin(),
            21.0,
            angle.sin(),
            angle.cos(),
            14.0,
        ],
    };
    let inv = fwd.inverse().expect("invertible");
    let blank = e.blank();
    let mut img = GrayImage::from_fn(w, h, |x, y| {
        let p = inv.map(PointPx {
            x: x as f64 + 0.5,
            y: y as f64 + 0.5,
        });
        bilinear(blank, p.x, p.y, 255)
    });
    let mut rng = SplitMix64::new(seed);
    let r = t.bubble_radius_px() * 0.82;
    for &j in fills {
        let c = fwd.map(t.bubbles()[j].centre);
        for y in (c.y - r).floor() as isize..=(c.y + r).ceil() as isize {
            for x in (c.x - r).floor() as isize..=(c.x + r).ceil() as isize {
                let (dx, dy) = (x as f64 + 0.5 - c.x, y as f64 + 0.5 - c.y);
                if dx * dx + dy * dy > r * r || x < 0 || y < 0 {
                    continue;
                }
                let (xu, yu) = (x as usize, y as usize);
                if xu >= w || yu >= h {
                    continue;
                }
                let ink = 25 + (rng.next_u64() % 51) as u32;
                let v = u32::from(img.get(xu, yu)) * ink / 255;
                img.set(xu, yu, v as u8);
            }
        }
    }
    img
}

/// A sheet rendered at 200 dpi.
pub fn sheet(e: &LegacyEngine, m: &Marks, seed: u64) -> GrayImage {
    let mut img = render(e, &m.fills(e), seed);
    if !m.light.is_empty() {
        // Light marks: the same geometry, 70 % of a solid fill's darkening.
        let faint = render(e, &m.light, seed);
        let plain = render(e, &[], seed);
        for y in 0..img.height() {
            for x in 0..img.width() {
                let (f, p) = (faint.get(x, y), plain.get(x, y));
                if f < p {
                    let v = u32::from(img.get(x, y)) * (255 - (u32::from(p - f) * 7 / 10)) / 255;
                    img.set(x, y, v as u8);
                }
            }
        }
    }
    img
}

/// Bilinear rescale by `num / den` (a 200-dpi page × 3/2 = 300 dpi).
pub fn rescale(img: &GrayImage, num: usize, den: usize) -> GrayImage {
    let (w, h) = (img.width() * num / den, img.height() * num / den);
    let k = den as f64 / num as f64;
    GrayImage::from_fn(w, h, |x, y| {
        bilinear(img, (x as f64 + 0.5) * k, (y as f64 + 0.5) * k, 255)
    })
}

/// Two-level thresholding (a bitonal scanner).
pub fn bitonal(img: &GrayImage, threshold: u8) -> GrayImage {
    GrayImage::from_fn(img.width(), img.height(), |x, y| {
        if img.get(x, y) < threshold { 0 } else { 255 }
    })
}

/// The back of `front` as a scanner sees it through thin paper: mirrored, faint (bleed-through
/// only), on slightly gray paper with sensor noise.
pub fn bleed_through(front: &GrayImage, seed: u64) -> GrayImage {
    let mut rng = SplitMix64::new(seed);
    let w = front.width();
    GrayImage::from_fn(w, front.height(), |x, y| {
        let v = front.get(w - 1 - x, y);
        let ghost = 238.0 - (255.0 - f64::from(v)) * 0.16;
        (ghost + (rng.next_u64() % 7) as f64 - 3.0).clamp(0.0, 255.0) as u8
    })
}

/// A plain blank page (paper with sensor noise) of `w × h` px.
pub fn blank_page(w: usize, h: usize, seed: u64) -> GrayImage {
    let mut rng = SplitMix64::new(seed);
    GrayImage::from_fn(w, h, |_, _| 246 + (rng.next_u64() % 9) as u8)
}

/// PNG (8-bit gray).
pub fn png(img: &GrayImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            img.as_raw(),
            img.width() as u32,
            img.height() as u32,
            image::ExtendedColorType::L8,
        )
        .expect("png");
    out
}

/// JPEG (8-bit gray) at `quality`.
pub fn jpeg(img: &GrayImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(
            img.as_raw(),
            img.width() as u32,
            img.height() as u32,
            image::ExtendedColorType::L8,
        )
        .expect("jpeg");
    out
}

/// One full-page image of a PDF page.
pub struct PdfImage {
    /// Pixels.
    pub width: usize,
    /// Pixels.
    pub height: usize,
    /// 1 (packed, 0 = black) or 8 (gray).
    pub bits: u8,
    /// Encoded sample data (after `filter`).
    pub data: Vec<u8>,
    /// `FlateDecode`, `DCTDecode` or none.
    pub filter: Option<&'static str>,
    /// Capture resolution (sets the page size).
    pub dpi: f64,
}

impl PdfImage {
    /// A bitonal page (pixels < 128 are black), 1-bit, Flate-compressed.
    pub fn bitonal(img: &GrayImage, dpi: f64) -> Self {
        let rb = img.width().div_ceil(8);
        let mut raw = vec![0xFFu8; rb * img.height()];
        for y in 0..img.height() {
            for x in 0..img.width() {
                if img.get(x, y) < 128 {
                    raw[y * rb + x / 8] &= !(0x80 >> (x % 8));
                }
            }
        }
        Self {
            width: img.width(),
            height: img.height(),
            bits: 1,
            data: miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6),
            filter: Some("FlateDecode"),
            dpi,
        }
    }

    /// A gray JPEG page.
    pub fn jpeg(img: &GrayImage, dpi: f64) -> Self {
        Self {
            width: img.width(),
            height: img.height(),
            bits: 8,
            data: jpeg(img, 90),
            filter: Some("DCTDecode"),
            dpi,
        }
    }
}

/// A PDF with one full-page image per page.
pub fn pdf(pages: &[PdfImage]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let n = pages.len();
    // Objects: 1 catalog, 2 pages, then per page: page, content, image.
    let page_obj = |k: usize| 3 + 3 * k;
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {n} >>",
            (0..n)
                .map(|k| format!("{} 0 R", page_obj(k)))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .into_bytes(),
    ];
    for (k, p) in pages.iter().enumerate() {
        let (w_pt, h_pt) = (
            p.width as f64 * 72.0 / p.dpi,
            p.height as f64 * 72.0 / p.dpi,
        );
        let content = format!("q {w_pt:.4} 0 0 {h_pt:.4} 0 0 cm /Im0 Do Q");
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w_pt:.4} {h_pt:.4}] \
                 /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                page_obj(k) + 2,
                page_obj(k) + 1
            )
            .into_bytes(),
        );
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len() + 1
            )
            .into_bytes(),
        );
        let mut img = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray \
             /BitsPerComponent {} {}/Length {} >>\nstream\n",
            p.width,
            p.height,
            p.bits,
            p.filter
                .map(|f| format!("/Filter /{f} "))
                .unwrap_or_default(),
            p.data.len()
        )
        .into_bytes();
        img.extend_from_slice(&p.data);
        img.extend_from_slice(b"\nendstream");
        objects.push(img);
    }
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A legacy IAU exam definition.
pub fn legacy_def(title: &str, questions: u16, forms: &[&str], key: KeySource) -> ExamDefinition {
    let mut d = ExamDefinition::new(
        title,
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        questions,
        5,
        forms.iter().map(|f| (*f).to_string()).collect(),
        CancelMode::EraseOnly,
    );
    d.key = key;
    d.validate().unwrap();
    d
}

/// Grade options writing a project package below `out`.
pub fn options(out: &Path, threads: usize) -> GradeOptions {
    GradeOptions {
        threads,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-17T12:00:00Z".into()),
        operator: "principal:test".into(),
        max_pages: None,
    }
}

/// Grades `inputs` into `out` (project + exports).
pub fn grade(def: &ExamDefinition, inputs: &[PathBuf], out: &Path, threads: usize) -> ExamProject {
    let mut p = ExamProject::grade(def, inputs, &options(out, threads), &|_| {}).unwrap();
    p.export(out).unwrap();
    p
}

/// The source file name of booklet `bi`'s first page.
pub fn source_of(p: &ExamProject, bi: usize) -> String {
    let b = &p.session.booklets[bi];
    p.session.sources[p.session.pages[b.pages[0]].source]
        .name
        .clone()
}

/// Writes `bytes` to `dir/name`, creating `dir`.
pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let f = dir.join(name);
    std::fs::write(&f, bytes).unwrap();
    f
}
