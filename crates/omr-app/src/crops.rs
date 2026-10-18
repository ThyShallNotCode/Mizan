//! Crop rendering for review (§13.3): the registered region of the working image as RGBA8
//! (straight alpha), with the bubble overlays in crop pixels for the UI to draw.

use crate::review::{CropGeom, OverlayBubble};
use omr_imaging::GrayImage;
use std::sync::Arc;

/// An RGBA8 crop, row-major, `width * height * 4` bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Crop {
    /// Width (px).
    pub width: u32,
    /// Height (px).
    pub height: u32,
    /// Pixels.
    pub rgba: Arc<[u8]>,
    /// Bubble overlays (crop px).
    pub bubbles: Vec<OverlayBubble>,
}

fn rgba_of(g: &GrayImage) -> Arc<[u8]> {
    let mut v = Vec::with_capacity(g.width() * g.height() * 4);
    for &p in g.as_raw() {
        v.extend_from_slice(&[p, p, p, 255]);
    }
    v.into()
}

/// Renders a crop; pixels outside the page are white. `overlays` = false drops the bubble
/// list (blind items, §13.4).
pub fn render(img: &GrayImage, geom: &CropGeom, overlays: bool) -> Crop {
    let [x, y, w, h] = geom.rect;
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let c = img.crop(x as isize, y as isize, w, h, 255);
    Crop {
        width: w as u32,
        height: h as u32,
        rgba: rgba_of(&c),
        bubbles: if overlays {
            geom.bubbles.clone()
        } else {
            Vec::new()
        },
    }
}

/// A whole-page view scaled by exact area coverage to at most `max_width` px.
pub fn thumbnail(img: &GrayImage, max_width: usize) -> Crop {
    let s = if img.width() > max_width && max_width > 0 {
        omr_imaging::resample_area(img, max_width as u32, img.width() as u32)
    } else {
        img.clone()
    };
    Crop {
        width: s.width() as u32,
        height: s.height() as u32,
        rgba: rgba_of(&s),
        bubbles: Vec::new(),
    }
}

/// PNG bytes of a crop (for the review-queue export and the CLI).
pub fn png(c: &Crop) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(c.width, c.height, c.rgba.to_vec())
        .ok_or_else(|| "crop size".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::OverlayState;

    #[test]
    fn crop_pads_with_white_and_keeps_overlays() {
        let img = GrayImage::from_fn(20, 10, |x, _| (x * 10) as u8);
        let g = CropGeom {
            page: 0,
            rect: [-2, 0, 6, 3],
            bubbles: vec![OverlayBubble {
                label: "A".into(),
                cx: 3.0,
                cy: 1.0,
                r: 1.0,
                fill: 0.5,
                state: OverlayState::Ambiguous,
            }],
            threshold: 0.2,
            band: (0.18, 0.22),
        };
        let c = render(&img, &g, true);
        assert_eq!((c.width, c.height), (6, 3));
        assert_eq!(&c.rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&c.rgba[8..12], &[0, 0, 0, 255]);
        assert_eq!(c.bubbles.len(), 1);
        assert!(render(&img, &g, false).bubbles.is_empty());
        let t = thumbnail(&img, 10);
        assert_eq!((t.width, t.height), (10, 5));
        assert!(png(&t).unwrap().starts_with(b"\x89PNG"));
    }
}
