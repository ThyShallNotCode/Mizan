//! Renders the app icon from its vector sources: the Windows `.ico` embedded by `omr-winres` and the
//! PNGs the Linux package installs.
//!
//! ```text
//! cargo run -p shell-slint --example brand_icons
//! ```
//!
//! 16-40 px come from the pixel-fitted drawings in `ui/assets/brand/icon-<n>.svg`, 48 px and up
//! from `ui/assets/icon.svg`. The sizes are the ones Windows asks for at 100-200 % scaling
//! (title bar, taskbar, Start, Explorer); 64 px and up are stored as PNG, the rest as 32-bit DIBs.

use std::fs;
use std::path::{Path, PathBuf};

const ICO_SIZES: [u32; 15] = [16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 128, 256];
const SMALL: [u32; 7] = [16, 20, 24, 30, 32, 36, 40];
const LINUX_SIZES: [u32; 2] = [256, 512];

fn render(svg: &Path, size: u32) -> tiny_skia::Pixmap {
    let data = fs::read(svg).unwrap_or_else(|e| panic!("{}: {e}", svg.display()));
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
        .unwrap_or_else(|e| panic!("{}: {e}", svg.display()));
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
}

/// A 32-bit BGRA DIB with an all-zero AND mask (the alpha channel carries transparency).
fn dib(p: &tiny_skia::Pixmap) -> Vec<u8> {
    let (w, h) = (p.width(), p.height());
    let mut out = Vec::new();
    for v in [40u32, w, 2 * h] {
        out.extend(v.to_le_bytes()); // biSize, biWidth, biHeight (image + mask)
    }
    out.extend(1u16.to_le_bytes()); // planes
    out.extend(32u16.to_le_bytes()); // bit count
    out.extend([0u8; 24]); // BI_RGB, no size or resolution, no palette
    for y in (0..h).rev() {
        for x in 0..w {
            let c = p.pixel(x, y).expect("pixel").demultiply();
            out.extend([c.blue(), c.green(), c.red(), c.alpha()]);
        }
    }
    let mask_row = w.div_ceil(32) * 4;
    out.extend(vec![0u8; (mask_row * h) as usize]);
    out
}

fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [0u16, 1, images.len() as u16] {
        out.extend(v.to_le_bytes());
    }
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in images {
        let w = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([w, w, 0, 0]);
        out.extend(1u16.to_le_bytes()); // planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend(data);
    }
    out
}

fn main() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let assets = crate_dir.join("ui/assets");
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES
        .iter()
        .map(|&s| {
            let svg = if SMALL.contains(&s) {
                assets.join(format!("brand/icon-{s}.svg"))
            } else {
                assets.join("icon.svg")
            };
            let p = render(&svg, s);
            let data = if s >= 64 {
                p.encode_png().expect("png")
            } else {
                dib(&p)
            };
            (s, data)
        })
        .collect();
    let ico_path = crate_dir.join("../omr-winres/assets/mizan.ico");
    fs::write(&ico_path, ico(&images)).expect("write mizan.ico");
    println!("{} ({} sizes)", ico_path.display(), images.len());
    for s in LINUX_SIZES {
        let out = assets.join(format!("brand/mizan-{s}.png"));
        let png = render(&assets.join("icon.svg"), s)
            .encode_png()
            .expect("png");
        fs::write(&out, png).expect("write png");
        println!("{}", out.display());
    }
}
