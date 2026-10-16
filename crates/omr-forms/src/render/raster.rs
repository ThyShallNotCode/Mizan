//! Rasterisation through `resvg` to 8-bit gray at any resolution.
//!
//! **Pixel convention.** A page point at `(x, y)` mm lands at `(x · dpi / 25.4, y · dpi / 25.4)`
//! in image coordinates, where pixel `(i, j)` covers `[i, i+1) × [j, j+1)` (its centre is at
//! `(i + 0.5, j + 0.5)`). The image is `round(210 · dpi / 25.4)` × `round(297 · dpi / 25.4)`.

use super::RenderError;
use super::fonts::{SANS_FAMILY, raster_fontdb};
use super::svg::{Layer, layer_svg, variable_regions};
use crate::geometry::{A4_H, A4_W};
use crate::instance::SheetInstance;
use crate::units::{UM_PER_INCH, Um};
use image::GrayImage;
use resvg::tiny_skia;
use resvg::usvg;

/// Pixels per millimetre at `dpi`.
pub fn px_per_mm(dpi: f32) -> f64 {
    f64::from(dpi) / 25.4
}

/// Image size of an A4 page at `dpi`.
pub fn page_size_px(dpi: f32) -> (u32, u32) {
    let f = |um: Um| (f64::from(um) * f64::from(dpi) / UM_PER_INCH).round() as u32;
    (f(A4_W), f(A4_H))
}

fn parse(svg: &str) -> Result<usvg::Tree, RenderError> {
    let opt = usvg::Options {
        fontdb: raster_fontdb(),
        font_family: SANS_FAMILY.to_owned(),
        ..usvg::Options::default()
    };
    usvg::Tree::from_str(svg, &opt).map_err(|e| RenderError::Svg(e.to_string()))
}

/// Scale from tree units to pixels so that 210 mm maps to `210 · dpi / 25.4` px exactly.
fn tree_scale(tree: &usvg::Tree, dpi: f32) -> f32 {
    let w_px = (f64::from(A4_W) * f64::from(dpi) / UM_PER_INCH) as f32;
    w_px / tree.size().width()
}

fn pixmap_to_gray(p: &tiny_skia::Pixmap) -> GrayImage {
    let (w, h) = (p.width(), p.height());
    let mut out = Vec::with_capacity((w * h) as usize);
    for px in p.data().chunks_exact(4) {
        // Opaque white background, so premultiplied equals straight colour.
        let y = (u32::from(px[0]) * 299 + u32::from(px[1]) * 587 + u32::from(px[2]) * 114 + 500)
            / 1000;
        out.push(y as u8);
    }
    GrayImage::from_raw(w, h, out).unwrap_or_else(|| GrayImage::new(w, h))
}

/// Rasterises an A4 SVG (as produced by [`super::svg`]) at `dpi` on white paper.
pub fn rasterize_svg(svg: &str, dpi: f32) -> Result<GrayImage, RenderError> {
    if !(20.0..=1_200.0).contains(&dpi) {
        return Err(RenderError::Dpi(dpi));
    }
    let tree = parse(svg)?;
    let (w, h) = page_size_px(dpi);
    let mut pm = tiny_skia::Pixmap::new(w, h).ok_or(RenderError::Dpi(dpi))?;
    pm.fill(tiny_skia::Color::WHITE);
    let s = tree_scale(&tree, dpi);
    resvg::render(&tree, tiny_skia::Transform::from_scale(s, s), &mut pm.as_mut());
    Ok(pixmap_to_gray(&pm))
}

/// The complete sheet at `dpi`.
pub fn rasterize_sheet(inst: &SheetInstance<'_>, dpi: f32) -> Result<GrayImage, RenderError> {
    rasterize_svg(&layer_svg(inst, Layer::All)?, dpi)
}

/// The static layer only (cache it per form and page; see [`composite_variable`]).
pub fn rasterize_static(inst: &SheetInstance<'_>, dpi: f32) -> Result<GrayImage, RenderError> {
    rasterize_svg(&layer_svg(inst, Layer::Static)?, dpi)
}

/// Draws the per-sheet layer (page codes, echo lines, personalisation) onto a cached static
/// raster of the same form and page. Only the variable regions are rasterised, on the same pixel
/// grid as a full render, then multiplied into `base`.
pub fn composite_variable(
    base: &mut GrayImage,
    inst: &SheetInstance<'_>,
    dpi: f32,
) -> Result<(), RenderError> {
    let (w, h) = page_size_px(dpi);
    if base.dimensions() != (w, h) {
        return Err(RenderError::Size {
            expected: (w, h),
            got: base.dimensions(),
        });
    }
    let tree = parse(&layer_svg(inst, Layer::Variable)?)?;
    let s = tree_scale(&tree, dpi);
    let k = px_per_mm(dpi);
    for r in variable_regions(inst)? {
        let r = r.inflate(600);
        let x0 = ((f64::from(r.x) / 1000.0 * k).floor().max(0.0)) as u32;
        let y0 = ((f64::from(r.y) / 1000.0 * k).floor().max(0.0)) as u32;
        let x1 = ((f64::from(r.right()) / 1000.0 * k).ceil() as u32).min(w);
        let y1 = ((f64::from(r.bottom()) / 1000.0 * k).ceil() as u32).min(h);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let mut pm = tiny_skia::Pixmap::new(x1 - x0, y1 - y0).ok_or(RenderError::Dpi(dpi))?;
        pm.fill(tiny_skia::Color::WHITE);
        let t = tiny_skia::Transform::from_row(s, 0.0, 0.0, s, -(x0 as f32), -(y0 as f32));
        resvg::render(&tree, t, &mut pm.as_mut());
        let ov = pixmap_to_gray(&pm);
        for (dx, dy, p) in ov.enumerate_pixels() {
            let b = base.get_pixel_mut(x0 + dx, y0 + dy);
            b.0[0] = ((u16::from(b.0[0]) * u16::from(p.0[0]) + 127) / 255) as u8;
        }
    }
    Ok(())
}

/// Encodes a gray image as PNG bytes (in memory; no file I/O).
pub fn encode_png(img: &GrayImage) -> Result<Vec<u8>, RenderError> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| RenderError::Encode(e.to_string()))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_sizes() {
        assert_eq!(page_size_px(200.0), (1654, 2339));
        assert_eq!(page_size_px(300.0), (2480, 3508));
    }
}
