//! Differential test on the owner's real IAU scanner PDFs (restricted data; never committed).
//!
//! Skips unless `OMR_IAU_PDF_DIR` (the scanner PDFs) and `OMR_IAU_REF_1BIT` (fingerprints from
//! `tests/support/ref_fingerprints.py`, an independent pypdf + Pillow/libtiff decode) are set.
//! Optional: `OMR_IAU_DIR` (folder with `pages.json` and the 200-dpi `pages/*.png` reference
//! reductions) for the ±1 gray-level check, and `OMR_IAU_OUT` (a scratch folder) for the timing
//! report. Only aggregate numbers are printed or written; no pixels, names or IDs.
//!
//! Checks per page: the 1-bit decode is pixel-exact (SHA-256 of the packed bitmap and the black
//! count), the `fax` crate's decoder agrees bit for bit, and the 600 → 200 dpi coverage
//! reduction is within ±1 of the reference PNG. Run with `--release` for meaningful timings.

use omr_pdf::{BitImage, ExtractOptions, PageRaster, PdfDocument, reduce_bilevel};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// Decodes a G4 stream with the `fax` crate into a 1 = black bitmap (cross-decoder check).
fn fax_decode(data: &[u8], w: u32, h: u32) -> Option<BitImage> {
    let mut img = BitImage::new(w, h).ok()?;
    let mut y = 0u32;
    fax::decoder::decode_g4(data.iter().copied(), w as u16 as u32, Some(h), |tr| {
        let mut i = 0;
        while i < tr.len() {
            let x0 = tr[i];
            let x1 = tr.get(i + 1).copied().unwrap_or(w).min(w);
            for x in x0..x1 {
                img.set(x, y, true);
            }
            i += 2;
        }
        y += 1;
    })?;
    (y == h).then_some(img)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// The raw CCITT bytes and size of a page's image, read with lopdf (independent of hayro).
fn raw_ccitt(doc: &lopdf::Document, page_no: u32) -> Option<(Vec<u8>, u32, u32)> {
    let pages = doc.get_pages();
    let pid = pages.get(&(page_no + 1))?;
    let (res, _) = doc.get_page_resources(*pid).ok()?;
    let res = res?;
    let xo = res.get(b"XObject").ok()?;
    let xo = match xo {
        lopdf::Object::Reference(r) => doc.get_dictionary(*r).ok()?,
        lopdf::Object::Dictionary(d) => d,
        _ => return None,
    };
    let (_, v) = xo.iter().next()?;
    let s = doc
        .get_object(v.as_reference().ok()?)
        .ok()?
        .as_stream()
        .ok()?;
    let w = s.dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = s.dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    Some((s.content.clone(), w, h))
}

#[test]
fn real_iau_pdfs_decode_pixel_exact_and_reduce_within_one_level() {
    let (Some(pdf_dir), Some(ref_path)) =
        (env_path("OMR_IAU_PDF_DIR"), env_path("OMR_IAU_REF_1BIT"))
    else {
        eprintln!("skipped: set OMR_IAU_PDF_DIR and OMR_IAU_REF_1BIT to run the real-data test");
        return;
    };
    let refs: Vec<Value> = serde_json::from_slice(&std::fs::read(&ref_path).unwrap()).unwrap();
    let mut by_key: BTreeMap<(String, u64), &Value> = BTreeMap::new();
    for r in &refs {
        by_key.insert(
            (
                r["file"].as_str().unwrap().to_owned(),
                r["page"].as_u64().unwrap(),
            ),
            r,
        );
    }
    // Optional PNG references: file stem (without the upload hash prefix) → page → png path.
    let png_index: BTreeMap<(String, u64), PathBuf> = env_path("OMR_IAU_DIR")
        .and_then(|d| {
            let j: Vec<Value> =
                serde_json::from_slice(&std::fs::read(d.join("pages.json")).ok()?).ok()?;
            Some(
                j.iter()
                    .map(|e| {
                        (
                            (
                                e["file"].as_str().unwrap().to_owned(),
                                e["page"].as_u64().unwrap() - 1,
                            ),
                            d.join(e["png200"].as_str().unwrap()),
                        )
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();

    let mut files: Vec<PathBuf> = std::fs::read_dir(&pdf_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "pdf"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no PDFs in OMR_IAU_PDF_DIR");

    let (mut pages, mut exact, mut fax_equal, mut png_checked, mut png_max_diff) =
        (0, 0, 0, 0, 0u8);
    let mut reduce_hash_equal = 0;
    let (mut png_edge_diff_px, mut png_interior_diff_px) = (0u64, 0u64);
    let (mut t_open, mut t_dec, mut t_red, mut t_both, mut t_fax) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut mpix = Vec::new();
    for f in &files {
        let fname = f.file_name().unwrap().to_string_lossy().into_owned();
        let stem = Path::new(&fname)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .split_once('-')
            .map_or(fname.clone(), |(_, s)| s.to_owned());
        let bytes = std::fs::read(f).unwrap();
        let lo = lopdf::Document::load_mem(&bytes).unwrap();
        let t0 = Instant::now();
        let doc = PdfDocument::open(bytes, ExtractOptions::default()).unwrap();
        t_open.push(t0.elapsed().as_secs_f64() * 1e3);
        for i in 0..doc.page_count() {
            let t0 = Instant::now();
            let out = doc.extract_page(i);
            let dec_ms = t0.elapsed().as_secs_f64() * 1e3;
            let p = out
                .extracted()
                .unwrap_or_else(|| panic!("{fname} p{i}: {:?}", out.refused()));
            let PageRaster::Bilevel(img) = &p.raster else {
                panic!("expected a bilevel page")
            };
            assert_eq!(
                p.nominal_dpi(),
                Some(600),
                "{fname} p{i} dpi {:?}",
                p.placement
            );
            let t1 = Instant::now();
            let g = reduce_bilevel(img, 3).unwrap();
            let red_ms = t1.elapsed().as_secs_f64() * 1e3;
            t_dec.push(dec_ms);
            t_red.push(red_ms);
            t_both.push(dec_ms + red_ms);
            mpix.push(f64::from(img.width()) * f64::from(img.height()) / 1e6);
            pages += 1;

            let r = by_key
                .get(&(fname.clone(), i as u64))
                .unwrap_or_else(|| panic!("no reference for {fname} p{i}"));
            assert_eq!(u64::from(img.width()), r["w"].as_u64().unwrap());
            assert_eq!(u64::from(img.height()), r["h"].as_u64().unwrap());
            let same = sha256_hex(&img.to_packed_rows()) == r["sha256_1bit"].as_str().unwrap()
                && img.black_count() == r["black"].as_u64().unwrap();
            assert!(
                same,
                "{fname} p{i}: 1-bit decode differs from the reference"
            );
            exact += 1;
            assert_eq!(u64::from(g.width()), r["w200"].as_u64().unwrap());
            assert_eq!(u64::from(g.height()), r["h200"].as_u64().unwrap());
            if sha256_hex(g.as_raw()) == r["sha256_200"].as_str().unwrap() {
                reduce_hash_equal += 1;
            }
            if let Some(png) = png_index.get(&(stem.clone(), i as u64)) {
                let refimg = image::open(png).unwrap().into_luma8();
                assert_eq!((refimg.width(), refimg.height()), (g.width(), g.height()));
                let (gw, gh) = (g.width() as usize, g.height() as usize);
                let mut d = 0u8;
                for (k, (a, b)) in refimg.as_raw().iter().zip(g.as_raw()).enumerate() {
                    let diff = a.abs_diff(*b);
                    if diff > 0 {
                        d = d.max(diff);
                        // Interior pixels average a full 3×3 block; only partial edge blocks
                        // may differ (Pillow's fixed-point corner rounding).
                        let (x, y) = (k % gw, k / gw);
                        let partial_x = x + 1 == gw && img.width() % 3 != 0;
                        let partial_y = y + 1 == gh && img.height() % 3 != 0;
                        if partial_x || partial_y {
                            png_edge_diff_px += 1;
                        } else {
                            png_interior_diff_px += 1;
                        }
                    }
                }
                png_max_diff = png_max_diff.max(d);
                png_checked += 1;
                assert!(d <= 1, "{fname} p{i}: 200-dpi reduction differs by {d}");
            }

            // Cross-decoder check and timing with the `fax` crate.
            let (raw, w, h) = raw_ccitt(&lo, i as u32).expect("raw CCITT bytes");
            let t2 = Instant::now();
            let fx = fax_decode(&raw, w, h);
            t_fax.push(t2.elapsed().as_secs_f64() * 1e3);
            if fx.as_ref() == Some(img) {
                fax_equal += 1;
            }
        }
    }
    let report = serde_json::json!({
        "files": files.len(),
        "pages": pages,
        "pixel_exact_1bit": exact,
        "fax_crate_bit_identical": fax_equal,
        "reduction_sha256_equal_to_pil_reduce3": reduce_hash_equal,
        "png_pages_checked": png_checked,
        "png_max_abs_diff": png_max_diff,
        "png_diff_pixels_partial_edge_blocks": png_edge_diff_px,
        "png_diff_pixels_interior": png_interior_diff_px,
        "megapixels_median": median(&mut mpix.clone()),
        "open_ms_median_per_file": median(&mut t_open),
        "decode_ms": { "median": median(&mut t_dec.clone()), "p95": pct(&mut t_dec.clone(), 0.95), "max": pct(&mut t_dec, 1.0) },
        "reduce_ms": { "median": median(&mut t_red.clone()), "p95": pct(&mut t_red.clone(), 0.95), "max": pct(&mut t_red, 1.0) },
        "decode_plus_reduce_ms": { "median": median(&mut t_both.clone()), "p95": pct(&mut t_both.clone(), 0.95), "max": pct(&mut t_both, 1.0) },
        "fax_decode_ms": { "median": median(&mut t_fax.clone()), "p95": pct(&mut t_fax, 0.95) },
        "optimized_build": !cfg!(debug_assertions),
    });
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    if let Some(out) = env_path("OMR_IAU_OUT") {
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(
            out.join("omr_pdf_real_iau.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(exact, pages);
    assert_eq!(
        png_interior_diff_px, 0,
        "interior 3×3 blocks must match exactly"
    );
    assert_eq!(
        fax_equal, pages,
        "fax and hayro-ccitt disagree on some page"
    );
}
