//! Capture simulation (spec §23.3 stage 7): the marked page is warped by a homography (skew,
//! translation, scale, shear, slight perspective), tone-mapped (paper white, toner black,
//! gamma, illumination gradient), blurred, given sensor noise and optional streaks, then encoded.

use crate::profile::{CaptureParams, invert};
use image::{GrayImage, Luma};
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use std::sync::OnceLock;

/// The captured page.
#[derive(Clone, Debug)]
pub struct Captured {
    /// The captured gray image.
    pub image: GrayImage,
    /// Row-major homography from template mm to captured-image pixel coordinates.
    pub mm_to_px: [f64; 9],
}

/// 4,096 standard-normal samples (Box–Muller from a fixed seed), indexed by random bits.
fn normal_table() -> &'static [f32; 4096] {
    static T: OnceLock<Box<[f32; 4096]>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new([0f32; 4096]);
        let mut s: u64 = 0x005E_ED0F_0DD5;
        let mut next = || {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((s >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        for i in (0..4096).step_by(2) {
            let (u1, u2) = (next(), next());
            let r = (-2.0 * u1.ln()).sqrt();
            let th = std::f64::consts::TAU * u2;
            t[i] = (r * th.cos()) as f32;
            t[i + 1] = (r * th.sin()) as f32;
        }
        t
    })
}

fn gaussian_kernel(sigma: f64) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as i32;
    let mut k: Vec<f32> = (-r..=r)
        .map(|i| (-(f64::from(i * i)) / (2.0 * sigma * sigma)).exp() as f32)
        .collect();
    let s: f32 = k.iter().sum();
    for v in &mut k {
        *v /= s;
    }
    k
}

fn blur(buf: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma <= 0.05 || w == 0 || h == 0 {
        return;
    }
    let k = gaussian_kernel(sigma);
    let r = k.len() / 2;
    let mut tmp = vec![0f32; buf.len()];
    // Horizontal pass: clamp only near the borders so the interior loop vectorises.
    for y in 0..h {
        let row = &buf[y * w..(y + 1) * w];
        let out = &mut tmp[y * w..(y + 1) * w];
        for x in 0..w {
            if x >= r && x + r < w {
                let win = &row[x - r..=x + r];
                out[x] = win.iter().zip(&k).map(|(a, b)| a * b).sum();
            } else {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    let xx = (x + i).saturating_sub(r).min(w - 1);
                    acc += kv * row[xx];
                }
                out[x] = acc;
            }
        }
    }
    // Vertical pass: accumulate whole rows (contiguous, vectorisable).
    for y in 0..h {
        let out = &mut buf[y * w..(y + 1) * w];
        out.fill(0.0);
        for (i, kv) in k.iter().enumerate() {
            let yy = (y + i).saturating_sub(r).min(h - 1);
            let src = &tmp[yy * w..(yy + 1) * w];
            for (o, s) in out.iter_mut().zip(src) {
                *o += kv * s;
            }
        }
    }
}

/// Simulates scanning `page` (a clean-plus-marks raster rendered at `p.dpi`).
pub fn capture(page: &GrayImage, p: &CaptureParams, rng: &mut ChaCha8Rng) -> Captured {
    let h_mm = p.homography();
    let hi = invert(&h_mm).unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    let (w, h) = p.frame_px();
    let (sw, sh) = page.dimensions();
    let k_src = f64::from(p.dpi) / 25.4;
    let src = page.as_raw();
    let (wu, hu) = (w as usize, h as usize);
    let mut buf = vec![0f32; wu * hu];
    let span = p.paper - p.black;
    let lut: Vec<f32> = (0..=255)
        .map(|v| (p.black + span * (f64::from(v) / 255.0).powf(p.gamma)) as f32)
        .collect();
    let backing = f32::from(p.backing);
    let (gx, gy) = p.illumination;
    let (sw_f, sh_f) = (f64::from(sw) - 0.5, f64::from(sh) - 0.5);
    let swu = sw as usize;
    let illum_x: Vec<f32> = (0..wu)
        .map(|u| (gx * ((u as f64 + 0.5) / wu as f64 - 0.5)) as f32)
        .collect();
    for v in 0..hu {
        let vc = v as f64 + 0.5;
        let illum_y = gy * (vc / hu as f64 - 0.5);
        // Homography numerators and denominator at u = 0.5, stepped by one pixel.
        let mut nx = hi[0] * 0.5 + hi[1] * vc + hi[2];
        let mut ny = hi[3] * 0.5 + hi[4] * vc + hi[5];
        let mut dw = hi[6] * 0.5 + hi[7] * vc + hi[8];
        let row = &mut buf[v * wu..(v + 1) * wu];
        for (u, out) in row.iter_mut().enumerate() {
            let inv = 1.0 / dw;
            let sx = nx * inv * k_src - 0.5;
            let sy = ny * inv * k_src - 0.5;
            nx += hi[0];
            ny += hi[3];
            dw += hi[6];
            let illum = 1.0 + illum_x[u] + illum_y as f32;
            *out = if sx < -0.5 || sy < -0.5 || sx > sw_f || sy > sh_f {
                backing * illum
            } else {
                // Truncation equals floor here (negative inputs clamp to 0) and avoids a libm
                // call per pixel on the baseline x86-64 target.
                let xi = if sx > 0.0 { sx as usize } else { 0 };
                let yi = if sy > 0.0 { sy as usize } else { 0 };
                let fx = (sx - xi as f64).clamp(0.0, 1.0) as f32;
                let fy = (sy - yi as f64).clamp(0.0, 1.0) as f32;
                let xj = (xi + 1).min(swu - 1);
                let yj = (yi + 1).min(sh as usize - 1);
                let (r0, r1) = (yi * swu, yj * swu);
                let a = f32::from(src[r0 + xi]);
                let b = f32::from(src[r0 + xj]);
                let c = f32::from(src[r1 + xi]);
                let d = f32::from(src[r1 + xj]);
                let top = a + (b - a) * fx;
                let bot = c + (d - c) * fx;
                let s = top + (bot - top) * fy;
                let lo = (s as usize).min(254);
                let t = s - lo as f32;
                (lut[lo] + (lut[lo + 1] - lut[lo]) * t) * illum
            };
        }
    }
    blur(&mut buf, wu, hu, p.blur_sigma);
    // Noise and quantisation.
    let table = normal_table();
    let sigma = p.noise_sigma as f32;
    let mut out = Vec::with_capacity(wu * hu);
    let mut bits = 0u64;
    let mut left = 0;
    for v in buf {
        if left == 0 {
            bits = rng.next_u64();
            left = 5;
        }
        let n = table[(bits & 0xFFF) as usize];
        bits >>= 12;
        left -= 1;
        out.push((v + sigma * n + 0.5).clamp(0.0, 255.0) as u8);
    }
    let mut img = GrayImage::from_raw(w, h, out).unwrap_or_else(|| GrayImage::new(w, h));
    if let Some(s) = p.streak {
        for x in s.x..(s.x + s.width).min(w) {
            for y in 0..h {
                let px = img.get_pixel_mut(x, y);
                *px = Luma([(f64::from(px.0[0]) + s.delta).round().clamp(0.0, 255.0) as u8]);
            }
        }
    }
    Captured {
        image: img,
        mm_to_px: h_mm,
    }
}

/// Encodes as baseline JPEG at `quality` with the dpi in the JFIF header.
pub fn encode_jpeg(img: &GrayImage, quality: u8, dpi: u32) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(
        dpi.min(u32::from(u16::MAX)) as u16,
    ));
    enc.encode(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::L8,
    )?;
    Ok(buf)
}

/// Encodes as PNG.
pub fn encode_png(img: &GrayImage) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Profile, apply_h};
    use rand::SeedableRng;

    #[test]
    fn kernel_is_normalised() {
        let k = gaussian_kernel(0.8);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(k.len() % 2, 1);
    }

    #[test]
    fn normal_table_has_unit_variance() {
        let t = normal_table();
        let m: f64 = t.iter().map(|v| f64::from(*v)).sum::<f64>() / 4096.0;
        let var: f64 = t.iter().map(|v| (f64::from(*v) - m).powi(2)).sum::<f64>() / 4096.0;
        assert!(m.abs() < 0.05 && (var - 1.0).abs() < 0.08, "{m} {var}");
    }

    #[test]
    fn a_black_dot_lands_where_the_homography_says() {
        let dpi = 100;
        let k = f64::from(dpi) / 25.4;
        let (pw, ph) = ((210.0 * k).round() as u32, (297.0 * k).round() as u32);
        let mut page = GrayImage::from_pixel(pw, ph, Luma([255]));
        // A 3 × 3 px black square centred on (50 mm, 80 mm).
        let (cx, cy) = ((50.0 * k) as u32, (80.0 * k) as u32);
        for y in cy - 1..=cy + 1 {
            for x in cx - 1..=cx + 1 {
                page.put_pixel(x, y, Luma([0]));
            }
        }
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let mut p = CaptureParams::sample(Profile::Hard, dpi, &mut rng);
        p.noise_sigma = 0.0;
        p.streak = None;
        let cap = capture(&page, &p, &mut rng);
        let (u, v) = apply_h(
            &cap.mm_to_px,
            (f64::from(cx) + 0.5) / k,
            (f64::from(cy) + 0.5) / k,
        );
        let px = cap.image.get_pixel(u as u32, v as u32).0[0];
        assert!(
            f64::from(px) < p.paper - 0.4 * (p.paper - p.black),
            "dot not at ({u}, {v}): {px}"
        );
    }
}
