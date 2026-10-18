//! Simulated phone photos of the committed IAU blank form (synthetic data only, truth by
//! construction): marks painted at the template's bubble centres, then a projective camera
//! (rotation, anisotropic scale, keystone), 120–160 ppi, colour cast, vignetting, a soft
//! shadow, a table around the sheet or a "scanner app" crop with whitened paper, lens blur,
//! sensor noise and RGB JPEG — decoded and brought to the working resolution exactly as
//! `omr-app` does for a JPEG page in a PDF (luma, then area resampling by the placement dpi).

#![allow(dead_code)]

use omr_forms::legacy::LegacyField;
use omr_imaging::{CaptureKind, GrayImage, bilinear, decode};
use omr_model::Response;
use omr_recog::{LegacyEngine, LegacyPageResult, PageRequest};
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;

/// Questions read on the simulated exam.
pub const QUESTIONS: u16 = 29;

/// What was marked.
#[derive(Clone, Debug)]
pub struct Truth {
    /// Per question: the marked option, or blank.
    pub answers: Vec<Option<u8>>,
    /// Student-number digits (10 columns).
    pub student: Vec<u8>,
    /// Version index (A–D).
    pub version: u8,
}

/// The camera and light of one photo.
#[derive(Clone, Debug)]
pub struct Photo {
    /// Sheet resolution in the photo (pixels per inch at the sheet centre).
    pub ppi: f64,
    /// Rotation (degrees).
    pub rotation_deg: f64,
    /// Anisotropy s_x / s_y − 1.
    pub anisotropy: f64,
    /// Keystone: relative scale change from the centre to the right / bottom edge.
    pub keystone: (f64, f64),
    /// Extra translation of the sheet in the frame (photo px).
    pub offset: (f64, f64),
    /// Extra translation by whole lattice pitches (answer rows / options), the adversarial
    /// inputs: the sheet sits exactly where its neighbouring row or column would.
    pub pitch_shift: (i32, i32),
    /// Scanner-app style: tight crop, no table, paper whitened by a levels stretch.
    pub tight: bool,
    /// Light colour (R, G, B multipliers).
    pub cast: [f64; 3],
    /// Vignetting depth at the corners (0–1).
    pub vignette: f64,
    /// Shadow: (direction angle, position along it as a fraction, depth 0–1, softness px).
    pub shadow: Option<(f64, f64, f64, f64)>,
    /// Lens blur sigma (photo px).
    pub blur: f64,
    /// Sensor noise sigma (gray levels).
    pub noise: f64,
    /// JPEG quality.
    pub quality: u8,
    /// Relative error of the placement dpi the app uses (the photo's margins).
    pub dpi_error: f64,
    /// Exposure override (light multiplier; `None`: drawn from 0.82–1.0). Below 0.5 is a dark
    /// photo, above 1.0 overexposes (the paper clips at white and light marks fade).
    pub exposure: Option<f64>,
}

impl Photo {
    /// Draws a photo setup; `pitch_shift` is set by the caller for the adversarial cases.
    pub fn sample(rng: &mut ChaCha8Rng) -> Self {
        let tight = rng.random_bool(0.4);
        let sym = |rng: &mut ChaCha8Rng, a: f64| rng.random_range(-a..=a);
        let warm = rng.random_range(-0.12..=0.12);
        Photo {
            ppi: rng.random_range(120.0..=160.0),
            rotation_deg: sym(rng, 4.0),
            anisotropy: sym(rng, 0.06),
            keystone: (sym(rng, 0.05), sym(rng, 0.05)),
            offset: (sym(rng, 25.0), sym(rng, 25.0)),
            pitch_shift: (0, 0),
            tight,
            cast: [1.0 + warm, 1.0 - warm.abs() / 3.0, 1.0 - 1.5 * warm],
            vignette: if tight {
                0.0
            } else {
                rng.random_range(0.0..=0.35)
            },
            shadow: (!tight && rng.random_bool(0.6)).then(|| {
                (
                    rng.random_range(0.0..std::f64::consts::TAU),
                    rng.random_range(0.2..=0.8),
                    rng.random_range(0.15..=0.45),
                    rng.random_range(40.0..=250.0),
                )
            }),
            blur: rng.random_range(0.35..=1.0),
            noise: rng.random_range(0.5..=3.5),
            quality: rng.random_range(70..=92),
            dpi_error: sym(rng, 0.04),
            exposure: None,
        }
    }
}

fn index(engine: &LegacyEngine, field: LegacyField, group: u16, option: u8) -> usize {
    engine
        .template()
        .bubbles()
        .iter()
        .position(|b| b.field == field && b.group == group && b.option == option)
        .expect("bubble")
}

/// Random marks: about 1 question in 12 left blank, every ID column and the version marked.
pub fn random_truth(rng: &mut ChaCha8Rng) -> Truth {
    Truth {
        answers: (0..QUESTIONS)
            .map(|_| (!rng.random_bool(0.08)).then(|| rng.random_range(0..5u8)))
            .collect(),
        student: (0..10).map(|_| rng.random_range(0..10u8)).collect(),
        version: rng.random_range(0..2u8),
    }
}

/// The blank form with pencil or pen fills at the marked bubbles (template space, 200 dpi).
pub fn marked_sheet(engine: &LegacyEngine, truth: &Truth, rng: &mut ChaCha8Rng) -> GrayImage {
    let mut img = engine.blank().clone();
    let r = engine.template().bubble_radius_px();
    let fill = |img: &mut GrayImage, j: usize, rng: &mut ChaCha8Rng| {
        let c = engine.template().bubbles()[j].centre;
        let rr = r * rng.random_range(0.78..=0.92);
        let ink: f64 = rng.random_range(25.0..=95.0);
        let (x0, x1) = ((c.x - rr).floor() as i64, (c.x + rr).ceil() as i64);
        let (y0, y1) = ((c.y - rr).floor() as i64, (c.y + rr).ceil() as i64);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f64 + 0.5 - c.x, y as f64 + 0.5 - c.y);
                if dx * dx + dy * dy > rr * rr
                    || x < 0
                    || y < 0
                    || x as usize >= img.width()
                    || y as usize >= img.height()
                {
                    continue;
                }
                let noise: f64 = rng.random_range(-18.0..=18.0);
                let v = (ink + noise).clamp(0.0, 255.0) as u8;
                let old = img.get(x as usize, y as usize);
                img.set(x as usize, y as usize, old.min(v));
            }
        }
    };
    for (q, a) in truth.answers.iter().enumerate() {
        if let Some(o) = a {
            fill(
                &mut img,
                index(engine, LegacyField::Answers, q as u16, *o),
                rng,
            );
        }
    }
    for (c, &d) in truth.student.iter().enumerate() {
        fill(
            &mut img,
            index(engine, LegacyField::StudentNumber, c as u16, d),
            rng,
        );
    }
    fill(
        &mut img,
        index(engine, LegacyField::Version, 0, truth.version),
        rng,
    );
    img
}

fn mul3(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    let mut o = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            o[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    o
}

fn inv3(h: &[f64; 9]) -> [f64; 9] {
    let c00 = h[4] * h[8] - h[5] * h[7];
    let c01 = h[5] * h[6] - h[3] * h[8];
    let c02 = h[3] * h[7] - h[4] * h[6];
    let det = h[0] * c00 + h[1] * c01 + h[2] * c02;
    [
        c00 / det,
        (h[2] * h[7] - h[1] * h[8]) / det,
        (h[1] * h[5] - h[2] * h[4]) / det,
        c01 / det,
        (h[0] * h[8] - h[2] * h[6]) / det,
        (h[2] * h[3] - h[0] * h[5]) / det,
        c02 / det,
        (h[1] * h[6] - h[0] * h[7]) / det,
        (h[0] * h[4] - h[1] * h[3]) / det,
    ]
}

fn gaussian_blur(buf: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma <= 0.05 {
        return;
    }
    let r = (3.0 * sigma).ceil() as usize;
    let mut k: Vec<f32> = (0..=2 * r)
        .map(|i| {
            let d = i as f64 - r as f64;
            (-d * d / (2.0 * sigma * sigma)).exp() as f32
        })
        .collect();
    let s: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    let mut tmp = vec![0f32; buf.len()];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x + i).saturating_sub(r).min(w - 1);
                acc += kv * buf[y * w + xx];
            }
            tmp[y * w + x] = acc;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let yy = (y + i).saturating_sub(r).min(h - 1);
                acc += kv * tmp[yy * w + x];
            }
            buf[y * w + x] = acc;
        }
    }
}

/// Renders the marked sheet as an RGB JPEG phone photo; returns the JPEG bytes and the
/// placement dpi the app would use.
pub fn render(engine: &LegacyEngine, sheet: &GrayImage, p: &Photo, seed: u64) -> (Vec<u8>, u32) {
    let (jpeg, dpi, _) = render_with_truth(engine, sheet, p, seed);
    (jpeg, dpi)
}

/// [`render`], also returning the true template-px → photo-px homography (row-major).
pub fn render_with_truth(
    engine: &LegacyEngine,
    sheet: &GrayImage,
    p: &Photo,
    seed: u64,
) -> (Vec<u8>, u32, [f64; 9]) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let (tw, th) = (sheet.width() as f64, sheet.height() as f64);
    let s = p.ppi / 200.0;
    let (sx, sy) = (
        s * (1.0 + p.anisotropy / 2.0),
        s * (1.0 - p.anisotropy / 2.0),
    );
    // Frame: the sheet plus a margin (none for the tight crop, which also cuts ≈ 1 %).
    let margin = if p.tight { -0.01 } else { 0.07 };
    let fw = (tw * sx * (1.0 + 2.0 * margin)).round() as usize;
    let fh = (th * sy * (1.0 + 2.0 * margin)).round() as usize;
    // Template px → photo px: centre, keystone, R·S, translate (+ whole pitches, adversarial).
    let (kx, ky) = (p.keystone.0 / (tw / 2.0), p.keystone.1 / (th / 2.0));
    let t1 = [1.0, 0.0, -tw / 2.0, 0.0, 1.0, -th / 2.0, 0.0, 0.0, 1.0];
    let key = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -kx, -ky, 1.0];
    let a = p.rotation_deg.to_radians();
    let (c, sn) = (a.cos(), a.sin());
    let lin = [c * sx, -sn * sy, 0.0, sn * sx, c * sy, 0.0, 0.0, 0.0, 1.0];
    let shift_x = p.offset.0 + f64::from(p.pitch_shift.0) * 50.4 * sx;
    let shift_y = p.offset.1 + f64::from(p.pitch_shift.1) * 41.62 * sy;
    let t2 = [
        1.0,
        0.0,
        fw as f64 / 2.0 + shift_x,
        0.0,
        1.0,
        fh as f64 / 2.0 + shift_y,
        0.0,
        0.0,
        1.0,
    ];
    let h = mul3(&t2, &mul3(&lin, &mul3(&key, &t1)));
    let hi = inv3(&h);
    // Reflectance (sheet) or table, blurred.
    let table = rng.random_range(0.15..=0.45);
    let mut refl = vec![0f32; fw * fh];
    for v in 0..fh {
        for u in 0..fw {
            let (x, y) = (u as f64 + 0.5, v as f64 + 0.5);
            let w = hi[6] * x + hi[7] * y + hi[8];
            let tx = (hi[0] * x + hi[1] * y + hi[2]) / w;
            let ty = (hi[3] * x + hi[4] * y + hi[5]) / w;
            refl[v * fw + u] = if tx < 0.0 || ty < 0.0 || tx >= tw || ty >= th {
                table as f32
            } else {
                f32::from(bilinear(sheet, tx, ty, 255)) / 255.0
            };
        }
    }
    gaussian_blur(&mut refl, fw, fh, p.blur);
    // Light: exposure, vignetting, a soft shadow, the colour cast; flare lifts the blacks.
    let exposure = rng.random_range(0.82..=1.0);
    let exposure = p.exposure.unwrap_or(exposure);
    let flare = rng.random_range(0.03..=0.10);
    let (cxf, cyf) = (fw as f64 / 2.0, fh as f64 / 2.0);
    let rmax = (cxf * cxf + cyf * cyf).sqrt();
    let mut rgb = Vec::with_capacity(fw * fh * 3);
    for v in 0..fh {
        for u in 0..fw {
            let (x, y) = (u as f64 + 0.5, v as f64 + 0.5);
            let rr = ((x - cxf).powi(2) + (y - cyf).powi(2)).sqrt() / rmax;
            let mut light = exposure * (1.0 - p.vignette * rr * rr);
            if let Some((ang, pos, depth, soft)) = p.shadow {
                let (dx, dy) = (ang.cos(), ang.sin());
                let along = (x - cxf) * dx + (y - cyf) * dy;
                let edge = (pos - 0.5) * 2.0 * rmax * 0.7;
                let t = ((along - edge) / soft).clamp(-1.0, 1.0) * 0.5 + 0.5;
                light *= 1.0 - depth * t;
            }
            let rho = f64::from(refl[v * fw + u]);
            for k in 0..3 {
                let val = 255.0 * (flare + (1.0 - flare) * rho * light * p.cast[k])
                    + p.noise
                        * (rng.random::<f64>() + rng.random::<f64>() + rng.random::<f64>() - 1.5)
                        * 2.0;
                rgb.push(val.clamp(0.0, 255.0));
            }
        }
    }
    // Scanner-app style: levels stretch so the paper saturates (like the real key photos).
    let bytes: Vec<u8> = if p.tight {
        let mut lum: Vec<f64> = rgb.chunks(3).map(|c| (c[0] + c[1] + c[2]) / 3.0).collect();
        lum.sort_by(f64::total_cmp);
        let hi_l = lum[lum.len() * 80 / 100].max(1.0);
        let lo_l = lum[lum.len() / 200];
        rgb.iter()
            .map(|&v| ((v - lo_l) * 255.0 / (hi_l - lo_l).max(1.0)).clamp(0.0, 255.0) as u8)
            .collect()
    } else {
        rgb.iter().map(|&v| v.round() as u8).collect()
    };
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, p.quality);
    enc.encode(&bytes, fw as u32, fh as u32, image::ExtendedColorType::Rgb8)
        .expect("jpeg");
    // The placement dpi: the frame's resolution if the PDF page box were the sheet.
    let dpi = (p.ppi * (1.0 + p.dpi_error)).round().max(1.0) as u32;
    let _ = engine;
    (out, dpi, h)
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// Decodes the photo and brings it to the working resolution as `omr-app` does, with the
/// capture class `omr-app` gives a JPEG page in a PDF (production classification: a colour
/// JPEG placed below 200 ppi, or one with a camera's EXIF, is a camera capture).
pub fn to_working(jpeg: &[u8], dpi: u32) -> (GrayImage, CaptureKind) {
    let d = decode(jpeg).expect("decode photo");
    let g = gcd(200, dpi);
    let img = omr_imaging::resample_area(&d.image, 200 / g, dpi / g);
    let kind = if omr_imaging::photo_capture(d.had_colour, Some(dpi))
        || omr_imaging::jpeg_from_camera(jpeg)
    {
        CaptureKind::Camera
    } else {
        omr_imaging::classify_capture(&img)
    };
    (img, kind)
}

/// Single-page read (batch of one: the blank-form prior plus the field offset).
pub fn read(engine: &LegacyEngine, img: &GrayImage, capture: CaptureKind) -> LegacyPageResult {
    let m = engine.measure_page(img);
    let b = if m.registered() {
        engine.batch_baseline(&[&m])
    } else {
        engine.prior_baseline()
    };
    let req = PageRequest {
        n_questions: QUESTIONS,
        high_stakes: false,
        capture,
        page: None,
    };
    engine.decide(&m, &b, &req)
}

/// The outcome of one photo against its truth.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub photos: u32,
    pub refused: u32,
    pub questions: u32,
    pub auto_accepted: u32,
    pub correct_auto: u32,
    pub silent: u32,
    pub review: u32,
    pub id_exact: u32,
    pub id_flagged: u32,
    pub id_wrong: u32,
    pub version_exact: u32,
    pub version_flagged: u32,
    pub version_wrong: u32,
    pub methods: std::collections::BTreeMap<String, u32>,
    pub refusal_codes: std::collections::BTreeMap<String, u32>,
    pub review_reasons: std::collections::BTreeMap<String, u32>,
    /// Reviews per photo (photos with ≥ 1 review).
    pub reviews_per_photo: Vec<u32>,
    /// Local verification: photos with a band of rows that failed, and the smallest band
    /// margin over the photos read.
    pub band_failures: u32,
    pub min_band_margin: Option<f64>,
    pub min_band_floor: Option<f64>,
    /// Answer rows auto-accepted with an erasure residue logged (MRK-003 INFO), and the rows
    /// read with a single fill (the denominator: rows where a residue could be logged).
    pub residue_rows: u32,
    pub single_rows: u32,
    /// Photos decided as photos (the wider camera band, `LegacyPageResult::photo`).
    pub decided_as_photo: u32,
}

impl Tally {
    pub fn add(&mut self, r: &LegacyPageResult, t: &Truth) {
        self.photos += 1;
        *self
            .methods
            .entry(format!("{:?}", r.registration.method))
            .or_default() += 1;
        let refused = r
            .questions
            .iter()
            .all(|q| matches!(q.response, Response::Unreadable { .. }));
        if refused {
            self.refused += 1;
            for e in r.exceptions.iter().filter(|e| e.code.starts_with("REG")) {
                let head: String = e.detail.chars().take(60).collect();
                *self
                    .refusal_codes
                    .entry(format!("{}/{:?} {head}", e.code, e.severity))
                    .or_default() += 1;
            }
        }
        if !refused {
            if r.registration.windows_failed > 0 {
                self.band_failures += 1;
            }
            let m = r.registration.window_margin;
            self.min_band_margin = Some(self.min_band_margin.map_or(m, |x: f64| x.min(m)));
            let f = r.registration.window_floor;
            self.min_band_floor = Some(self.min_band_floor.map_or(f, |x: f64| x.min(f)));
        }
        if r.photo {
            self.decided_as_photo += 1;
        }
        for q in r.questions.iter().filter(|q| q.auto_accepted) {
            if matches!(q.response, Response::Single(_)) {
                self.single_rows += 1;
                if q.exceptions.iter().any(|e| e.code == "MRK-003") {
                    self.residue_rows += 1;
                }
            }
        }
        let mut reviews = 0;
        for q in &r.questions {
            if !q.auto_accepted {
                reviews += 1;
                for e in &q.exceptions {
                    *self
                        .review_reasons
                        .entry(format!(
                            "{}:{}",
                            e.code,
                            e.sub_reason.clone().unwrap_or_default()
                        ))
                        .or_default() += 1;
                }
            }
        }
        if reviews > 0 && !refused {
            self.reviews_per_photo.push(reviews);
        }
        for q in &r.questions {
            self.questions += 1;
            let want = match t.answers[usize::from(q.question)] {
                Some(o) => Response::Single(o),
                None => Response::Blank,
            };
            if q.auto_accepted {
                self.auto_accepted += 1;
                if q.response == want {
                    self.correct_auto += 1;
                } else {
                    self.silent += 1;
                }
            } else {
                self.review += 1;
            }
        }
        let id: String = t.student.iter().map(|d| char::from(b'0' + d)).collect();
        if r.student_number.confident() {
            if r.student_number.text() == id {
                self.id_exact += 1;
            } else {
                self.id_wrong += 1;
            }
        } else {
            self.id_flagged += 1;
        }
        let want_v = ["A", "B", "C", "D"][usize::from(t.version)];
        match r.version.value() {
            Some(v) if v == want_v => self.version_exact += 1,
            Some(_) => self.version_wrong += 1,
            None => self.version_flagged += 1,
        }
    }
}
