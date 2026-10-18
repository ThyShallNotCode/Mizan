//! Ground-truth records, one per page image (a line of `truth.jsonl`).

use crate::paint::MarkStats;
use crate::profile::{CaptureParams, Profile};
use crate::script::{Difficulty, Expected, Instrument};
use omr_model::{MarkClass, Response};
use serde::{Deserialize, Serialize};

/// One painted mark with what it achieved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkTruth {
    /// 0-based option.
    pub option: u8,
    /// Appearance class.
    pub class: MarkClass,
    /// Target darkness.
    pub darkness: f32,
    /// Target coverage.
    pub coverage_target: f32,
    /// Achieved interior coverage and mean ink.
    pub achieved: MarkStats,
    /// Instrument.
    pub instrument: Instrument,
}

/// Ground truth of one question on the page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionTruth {
    /// 0-based question.
    pub question: u16,
    /// What the student meant.
    pub intended: Response,
    /// What a perfect recognizer outputs (or `Review`) under the exam's cancel mode (§8.8).
    pub expected: Expected,
    /// Class of every bubble (index = option).
    pub classes: Vec<MarkClass>,
    /// Difficulty tag.
    pub difficulty: Difficulty,
    /// Painted marks.
    pub marks: Vec<MarkTruth>,
}

/// A stray stroke outside every bubble.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrayTruth {
    /// Polyline in template mm.
    pub points_mm: Vec<(f64, f64)>,
    /// Stroke width, mm.
    pub width_mm: f64,
    /// Darkness.
    pub darkness: f32,
}

/// Ground truth of one page image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageTruth {
    /// Sheet index in the corpus.
    pub sheet: u64,
    /// Image file name relative to the corpus root (empty outside a corpus).
    pub file: String,
    /// 1-based page.
    pub page: u16,
    /// Pages per booklet.
    pub pages: u16,
    /// Form code (1 = A).
    pub form: u16,
    /// Booklet serial.
    pub booklet: u32,
    /// The 41-character page-code payload printed on this page.
    pub payload: String,
    /// The bubbled student ID (`None` without an ID grid).
    pub student_id: Option<String>,
    /// ID digits bubbled on this page (page 1 only).
    pub id_digits: Vec<u8>,
    /// Instrument.
    pub instrument: Instrument,
    /// Profile.
    pub profile: Profile,
    /// Capture parameters.
    pub capture: CaptureParams,
    /// Row-major homography: template mm → image px (pixel `(i, j)` covers `[i, i+1)`).
    pub mm_to_px: [f64; 9],
    /// Questions on this page.
    pub questions: Vec<QuestionTruth>,
    /// Stray marks on this page.
    pub strays: Vec<StrayTruth>,
}
