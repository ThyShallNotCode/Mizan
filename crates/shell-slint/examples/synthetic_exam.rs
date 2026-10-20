//! Writes the synthetic workspace the real-backend screenshots use (no real data): three IAU
//! exams on the committed blank form with painted pencil marks, graded through omr-app; the
//! midterm in two scan batches (the second added to the graded exam), and a final graded with
//! the instructor's synthetic key sheets (held until the key summary is confirmed). Beside them,
//! `remark-cs211-midterm-1/` and `remark-cs212-quiz-2/` hold synthetic Remark graded workbooks
//! of the midterm's and the quiz's sheets (invented rows in the documented export layout, never
//! a real export) for the Remark import and parallel-run screens, and `new-cs213-quiz-1/` the
//! files of an exam not graded yet (its scans, key, roster and CLOs) for the New-exam card.
//!
//! ```text
//! cargo run -p shell-slint --example synthetic_exam -- <workspace-dir>
//! omr-desktop --workspace <workspace-dir> --screenshot docs/design/screens/real \
//!     --remark <workspace-dir>/remark-cs211-midterm-1 \
//!     --remark-setup <workspace-dir>/remark-cs212-quiz-2 \
//!     --new-exam "<workspace-dir>/new-cs213-quiz-1/CS213 Quiz 1"
//! cargo run -p shell-slint --example synthetic_exam -- <dir> --bare   # CS 213 only: no roster,
//!                                                     # no CLOs, an inferred key (edge states)
//! ```
#![forbid(unsafe_code)]

#[path = "../tests/support/synthetic_exam.rs"]
mod synthetic_exam;
#[path = "../tests/support/synthetic_remark.rs"]
mod synthetic_remark;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!(
            "usage: synthetic_exam <workspace-dir> [--bare]\n  writes three graded synthetic exams \
             (CS 211 Midterm 1, CS 212 Quiz 2, CS 214 Final with key sheets) below the folder \
             and the files of CS 213 Quiz 1, not graded yet, or with --bare only CS 213 Quiz 1 \
             graded (no roster, no CLOs, an inferred key); give a new or empty folder"
        );
        return ExitCode::from(2);
    };
    let bare = std::env::args().nth(2).as_deref() == Some("--bare");
    let specs: Vec<&synthetic_exam::Spec> = if bare {
        vec![&synthetic_exam::BARE]
    } else {
        vec![
            &synthetic_exam::QUIZ,
            &synthetic_exam::FINAL,
            &synthetic_exam::MIDTERM,
        ]
    };
    for spec in specs {
        if dir.join(spec.folder).join("project.omrproj").exists() {
            eprintln!(
                "synthetic_exam: {} already holds {}; give a new folder or delete it first",
                dir.display(),
                spec.folder
            );
            return ExitCode::FAILURE;
        }
        let t0 = Instant::now();
        // the midterm's last four sheets come in a second batch (one in portrait, with a
        // blank back), added to the graded exam
        let built = if spec.folder == synthetic_exam::FINAL.folder {
            synthetic_exam::build_with_key_sheets(spec, &dir)
        } else if spec.keyed && spec.roster_and_clos {
            synthetic_exam::build_in_two_batches(spec, &dir, spec.sheets - 4)
        } else {
            synthetic_exam::build(spec, &dir)
        };
        println!(
            "{} {} · {} sheets → {} ({:.1} s)",
            spec.course_code,
            spec.title,
            spec.sheets,
            built.project.display(),
            t0.elapsed().as_secs_f64()
        );
        // synthetic Remark exports: the midterm's with weighted points (the New-exam import,
        // the parallel run), the quiz's with its own 1-point questions (Setup's import, which
        // replaces its inferred key)
        if spec.folder == synthetic_exam::MIDTERM.folder
            || spec.folder == synthetic_exam::QUIZ.folder
        {
            let books = synthetic_remark::write_books_with(
                spec,
                &dir.join(format!("remark-{}", spec.folder)),
                spec.folder == synthetic_exam::QUIZ.folder,
            );
            println!(
                "{} {} · {} synthetic Remark workbooks → {}",
                spec.course_code,
                spec.title,
                books.len(),
                dir.join(format!("remark-{}", spec.folder)).display()
            );
        }
    }
    if !bare {
        // the next exam, not graded yet: what the New-exam card is made from
        let spec = &synthetic_exam::NEXT;
        let files = synthetic_exam::write_exam_files(spec, &dir.join(NEXT_FOLDER));
        println!(
            "{} {} · {} sheets, key, roster and CLOs, not graded → {}",
            spec.course_code,
            spec.title,
            spec.sheets,
            files.scans.display()
        );
    }
    ExitCode::SUCCESS
}

/// The folder of the exam not graded yet, below the workspace.
const NEXT_FOLDER: &str = "new-cs213-quiz-1";
