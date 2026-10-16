//! What the simulated student does on a sheet: per-question marks with ground truth known by
//! construction (spec §23.3 stages 3–5), and the expected engine outcome under the exam's
//! `cancel_mode` (§8.8 table).

use omr_forms::Template;
use omr_model::{CancelMode, MarkClass, Response};
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// Writing instrument of a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Instrument {
    /// Graphite pencil (grainy, darkness 0.55–0.95).
    Pencil,
    /// Ball-point or gel pen (uniform dark).
    Pen,
}

/// How often each kind of mark occurs (per question unless stated).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkRates {
    /// Question left blank.
    pub blank: f64,
    /// Two options filled (intended `Multi`).
    pub multi: f64,
    /// Answer changed by erasure: faint residue on the old option, fill on the new one.
    pub erasure: f64,
    /// The answer is a light fill (darkness 0.25–0.5).
    pub light: f64,
    /// A stray dot or partial fill on another option beside a clear answer.
    pub dot: f64,
    /// The answer is a tick or X instead of a fill.
    pub check_or_x: f64,
    /// An option crossed out and another filled.
    pub crossed_out: f64,
    /// Probability that a sheet is written in pen.
    pub pen_sheet: f64,
    /// Mean number of stray marks outside bubbles per sheet (Poisson).
    pub stray_per_sheet: f64,
}

impl MarkRates {
    /// No hard cases at all.
    pub fn clean() -> Self {
        Self {
            blank: 0.02,
            multi: 0.0,
            erasure: 0.0,
            light: 0.0,
            dot: 0.0,
            check_or_x: 0.0,
            crossed_out: 0.0,
            pen_sheet: 0.1,
            stray_per_sheet: 0.0,
        }
    }

    /// The common case: some blanks, light fills and erasures, rare multis and dots.
    pub fn standard() -> Self {
        Self {
            blank: 0.03,
            multi: 0.01,
            erasure: 0.04,
            light: 0.03,
            dot: 0.01,
            check_or_x: 0.0,
            crossed_out: 0.0,
            pen_sheet: 0.15,
            stray_per_sheet: 0.5,
        }
    }

    /// Many light marks and erasures, plus ticks, crosses and stray marks.
    pub fn hard() -> Self {
        Self {
            blank: 0.04,
            multi: 0.02,
            erasure: 0.10,
            light: 0.10,
            dot: 0.03,
            check_or_x: 0.02,
            crossed_out: 0.02,
            pen_sheet: 0.2,
            stray_per_sheet: 2.0,
        }
    }
}

/// One deliberate mark on one bubble.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BubbleMark {
    /// 0-based option.
    pub option: u8,
    /// Appearance class.
    pub class: MarkClass,
    /// Target darkness of the mark (fraction of full black).
    pub darkness: f32,
    /// Target coverage of the bubble interior.
    pub coverage: f32,
    /// Instrument.
    pub instrument: Instrument,
}

/// Difficulty tag of a question (the hardest feature present).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Difficulty {
    /// One clear, dark fill.
    Easy,
    /// No marks.
    Blank,
    /// A light fill.
    Light,
    /// An erasure residue beside the final answer.
    Erasure,
    /// Several filled options.
    Multi,
    /// A dot, tick, X or cross-out (may need review).
    Ambiguous,
}

/// The engine outcome a perfect, calibrated recognizer should produce (§8.8).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Expected {
    /// Auto-acceptable with this response.
    Auto(Response),
    /// Must go to review (an R class in the §8.8 table).
    Review,
}

/// Ground truth for one question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionScript {
    /// 0-based question.
    pub question: u16,
    /// What the student meant (the score-relevant truth).
    pub intended: Response,
    /// What a perfect recognizer outputs under the exam's `cancel_mode`.
    pub expected: Expected,
    /// Class of every bubble (index = option); `Empty` where unmarked.
    pub classes: Vec<MarkClass>,
    /// The marks, in painting order.
    pub marks: Vec<BubbleMark>,
    /// Difficulty tag.
    pub difficulty: Difficulty,
}

/// Selection state S(c) of a class (§8.8 table): `Some(true)` = Y, `Some(false)` = N,
/// `None` = R.
///
/// `other` is the context the class depends on: for `partial_or_dot` and `stray_or_damage`,
/// whether another bubble of the question is Y; for `whiteout` ("N; R if it is the only mark"),
/// whether another bubble of the question carries any mark. Other classes ignore it.
/// [`expected_outcome`] passes the right one per class.
pub fn selection_state(class: MarkClass, mode: CancelMode, other: bool) -> Option<bool> {
    use CancelMode::*;
    use MarkClass::*;
    match (class, mode) {
        (Empty | ErasedResidue, _) => Some(false),
        (Filled, EraseOnly | CrossCancels) => Some(true),
        (Filled, FillCancelsCheck) => Some(false),
        (LightFill, EraseOnly | CrossCancels) => Some(true),
        (LightFill, FillCancelsCheck) => None,
        (CrossedOut, CrossCancels) => Some(false),
        (CrossedOut, _) => None,
        (CheckOrX, FillCancelsCheck) => Some(true),
        (CheckOrX, _) => None,
        (CircledLabel, _) => None,
        (PartialOrDot | StrayOrDamage, _) => other.then_some(false),
        (Whiteout, _) => other.then_some(false),
    }
}

/// The expected outcome of a question from its bubble classes.
pub fn expected_outcome(classes: &[MarkClass], mode: CancelMode) -> Expected {
    // First pass: definite Y bubbles (classes that do not depend on the others).
    let firm_y: Vec<bool> = classes
        .iter()
        .map(|c| selection_state(*c, mode, false) == Some(true))
        .collect();
    let mut selected = Vec::new();
    for (i, c) in classes.iter().enumerate() {
        let other = if *c == MarkClass::Whiteout {
            classes
                .iter()
                .enumerate()
                .any(|(j, k)| j != i && *k != MarkClass::Empty)
        } else {
            firm_y.iter().enumerate().any(|(j, y)| j != i && *y)
        };
        match selection_state(*c, mode, other) {
            Some(true) => selected.push(i as u8),
            Some(false) => {}
            None => return Expected::Review,
        }
    }
    Expected::Auto(match selected.len() {
        0 => Response::Blank,
        1 => Response::Single(selected[0]),
        _ => Response::Multi(selected),
    })
}

fn pick_other(rng: &mut ChaCha8Rng, n: u8, not: &[u8]) -> Option<u8> {
    let free: Vec<u8> = (0..n).filter(|o| !not.contains(o)).collect();
    if free.is_empty() {
        None
    } else {
        Some(free[rng.random_range(0..free.len())])
    }
}

fn fill(rng: &mut ChaCha8Rng, option: u8, inst: Instrument, base: f32, light: bool) -> BubbleMark {
    let (class, darkness, coverage) = if light {
        (
            MarkClass::LightFill,
            rng.random_range(0.25f32..=0.5),
            rng.random_range(0.7f32..=1.0),
        )
    } else {
        match inst {
            Instrument::Pen => (
                MarkClass::Filled,
                rng.random_range(0.85f32..=0.98),
                rng.random_range(0.9f32..=1.0),
            ),
            Instrument::Pencil => (
                MarkClass::Filled,
                (base + rng.random_range(-0.06f32..=0.06)).clamp(0.55, 0.95),
                rng.random_range(0.7f32..=1.0),
            ),
        }
    };
    BubbleMark {
        option,
        class,
        darkness,
        coverage,
        instrument: if light { Instrument::Pencil } else { inst },
    }
}

/// How one student marks: the exam's cancel mode, the instrument and the typical darkness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// The exam's marking convention.
    pub mode: CancelMode,
    /// Instrument.
    pub instrument: Instrument,
    /// Typical pencil darkness of this student.
    pub base_darkness: f32,
}

/// Brings an intended response into the space a student can mark on a single-answer item with
/// `options` bubbles: options that do not exist and duplicates are dropped, `Set` is marked like
/// `Multi`, and `Unreadable` (not something a student can intend) becomes `Blank`.
pub fn normalise_intent(intended: Response, options: u8) -> Response {
    let chosen = |v: &[u8]| {
        let mut v: Vec<u8> = v.iter().copied().filter(|o| *o < options).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let v = match intended {
        Response::Single(o) => chosen(&[o]),
        Response::Multi(v) | Response::Set(v) => chosen(&v),
        Response::Blank | Response::Unreadable { .. } => Vec::new(),
    };
    match v.len() {
        0 => Response::Blank,
        1 => Response::Single(v[0]),
        _ => Response::Multi(v),
    }
}

/// Builds the marks of one question for an intended response (normalised first, see
/// [`normalise_intent`]). Rates add erasures, light fills, dots, ticks and cancellations around
/// the intent. The student follows the exam's marking convention (§6.5): under
/// `fill_cancels_check` a selection is a tick and a cancellation is a full fill; otherwise a
/// selection is a fill and a cancellation is a cross-out.
pub fn script_question(
    rng: &mut ChaCha8Rng,
    question: u16,
    options: u8,
    intended: Response,
    style: Style,
    rates: &MarkRates,
) -> QuestionScript {
    let (mode, inst, base) = (style.mode, style.instrument, style.base_darkness);
    let fcc = mode == CancelMode::FillCancelsCheck;
    let intended = normalise_intent(intended, options);
    let mut marks = Vec::new();
    let chosen: Vec<u8> = match &intended {
        Response::Single(o) => vec![*o],
        Response::Multi(v) | Response::Set(v) => v.clone(),
        Response::Blank | Response::Unreadable { .. } => Vec::new(),
    };
    // Erasure first (residue of an abandoned answer), then cross-outs, then the answer.
    if !chosen.is_empty()
        && rng.random_bool(rates.erasure)
        && let Some(o) = pick_other(rng, options, &chosen)
    {
        marks.push(BubbleMark {
            option: o,
            class: MarkClass::ErasedResidue,
            darkness: rng.random_range(0.05f32..=0.2),
            coverage: rng.random_range(0.5f32..=1.0),
            instrument: Instrument::Pencil,
        });
    }
    if !chosen.is_empty()
        && rng.random_bool(rates.crossed_out)
        && let Some(o) = pick_other(rng, options, &chosen)
        && !marks.iter().any(|m| m.option == o)
    {
        if fcc {
            // Under fill_cancels_check a complete fill is the cancellation (§6.5, §8.8).
            marks.push(fill(rng, o, inst, base, false));
        } else {
            marks.push(BubbleMark {
                option: o,
                class: MarkClass::CrossedOut,
                darkness: rng.random_range(0.6f32..=0.95),
                coverage: rng.random_range(0.8f32..=1.0),
                instrument: inst,
            });
        }
    }
    // Under fill_cancels_check every selection is a tick or X; otherwise a tick is an
    // occasional deviation from the instructions (review under §8.8).
    let tick = if fcc {
        !chosen.is_empty()
    } else {
        chosen.len() == 1 && rng.random_bool(rates.check_or_x)
    };
    for &o in &chosen {
        if tick {
            marks.push(BubbleMark {
                option: o,
                class: MarkClass::CheckOrX,
                darkness: rng.random_range(0.6f32..=0.9),
                coverage: rng.random_range(0.1f32..=0.35),
                instrument: inst,
            });
        } else {
            let light = inst == Instrument::Pencil && rng.random_bool(rates.light);
            marks.push(fill(rng, o, inst, base, light));
        }
    }
    if chosen.len() == 1
        && rng.random_bool(rates.dot)
        && let Some(o) = pick_other(rng, options, &chosen)
        && !marks.iter().any(|m| m.option == o)
    {
        marks.push(BubbleMark {
            option: o,
            class: MarkClass::PartialOrDot,
            darkness: rng.random_range(0.5f32..=0.9),
            coverage: rng.random_range(0.03f32..=0.25),
            instrument: inst,
        });
    }
    let mut classes = vec![MarkClass::Empty; usize::from(options)];
    for m in &marks {
        classes[usize::from(m.option)] = m.class;
    }
    // A tick is the normal selection under fill_cancels_check, and a fill its cancellation.
    let difficulty = if marks.iter().any(|m| match m.class {
        MarkClass::PartialOrDot | MarkClass::CrossedOut => true,
        MarkClass::CheckOrX => !fcc,
        MarkClass::Filled => fcc,
        _ => false,
    }) {
        Difficulty::Ambiguous
    } else if chosen.len() > 1 {
        Difficulty::Multi
    } else if marks.iter().any(|m| m.class == MarkClass::ErasedResidue) {
        Difficulty::Erasure
    } else if marks.iter().any(|m| m.class == MarkClass::LightFill) {
        Difficulty::Light
    } else if chosen.is_empty() {
        Difficulty::Blank
    } else {
        Difficulty::Easy
    };
    QuestionScript {
        question,
        expected: expected_outcome(&classes, mode),
        intended,
        classes,
        marks,
        difficulty,
    }
}

/// Draws an intended response for a question.
pub fn draw_response(rng: &mut ChaCha8Rng, options: u8, rates: &MarkRates) -> Response {
    if rng.random_bool(rates.blank) {
        Response::Blank
    } else if options >= 2 && rng.random_bool(rates.multi) {
        let a = rng.random_range(0..options);
        let mut b = rng.random_range(0..options - 1);
        if b >= a {
            b += 1;
        }
        let mut v = vec![a, b];
        v.sort_unstable();
        Response::Multi(v)
    } else {
        Response::Single(rng.random_range(0..options))
    }
}

/// Everything the student does on one booklet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetScript {
    /// Form (1 = A).
    pub form: u16,
    /// Booklet serial.
    pub serial: u32,
    /// Student ID bubbled in the ID grid (`None` when the template has no grid).
    pub student_id: Option<String>,
    /// Instrument.
    pub instrument: Instrument,
    /// Typical pencil darkness of this student.
    pub base_darkness: f32,
    /// Per question, ordered by question.
    pub questions: Vec<QuestionScript>,
    /// Number of stray marks to scatter (per sheet, on page 1… n).
    pub strays: u32,
}

impl SheetScript {
    /// A random script: intended responses drawn with `rates`.
    pub fn random(
        template: &Template,
        form: u16,
        serial: u32,
        student_id: Option<String>,
        rates: &MarkRates,
        rng: &mut ChaCha8Rng,
    ) -> Self {
        let responses: Vec<Response> = (0..template.question_count())
            .map(|q| {
                let n = template.question(q).map_or(4, |g| g.options);
                draw_response(rng, n, rates)
            })
            .collect();
        Self::from_responses(template, form, serial, student_id, &responses, rates, rng)
    }

    /// A script that realises the given intended responses (one per question; missing ones
    /// are blank), styled with `rates` (erasures, light fills, dots, ticks, cross-outs).
    pub fn from_responses(
        template: &Template,
        form: u16,
        serial: u32,
        student_id: Option<String>,
        responses: &[Response],
        rates: &MarkRates,
        rng: &mut ChaCha8Rng,
    ) -> Self {
        let instrument = if rng.random_bool(rates.pen_sheet) {
            Instrument::Pen
        } else {
            Instrument::Pencil
        };
        let base_darkness = rng.random_range(0.62f32..=0.9);
        let mode = template.cancel_mode();
        let questions = (0..template.question_count())
            .map(|q| {
                let n = template.question(q).map_or(4, |g| g.options);
                let r = responses
                    .get(usize::from(q))
                    .cloned()
                    .unwrap_or(Response::Blank);
                let style = Style {
                    mode,
                    instrument,
                    base_darkness,
                };
                script_question(rng, q, n, r, style, rates)
            })
            .collect();
        let strays = poisson(rng, rates.stray_per_sheet);
        Self {
            form,
            serial,
            student_id,
            instrument,
            base_darkness,
            questions,
            strays,
        }
    }
}

/// Knuth's Poisson sampler (small means only).
fn poisson(rng: &mut ChaCha8Rng, mean: f64) -> u32 {
    if mean <= 0.0 {
        return 0;
    }
    let l = (-mean).exp();
    let mut k = 0;
    let mut p = 1.0;
    loop {
        p *= rng.random::<f64>();
        if p <= l || k > 1_000 {
            return k;
        }
        k += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MarkClass::*;
    use rand::SeedableRng;

    #[test]
    fn section_8_8_table() {
        let e = CancelMode::EraseOnly;
        let x = CancelMode::CrossCancels;
        let f = CancelMode::FillCancelsCheck;
        assert_eq!(selection_state(Filled, e, false), Some(true));
        assert_eq!(selection_state(Filled, f, false), Some(false));
        assert_eq!(selection_state(LightFill, f, false), None);
        assert_eq!(selection_state(CrossedOut, x, false), Some(false));
        assert_eq!(selection_state(CrossedOut, e, false), None);
        assert_eq!(selection_state(CheckOrX, f, false), Some(true));
        assert_eq!(selection_state(CheckOrX, e, false), None);
        assert_eq!(selection_state(PartialOrDot, e, true), Some(false));
        assert_eq!(selection_state(PartialOrDot, e, false), None);
        assert_eq!(selection_state(CircledLabel, x, true), None);
    }

    #[test]
    fn expected_outcomes() {
        let e = CancelMode::EraseOnly;
        assert_eq!(
            expected_outcome(&[Empty, ErasedResidue, Filled, Empty], e),
            Expected::Auto(Response::Single(2))
        );
        assert_eq!(
            expected_outcome(&[Empty; 4], e),
            Expected::Auto(Response::Blank)
        );
        assert_eq!(
            expected_outcome(&[Filled, Empty, LightFill], e),
            Expected::Auto(Response::Multi(vec![0, 2]))
        );
        assert_eq!(
            expected_outcome(&[PartialOrDot, Empty], e),
            Expected::Review
        );
        assert_eq!(
            expected_outcome(&[PartialOrDot, Filled], e),
            Expected::Auto(Response::Single(1))
        );
        assert_eq!(
            expected_outcome(&[CrossedOut, Filled], CancelMode::CrossCancels),
            Expected::Auto(Response::Single(1))
        );
        assert_eq!(expected_outcome(&[CrossedOut, Filled], e), Expected::Review);
        // White-out: "N; R if it is the only mark" (§8.8), whatever the other mark is.
        assert_eq!(expected_outcome(&[Whiteout, Empty], e), Expected::Review);
        assert_eq!(
            expected_outcome(&[Whiteout, Filled], e),
            Expected::Auto(Response::Single(1))
        );
        assert_eq!(
            expected_outcome(&[Whiteout, CrossedOut], CancelMode::CrossCancels),
            Expected::Auto(Response::Blank)
        );
    }

    #[test]
    fn intents_are_normalised_to_markable_responses() {
        assert_eq!(normalise_intent(Response::Single(7), 4), Response::Blank);
        assert_eq!(
            normalise_intent(Response::Multi(vec![3, 1, 3, 9]), 4),
            Response::Multi(vec![1, 3])
        );
        assert_eq!(
            normalise_intent(Response::Multi(vec![2, 2]), 4),
            Response::Single(2)
        );
        assert_eq!(
            normalise_intent(Response::Set(vec![0, 2]), 4),
            Response::Multi(vec![0, 2])
        );
        assert_eq!(
            normalise_intent(Response::Unreadable { bubbles: vec![1] }, 4),
            Response::Blank
        );
        // Out-of-range options never panic and never reach the painted marks.
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let s = script_question(
            &mut rng,
            0,
            4,
            Response::Multi(vec![1, 200]),
            Style {
                mode: CancelMode::EraseOnly,
                instrument: Instrument::Pencil,
                base_darkness: 0.8,
            },
            &MarkRates::clean(),
        );
        assert_eq!(s.intended, Response::Single(1));
        assert!(s.marks.iter().all(|m| m.option < 4));
    }

    #[test]
    fn fill_cancels_check_students_tick_to_select_and_fill_to_cancel() {
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        let mut rates = MarkRates::hard();
        rates.crossed_out = 0.5;
        let style = Style {
            mode: CancelMode::FillCancelsCheck,
            instrument: Instrument::Pencil,
            base_darkness: 0.8,
        };
        let mut cancels = 0;
        for q in 0..500u16 {
            let s = script_question(&mut rng, q, 5, Response::Single(2), style, &rates);
            assert_eq!(s.classes[2], CheckOrX, "{s:?}");
            assert!(!s.classes.contains(&CrossedOut));
            cancels += usize::from(s.classes.contains(&Filled));
            assert_eq!(s.expected, Expected::Auto(Response::Single(2)), "{s:?}");
        }
        assert!(cancels > 100, "cancellations are fills: {cancels}");
    }

    #[test]
    fn rates_are_respected() {
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let r = MarkRates::standard();
        let n = 20_000;
        let blanks = (0..n)
            .filter(|_| draw_response(&mut rng, 5, &r) == Response::Blank)
            .count();
        let p = blanks as f64 / f64::from(n);
        assert!((p - r.blank).abs() < 0.01, "blank rate {p}");
    }

    #[test]
    fn scripted_questions_are_consistent() {
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let r = MarkRates::hard();
        for mode in [
            CancelMode::EraseOnly,
            CancelMode::CrossCancels,
            CancelMode::FillCancelsCheck,
        ] {
            let mut auto = 0;
            for q in 0..2_000u16 {
                let intended = draw_response(&mut rng, 5, &r);
                let s = script_question(
                    &mut rng,
                    q,
                    5,
                    intended.clone(),
                    Style {
                        mode,
                        instrument: Instrument::Pencil,
                        base_darkness: 0.8,
                    },
                    &r,
                );
                assert_eq!(s.classes.len(), 5);
                assert_eq!(s.intended, intended);
                // Under every cancel mode, an auto outcome always equals the intent.
                if let Expected::Auto(resp) = &s.expected {
                    assert_eq!(resp, &s.intended, "{mode:?} {s:?}");
                    auto += 1;
                }
                // Options are marked at most once.
                let mut opts: Vec<u8> = s.marks.iter().map(|m| m.option).collect();
                opts.sort_unstable();
                opts.dedup();
                assert_eq!(opts.len(), s.marks.len());
            }
            assert!(auto > 1_800, "{mode:?}: only {auto} auto-acceptable");
        }
    }
}
