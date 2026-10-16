//! Corner-tag detection (§8.3 step 1, §6.4): AprilTag 36h11 tags in the four image-corner
//! windows, located on a decimated copy (the spec's L1 search, §8.2) and decoded at native
//! resolution; orientation and mirroring from the tag IDs (§8.3 step 2).
//!
//! The detector sits behind [`TagDetector`] so that the C `apriltag` fallback (§6.4) or an
//! in-house detector can replace `kornia-apriltag` 0.2.0 without touching the rest of the
//! pipeline. Detections are sorted before use, so the result does not depend on the order in
//! which a detector reports them.

use super::config::TagConfig;
use kornia_apriltag::{AprilTagDecoder, DecodeTagsConfig, family::TagFamilyKind};
use kornia_image::{Image, ImageSize};
use omr_imaging::GrayImage;
use omr_model::PointPx;
use serde::{Deserialize, Serialize};

/// One decoded tag36h11 tag, in the coordinates of the image passed to the detector.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagHit {
    /// Tag ID (0–586).
    pub id: u16,
    /// Hamming distance of the decoded code word.
    pub hamming: u8,
    /// The quad's corners, clockwise on screen (y down), starting anywhere.
    pub corners: [PointPx; 4],
}

impl TagHit {
    /// Mean of the four corners.
    pub fn centre(&self) -> PointPx {
        let (sx, sy) = self
            .corners
            .iter()
            .fold((0.0, 0.0), |(x, y), c| (x + c.x, y + c.y));
        PointPx {
            x: sx / 4.0,
            y: sy / 4.0,
        }
    }

    /// Tag set (IDs 4k … 4k+3, §6.4).
    pub fn set(&self) -> u16 {
        self.id / 4
    }

    /// Corner index within the set: 0 TL, 1 TR, 2 BR, 3 BL.
    pub fn corner(&self) -> usize {
        usize::from(self.id % 4)
    }
}

/// A tag36h11 detector. Implementations must be deterministic for a given image.
pub trait TagDetector: Send + Sync {
    /// Short identifier recorded in diagnostics.
    fn name(&self) -> &'static str;

    /// Detects tag36h11 tags in `img`; the quad search may run at decimation `downscale`
    /// (≥ 1), decoding and corner fitting at native resolution.
    fn detect(&self, img: &GrayImage, downscale: usize) -> Vec<TagHit>;
}

/// `kornia-apriltag` 0.2.0 (Apache-2.0), pinned behind [`TagDetector`] (§6.4).
#[derive(Clone, Copy, Debug, Default)]
pub struct KorniaDetector;

impl TagDetector for KorniaDetector {
    fn name(&self) -> &'static str {
        "kornia-apriltag-0.2.0"
    }

    fn detect(&self, img: &GrayImage, downscale: usize) -> Vec<TagHit> {
        if img.width() < 16 || img.height() < 16 {
            return Vec::new();
        }
        let size = ImageSize {
            width: img.width(),
            height: img.height(),
        };
        let Ok(k) = Image::<u8, 1>::new(size, img.as_raw().to_vec()) else {
            return Vec::new();
        };
        let Ok(mut cfg) = DecodeTagsConfig::new(vec![TagFamilyKind::Tag36H11]) else {
            return Vec::new();
        };
        cfg.downscale_factor = downscale.max(1);
        let Ok(mut dec) = AprilTagDecoder::new(cfg, size) else {
            return Vec::new();
        };
        // A young third-party detector: a panic on odd input becomes "no tags" (REG-001)
        // instead of taking the batch down.
        let decoded =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dec.decode(&k))).ok();
        let Some(Ok(dets)) = decoded else {
            return Vec::new();
        };
        let mut out: Vec<TagHit> = dets
            .into_iter()
            .map(|d| TagHit {
                id: d.id,
                hamming: d.hamming,
                corners: d.quad.corners.map(|c| PointPx {
                    x: f64::from(c.x),
                    y: f64::from(c.y),
                }),
            })
            .filter(|h| h.corners.iter().all(|c| c.x.is_finite() && c.y.is_finite()))
            .collect();
        sort_hits(&mut out);
        out
    }
}

/// Sorts hits by (id, centre) so downstream code never depends on detector order.
pub(crate) fn sort_hits(hits: &mut [TagHit]) {
    hits.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then(a.hamming.cmp(&b.hamming))
            .then(a.centre().y.total_cmp(&b.centre().y))
            .then(a.centre().x.total_cmp(&b.centre().x))
    });
}

/// The outcome of the corner search on one page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagSearch {
    /// Tags of the chosen set, at most one per corner index (TL, TR, BR, BL), in **original**
    /// image coordinates (already un-mirrored back when `mirrored`).
    pub tags: Vec<TagHit>,
    /// The tag set found (§6.4: 0 answer pages, 1 headers, 2 calibration, 3 proofs).
    pub set: Option<u16>,
    /// True if the tags decoded only in the horizontally mirrored image (REG-005).
    pub mirrored: bool,
    /// Every hit of any ID (for diagnostics), in the coordinates of the hypothesis kept.
    pub all_ids: Vec<u16>,
    /// Decimation used for the first pass.
    pub downscale: u32,
}

/// Estimated pixels per millimetre of a scan of an A4 sheet (header DPI is distrusted, §8.2):
/// the short image side covers the paper's 210 mm plus a few millimetres of scan margin.
pub(crate) fn estimate_ppm(width: usize, height: usize) -> f64 {
    (width.min(height) as f64 / 216.0).max(0.5)
}

struct Window {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

fn windows(width: usize, height: usize, side_px: usize) -> [Window; 4] {
    // Windows may overlap on small images; duplicate hits are removed afterwards.
    let w = side_px.clamp(1, width.max(1));
    let h = side_px.clamp(1, height.max(1));
    [
        Window { x0: 0, y0: 0, w, h },
        Window {
            x0: width - w,
            y0: 0,
            w,
            h,
        },
        Window {
            x0: width - w,
            y0: height - h,
            w,
            h,
        },
        Window {
            x0: 0,
            y0: height - h,
            w,
            h,
        },
    ]
}

fn detect_in(
    det: &dyn TagDetector,
    img: &GrayImage,
    win: &Window,
    downscale: usize,
    max_hamming: u8,
) -> Vec<TagHit> {
    let crop = img.crop(win.x0 as isize, win.y0 as isize, win.w, win.h, 255);
    det.detect(&crop, downscale)
        .into_iter()
        .filter(|h| h.hamming <= max_hamming)
        .map(|mut h| {
            for c in &mut h.corners {
                c.x += win.x0 as f64;
                c.y += win.y0 as f64;
            }
            h
        })
        .collect()
}

/// Horizontal mirror of an image.
pub(crate) fn mirror(img: &GrayImage) -> GrayImage {
    let w = img.width();
    GrayImage::from_fn(w, img.height(), |x, y| img.get(w - 1 - x, y))
}

/// Picks the best consistent tag set among `hits`: the known set with the most distinct
/// corners whose layout order is clockwise on screen (a mirrored arrangement is
/// counter-clockwise). Returns the set and one hit per corner index.
fn choose(hits: &[TagHit], cfg: &TagConfig) -> Option<(u16, Vec<TagHit>)> {
    let mut best: Option<(usize, u16, Vec<TagHit>)> = None;
    for &set in &cfg.known_tag_sets {
        let mut per: [Option<TagHit>; 4] = [None; 4];
        for h in hits.iter().filter(|h| h.set() == set) {
            let slot = &mut per[h.corner()];
            // Keep the lowest Hamming distance; ties keep the first (hits are sorted).
            if slot.is_none_or(|p| h.hamming < p.hamming) {
                *slot = Some(*h);
            }
        }
        let chosen: Vec<TagHit> = per.iter().flatten().copied().collect();
        if chosen.len() < 2 || !clockwise(&chosen) {
            continue;
        }
        let rank = chosen.len() * 2 + usize::from(set == cfg.answer_tag_set);
        if best.as_ref().is_none_or(|(r, _, _)| rank > *r) {
            best = Some((rank, set, chosen));
        }
    }
    best.map(|(_, s, c)| (s, c))
}

/// True if tags listed in corner order (TL, TR, BR, BL; gaps allowed) run clockwise on
/// screen. Two tags carry no orientation evidence and pass.
fn clockwise(tags: &[TagHit]) -> bool {
    if tags.len() < 3 {
        return true;
    }
    let pts: Vec<PointPx> = tags.iter().map(TagHit::centre).collect();
    let n = pts.len();
    let area: f64 = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    area > 0.0
}

/// Searches the four corner windows for tags (§8.3 steps 1–2), with the mirrored hypothesis
/// and a decimation retry chain for windows that found nothing.
pub(crate) fn search(det: &dyn TagDetector, img: &GrayImage, cfg: &TagConfig) -> TagSearch {
    let (w, h) = (img.width(), img.height());
    let ppm = estimate_ppm(w, h);
    let side = ((f64::from(cfg.window_um) / 1000.0) * ppm).round() as usize;
    let wins = windows(w, h, side.max(32));
    let ds = ((6.4 * ppm / f64::from(cfg.search_square_px.max(4))).round() as usize)
        .clamp(1, cfg.max_downscale.max(1) as usize);
    let mut retries = vec![ds];
    for alt in [ds.saturating_sub(1), ds + 1] {
        if alt >= 1 && alt <= cfg.max_downscale.max(1) as usize && !retries.contains(&alt) {
            retries.push(alt);
        }
    }

    let run = |image: &GrayImage| -> Vec<TagHit> {
        let mut hits = Vec::new();
        for win in &wins {
            let mut found = Vec::new();
            for &f in &retries {
                found = detect_in(det, image, win, f, cfg.max_hamming);
                if found.iter().any(|h| cfg.known_tag_sets.contains(&h.set())) {
                    break;
                }
            }
            hits.extend(found);
        }
        sort_hits(&mut hits);
        dedup(&mut hits);
        hits
    };

    let hits = run(img);
    if let Some((set, tags)) = choose(&hits, cfg)
        && (tags.len() >= 3 || !mirrored_better(img, cfg, &run, tags.len()))
    {
        return TagSearch {
            all_ids: hits.iter().map(|h| h.id).collect(),
            tags,
            set: Some(set),
            mirrored: false,
            downscale: ds as u32,
        };
    }
    // Mirrored hypothesis (§8.3 step 2): decode the mirrored image.
    let flipped = mirror(img);
    let mhits = run(&flipped);
    if let Some((set, tags)) = choose(&mhits, cfg) {
        let tags = tags
            .into_iter()
            .map(|mut t| {
                // Back to original coordinates: x → W − x (pixel i covers [i, i+1)).
                for c in &mut t.corners {
                    c.x = w as f64 - c.x;
                }
                // Mirroring reverses the winding; restore clockwise order on screen.
                t.corners.reverse();
                t
            })
            .collect();
        return TagSearch {
            all_ids: mhits.iter().map(|h| h.id).collect(),
            tags,
            set: Some(set),
            mirrored: true,
            downscale: ds as u32,
        };
    }
    TagSearch {
        all_ids: hits.iter().map(|h| h.id).collect(),
        tags: Vec::new(),
        set: None,
        mirrored: false,
        downscale: ds as u32,
    }
}

/// With only two tags the arrangement carries no winding; prefer the mirrored hypothesis
/// only if it yields more tags of one set.
fn mirrored_better(
    img: &GrayImage,
    cfg: &TagConfig,
    run: &dyn Fn(&GrayImage) -> Vec<TagHit>,
    have: usize,
) -> bool {
    let mhits = run(&mirror(img));
    choose(&mhits, cfg).is_some_and(|(_, t)| t.len() > have)
}

/// Removes duplicate hits of one ID within a few pixels (overlapping windows).
fn dedup(hits: &mut Vec<TagHit>) {
    let mut out: Vec<TagHit> = Vec::with_capacity(hits.len());
    for h in hits.iter() {
        let c = h.centre();
        if out.iter().any(|o| {
            let oc = o.centre();
            o.id == h.id && (oc.x - c.x).hypot(oc.y - c.y) < 8.0
        }) {
            continue;
        }
        out.push(*h);
    }
    *hits = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: u16, x: f64, y: f64) -> TagHit {
        let c = |dx: f64, dy: f64| PointPx {
            x: x + dx,
            y: y + dy,
        };
        TagHit {
            id,
            hamming: 0,
            corners: [c(-5.0, -5.0), c(5.0, -5.0), c(5.0, 5.0), c(-5.0, 5.0)],
        }
    }

    #[test]
    fn detector_survives_odd_images() {
        let d = KorniaDetector;
        for (w, h) in [(1, 1), (15, 15), (17, 40), (64, 64), (300, 37)] {
            let noise = GrayImage::from_fn(w, h, |x, y| ((x * 7919 + y * 104_729) % 251) as u8);
            for ds in 1..=4 {
                assert!(d.detect(&noise, ds).is_empty() || w >= 64);
            }
            let flat = GrayImage::new(w, h, 0);
            assert!(d.detect(&flat, 2).is_empty());
        }
        let cfg = TagConfig::default();
        let s = search(&d, &GrayImage::new(40, 40, 255), &cfg);
        assert!(s.set.is_none() && s.tags.is_empty());
    }

    #[test]
    fn set_choice_and_winding() {
        let cfg = TagConfig::default();
        let upright = [
            hit(0, 10.0, 10.0),
            hit(1, 100.0, 10.0),
            hit(2, 100.0, 140.0),
            hit(3, 10.0, 140.0),
        ];
        let (set, tags) = choose(&upright, &cfg).unwrap();
        assert_eq!((set, tags.len()), (0, 4));
        // Upside down: still clockwise.
        let rotated = [
            hit(2, 10.0, 10.0),
            hit(3, 100.0, 10.0),
            hit(0, 100.0, 140.0),
            hit(1, 10.0, 140.0),
        ];
        assert!(choose(&rotated, &cfg).is_some());
        // Mirrored arrangement: counter-clockwise, rejected.
        let mirrored = [
            hit(1, 10.0, 10.0),
            hit(0, 100.0, 10.0),
            hit(3, 100.0, 140.0),
            hit(2, 10.0, 140.0),
        ];
        assert!(choose(&mirrored, &cfg).is_none());
        // Proof tag set is recognised as set 3.
        let proof = [
            hit(12, 10.0, 10.0),
            hit(13, 100.0, 10.0),
            hit(14, 100.0, 140.0),
        ];
        assert_eq!(choose(&proof, &cfg).unwrap().0, 3);
        // A lone tag is not enough.
        assert!(choose(&upright[..1], &cfg).is_none());
    }
}
