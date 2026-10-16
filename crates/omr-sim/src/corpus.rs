//! Corpus writer: N sheets → `images/*.jpg|png`, `truth.jsonl` (one line per page image, in
//! sheet then page order) and `manifest.json`. Deterministic by seed and independent of the
//! thread count: each sheet draws from its own ChaCha stream.

use crate::SimError;
use crate::capture::{encode_jpeg, encode_png};
use crate::profile::Profile;
use crate::script::{MarkRates, SheetScript};
use crate::sheet::Simulator;
use omr_forms::{ExamKey, Template};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::Path;

/// Generator identifier written into every manifest.
pub const GENERATOR: &str = concat!("omr-sim/", env!("CARGO_PKG_VERSION"));

/// Output image format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    /// Baseline JPEG at the page's sampled quality.
    Jpeg,
    /// Lossless PNG.
    Png,
}

impl ImageFormat {
    fn ext(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
        }
    }
}

/// What to generate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorpusConfig {
    /// Number of booklets.
    pub sheets: u32,
    /// Capture profile.
    pub profile: Profile,
    /// Master seed.
    pub seed: u64,
    /// Render and capture resolution.
    pub dpi: u32,
    /// Image format.
    pub format: ImageFormat,
    /// Student IDs to bubble (each must have the template's ID-digit count); drawn without
    /// replacement in a seeded order, then reused. Empty: random IDs.
    pub roster: Vec<String>,
    /// Mark rates.
    pub rates: MarkRates,
    /// First booklet serial.
    pub serial_start: u32,
    /// Worker threads (≥ 1).
    pub threads: usize,
}

impl CorpusConfig {
    /// The standard-profile defaults: 200 dpi JPEG, standard marks.
    pub fn standard(sheets: u32, seed: u64) -> Self {
        Self {
            sheets,
            profile: Profile::Standard,
            seed,
            dpi: 200,
            format: ImageFormat::Jpeg,
            roster: Vec::new(),
            rates: MarkRates::standard(),
            serial_start: 1,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        }
    }
}

/// The corpus manifest (`manifest.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorpusManifest {
    /// Generator and version.
    pub generator: String,
    /// Profile.
    pub profile: Profile,
    /// Master seed.
    pub seed: u64,
    /// Resolution.
    pub dpi: u32,
    /// Image format.
    pub format: ImageFormat,
    /// SHA-384 of the template's canonical layout manifest.
    pub template_sha384: String,
    /// 40-bit exam ID printed in the page codes.
    pub exam_id40: u64,
    /// Booklets.
    pub sheets: u32,
    /// Page images.
    pub pages: u64,
    /// Questions over all booklets.
    pub questions: u64,
    /// Mark rates.
    pub rates: MarkRates,
    /// Roster size (0 = random IDs).
    pub roster_size: usize,
    /// First serial.
    pub serial_start: u32,
    /// Truth file name.
    pub truth: String,
    /// Image directory.
    pub images: String,
}

/// What was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorpusSummary {
    /// Booklets.
    pub sheets: u32,
    /// Page images.
    pub pages: u64,
    /// Total bytes of image data.
    pub image_bytes: u64,
}

/// The seeded order in which roster entries are used.
fn roster_order(n: usize, seed: u64) -> Vec<usize> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    rng.set_stream(u64::MAX);
    let mut v: Vec<usize> = (0..n).collect();
    // Fisher–Yates with the generator's bounded sampling (test data only; not §6.6 shuffling).
    for i in (1..n).rev() {
        let j = rng.random_range(0..=i);
        v.swap(i, j);
    }
    v
}

/// The script of sheet `index` (deterministic by seed and index).
pub fn sheet_script(
    template: &Template,
    cfg: &CorpusConfig,
    order: &[usize],
    index: u32,
) -> Result<(SheetScript, ChaCha8Rng), SimError> {
    let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed);
    rng.set_stream(u64::from(index) + 1);
    let digits = usize::from(template.spec().id_digits);
    let student_id = if digits == 0 {
        None
    } else if cfg.roster.is_empty() {
        Some(
            (0..digits)
                .map(|_| char::from(b'0' + rng.random_range(0..10u8)))
                .collect(),
        )
    } else {
        let id = order
            .get(index as usize % order.len().max(1))
            .and_then(|&i| cfg.roster.get(i))
            .cloned()
            .ok_or_else(|| SimError::Roster(format!("no roster entry for sheet {index}")))?;
        if id.chars().count() != digits || !id.chars().all(|c| c.is_ascii_digit()) {
            return Err(SimError::Roster(id));
        }
        Some(id)
    };
    let form = rng.random_range(1..=template.spec().forms);
    let serial = cfg.serial_start.saturating_add(index);
    let script = SheetScript::random(template, form, serial, student_id, &cfg.rates, &mut rng);
    Ok((script, rng))
}

struct Done {
    index: u32,
    lines: Vec<String>,
    bytes: u64,
    pages: u64,
}

fn run_sheet(
    sim: &Simulator<'_>,
    cfg: &CorpusConfig,
    order: &[usize],
    index: u32,
    images: &Path,
) -> Result<Done, SimError> {
    let (script, mut rng) = sheet_script(sim.template(), cfg, order, index)?;
    let pages = sim.simulate(&script, cfg.profile, &mut rng)?;
    let mut lines = Vec::new();
    let mut bytes = 0u64;
    let n = pages.len() as u64;
    for mut p in pages {
        let name = format!("sheet_{index:06}_p{}.{}", p.truth.page, cfg.format.ext());
        let data = match cfg.format {
            ImageFormat::Jpeg => encode_jpeg(&p.image, p.truth.capture.jpeg_quality, cfg.dpi),
            ImageFormat::Png => encode_png(&p.image),
        }
        .map_err(|e| SimError::Encode(e.to_string()))?;
        std::fs::write(images.join(&name), &data)?;
        bytes += data.len() as u64;
        p.truth.sheet = u64::from(index);
        p.truth.file = format!("images/{name}");
        lines.push(serde_json::to_string(&p.truth).map_err(|e| SimError::Encode(e.to_string()))?);
    }
    Ok(Done {
        index,
        lines,
        bytes,
        pages: n,
    })
}

/// Generates a corpus into `out_dir` (created if missing).
pub fn write_corpus(
    template: &Template,
    key: &ExamKey,
    cfg: &CorpusConfig,
    out_dir: &Path,
) -> Result<CorpusSummary, SimError> {
    let images = out_dir.join("images");
    std::fs::create_dir_all(&images)?;
    let sim = Simulator::new(template, key, cfg.dpi);
    let order = roster_order(cfg.roster.len(), cfg.seed);
    let threads = cfg.threads.max(1).min(cfg.sheets.max(1) as usize);
    let mut done: Vec<Result<Done, SimError>> = Vec::with_capacity(cfg.sheets as usize);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let (sim, order, images) = (&sim, &order, &images);
                s.spawn(move || {
                    (t as u32..cfg.sheets)
                        .step_by(threads)
                        .map(|i| run_sheet(sim, cfg, order, i, images))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            match h.join() {
                Ok(v) => done.extend(v),
                Err(_) => done.push(Err(SimError::Poisoned)),
            }
        }
    });
    let mut ok: Vec<Done> = done.into_iter().collect::<Result<_, _>>()?;
    ok.sort_by_key(|d| d.index);
    let mut truth = std::io::BufWriter::new(std::fs::File::create(out_dir.join("truth.jsonl"))?);
    let (mut pages, mut bytes) = (0u64, 0u64);
    for d in &ok {
        for l in &d.lines {
            truth.write_all(l.as_bytes())?;
            truth.write_all(b"\n")?;
        }
        pages += d.pages;
        bytes += d.bytes;
    }
    truth.flush()?;
    let manifest = CorpusManifest {
        generator: GENERATOR.to_owned(),
        profile: cfg.profile,
        seed: cfg.seed,
        dpi: cfg.dpi,
        format: cfg.format,
        template_sha384: template.hash_hex(),
        exam_id40: template.exam_id40(),
        sheets: cfg.sheets,
        pages,
        questions: u64::from(cfg.sheets) * u64::from(template.question_count()),
        rates: cfg.rates,
        roster_size: cfg.roster.len(),
        serial_start: cfg.serial_start,
        truth: "truth.jsonl".into(),
        images: "images/".into(),
    };
    let json =
        serde_json::to_string_pretty(&manifest).map_err(|e| SimError::Encode(e.to_string()))?;
    std::fs::write(out_dir.join("manifest.json"), json)?;
    Ok(CorpusSummary {
        sheets: cfg.sheets,
        pages,
        image_bytes: bytes,
    })
}

/// Reads `truth.jsonl` back.
pub fn read_truth(path: &Path) -> Result<Vec<crate::truth::PageTruth>, SimError> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| SimError::Encode(e.to_string())))
        .collect()
}
