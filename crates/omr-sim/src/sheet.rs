//! The simulator: clean raster (cached per form and page) → per-sheet page codes → marks →
//! capture, with ground truth carried through every step.

use crate::SimError;
use crate::capture::{Captured, capture};
use crate::paint::Canvas;
use crate::profile::{CaptureParams, Profile};
use crate::script::{BubbleMark, Instrument, SheetScript};
use crate::truth::{MarkTruth, PageTruth, QuestionTruth, StrayTruth};
use image::GrayImage;
use omr_forms::render::{composite_variable, rasterize_static};
use omr_forms::{ExamKey, PageGeometry, SheetInstance, Template};
use omr_model::{BookletSerial, FormCode, MarkClass, PageNo};
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// One simulated page image with its truth.
#[derive(Clone, Debug)]
pub struct SimPage {
    /// The captured image.
    pub image: GrayImage,
    /// Ground truth.
    pub truth: PageTruth,
}

/// What [`Simulator::paint_page`] painted on one page.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintedPage {
    /// Truth of the questions on the page.
    pub questions: Vec<QuestionTruth>,
    /// ID digits bubbled on the page (page 1 only).
    pub id_digits: Vec<u8>,
    /// Stray marks.
    pub strays: Vec<StrayTruth>,
}

/// Renders, marks and captures sheets of one template. Shareable across threads; the static
/// raster of each (form, page) is rendered once and reused.
pub struct Simulator<'a> {
    template: &'a Template,
    key: &'a ExamKey,
    dpi: u32,
    cache: Mutex<BTreeMap<(u16, u16), Arc<GrayImage>>>,
}

impl<'a> Simulator<'a> {
    /// A simulator rendering at `dpi` (also the capture resolution).
    pub fn new(template: &'a Template, key: &'a ExamKey, dpi: u32) -> Self {
        Self {
            template,
            key,
            dpi,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    /// The template.
    pub fn template(&self) -> &'a Template {
        self.template
    }

    /// The resolution.
    pub fn dpi(&self) -> u32 {
        self.dpi
    }

    fn static_raster(&self, form: FormCode, page: PageNo) -> Result<Arc<GrayImage>, SimError> {
        let k = (form.0, page.0);
        if let Some(img) = self.cache.lock().map_err(|_| SimError::Poisoned)?.get(&k) {
            return Ok(img.clone());
        }
        let inst = SheetInstance::new(self.template, self.key, form, page);
        let img = Arc::new(rasterize_static(&inst, self.dpi as f32)?);
        let mut c = self.cache.lock().map_err(|_| SimError::Poisoned)?;
        Ok(c.entry(k).or_insert(img).clone())
    }

    /// The unmarked printed page for a booklet (static raster plus page codes and echo lines).
    pub fn clean_page(
        &self,
        form: FormCode,
        page: PageNo,
        serial: BookletSerial,
    ) -> Result<GrayImage, SimError> {
        let mut img = (*self.static_raster(form, page)?).clone();
        let inst = SheetInstance::new(self.template, self.key, form, page).with_serial(serial);
        composite_variable(&mut img, &inst, self.dpi as f32)?;
        Ok(img)
    }

    /// Paints the script's marks for `page` into `img`.
    pub fn paint_page(
        &self,
        img: &mut GrayImage,
        script: &SheetScript,
        page: PageNo,
        rng: &mut ChaCha8Rng,
    ) -> Result<PaintedPage, SimError> {
        let g = self
            .template
            .page(page)
            .ok_or(SimError::NoSuchPage(page.0))?;
        let mut canvas = Canvas::new(img, f64::from(self.dpi));
        // Student ID (page 1).
        let mut id_digits = Vec::new();
        if let Some(id) = &script.student_id
            && !g.id_bubbles.is_empty()
        {
            for (col, ch) in id.chars().enumerate() {
                let d = ch
                    .to_digit(10)
                    .ok_or_else(|| SimError::Roster(id.clone()))? as u8;
                id_digits.push(d);
                let Some(b) = g
                    .id_bubbles
                    .iter()
                    .find(|b| usize::from(b.column) == col && b.digit == d)
                else {
                    continue;
                };
                let m = id_mark(script, rng);
                let c = b.centre_mm();
                canvas.paint_mark(c.x, c.y, b.radius_mm(), &m, rng);
            }
        }
        // Answers.
        let mut questions = Vec::new();
        for q in g.questions() {
            let Some(qs) = script.questions.get(usize::from(q)) else {
                continue;
            };
            let bubbles = self.template.question_bubbles(q);
            let mut marks = Vec::new();
            for m in &qs.marks {
                let Some(b) = bubbles.get(usize::from(m.option)) else {
                    continue;
                };
                let c = b.centre_mm();
                let achieved = canvas.paint_mark(c.x, c.y, b.radius_mm(), m, rng);
                marks.push(MarkTruth {
                    option: m.option,
                    class: m.class,
                    darkness: m.darkness,
                    coverage_target: m.coverage,
                    achieved,
                    instrument: m.instrument,
                });
            }
            questions.push(QuestionTruth {
                question: q,
                intended: qs.intended.clone(),
                expected: qs.expected.clone(),
                classes: qs.classes.clone(),
                difficulty: qs.difficulty,
                marks,
            });
        }
        // Stray marks, spread over the pages.
        let pages = u32::from(self.template.page_count());
        let mine = script.strays / pages + u32::from(u32::from(page.0) <= script.strays % pages);
        let mut strays = Vec::new();
        for _ in 0..mine {
            if let Some(s) = place_stray(g, rng) {
                let seed = rng.random::<u64>();
                canvas.paint_stray(s.points_mm.clone(), s.width_mm, s.darkness, seed);
                strays.push(s);
            }
        }
        Ok(PaintedPage {
            questions,
            id_digits,
            strays,
        })
    }

    /// Simulates every page of one booklet.
    pub fn simulate(
        &self,
        script: &SheetScript,
        profile: Profile,
        rng: &mut ChaCha8Rng,
    ) -> Result<Vec<SimPage>, SimError> {
        let form = FormCode(script.form);
        let serial = BookletSerial(script.serial);
        let mut out = Vec::new();
        for p in 1..=self.template.page_count() {
            let page = PageNo(p);
            let mut img = self.clean_page(form, page, serial)?;
            let PaintedPage {
                questions,
                id_digits,
                strays,
            } = self.paint_page(&mut img, script, page, rng)?;
            let params = CaptureParams::sample(profile, self.dpi, rng);
            let Captured { image, mm_to_px } = capture(&img, &params, rng);
            let payload = SheetInstance::new(self.template, self.key, form, page)
                .with_serial(serial)
                .payload()?;
            out.push(SimPage {
                image,
                truth: PageTruth {
                    sheet: 0,
                    file: String::new(),
                    page: p,
                    pages: self.template.page_count(),
                    form: script.form,
                    booklet: script.serial,
                    payload,
                    student_id: script.student_id.clone(),
                    id_digits,
                    instrument: script.instrument,
                    profile,
                    capture: params,
                    mm_to_px,
                    questions,
                    strays,
                },
            });
        }
        Ok(out)
    }
}

fn id_mark(script: &SheetScript, rng: &mut ChaCha8Rng) -> BubbleMark {
    let (darkness, coverage) = match script.instrument {
        Instrument::Pen => (
            rng.random_range(0.85f32..=0.98),
            rng.random_range(0.9f32..=1.0),
        ),
        Instrument::Pencil => (
            (script.base_darkness + rng.random_range(-0.06f32..=0.06)).clamp(0.55, 0.95),
            rng.random_range(0.75f32..=1.0),
        ),
    };
    BubbleMark {
        option: 0,
        class: MarkClass::Filled,
        darkness,
        coverage,
        instrument: script.instrument,
    }
}

/// A stray stroke inside the safe box that keeps clear of every bubble, tag and code.
fn place_stray(g: &PageGeometry, rng: &mut ChaCha8Rng) -> Option<StrayTruth> {
    let sb = g.tags.first().map(|_| omr_forms::geometry::SAFE_BOX_A4)?;
    let centres: Vec<(f64, f64, f64)> = g
        .bubbles
        .iter()
        .map(|b| (b.centre_mm(), b.radius_mm()))
        .chain(g.id_bubbles.iter().map(|b| (b.centre_mm(), b.radius_mm())))
        .chain(
            g.form_bubbles
                .iter()
                .map(|b| (b.centre_mm(), b.radius_mm())),
        )
        .map(|(c, r)| (c.x, c.y, r))
        .collect();
    let keep_out: Vec<omr_model::RectMm> = g
        .tags
        .iter()
        .map(|t| t.outer.inflate(2_000).to_mm())
        .chain(
            g.codes
                .iter()
                .flat_map(|c| [c.rect.inflate(2_000).to_mm(), c.echo.to_mm()]),
        )
        .collect();
    let (x0, y0, x1, y1) = (
        f64::from(sb.x) / 1000.0,
        f64::from(sb.y) / 1000.0,
        f64::from(sb.right()) / 1000.0,
        f64::from(sb.bottom()) / 1000.0,
    );
    for _ in 0..40 {
        let segs = rng.random_range(1..=3);
        let mut pts = vec![(rng.random_range(x0..x1), rng.random_range(y0..y1))];
        for _ in 0..segs {
            let (px, py) = pts[pts.len() - 1];
            let len = rng.random_range(2.0..6.0);
            let a: f64 = rng.random_range(0.0..std::f64::consts::TAU);
            pts.push((px + len * a.cos(), py + len * a.sin()));
        }
        let width = rng.random_range(0.3..0.6);
        let ok = pts.windows(2).all(|w| {
            (0..=10).all(|i| {
                let t = f64::from(i) / 10.0;
                let x = w[0].0 + t * (w[1].0 - w[0].0);
                let y = w[0].1 + t * (w[1].1 - w[0].1);
                let inside = x > x0 && x < x1 && y > y0 && y < y1;
                let clear = centres
                    .iter()
                    .all(|(cx, cy, r)| (x - cx).hypot(y - cy) > 1.5 * r + width);
                let free = keep_out
                    .iter()
                    .all(|k| !(x >= k.x && x <= k.x + k.w && y >= k.y && y <= k.y + k.h));
                inside && clear && free
            })
        });
        if ok {
            return Some(StrayTruth {
                points_mm: pts,
                width_mm: width,
                darkness: rng.random_range(0.4f32..0.85),
            });
        }
    }
    None
}
