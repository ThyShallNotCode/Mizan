//! Writes the synthetic workspace the real-backend screenshots use (no real data): three IAU
//! exams on the committed blank form with painted pencil marks, graded through omr-app; the
//! midterm in two scan batches (the second added to the graded exam), and a final graded with
//! the instructor's synthetic key sheets (held until the key summary is confirmed).
//!
//! ```text
//! cargo run -p shell-slint --example synthetic_exam -- <workspace-dir>
//! omr-desktop --workspace <workspace-dir> --screenshot docs/design/screens/real
//! cargo run -p shell-slint --example synthetic_exam -- <dir> --bare   # CS 213 only: no roster,
//!                                                     # no CLOs, an inferred key (edge states)
//! ```
#![forbid(unsafe_code)]

#[path = "../tests/support/synthetic_exam.rs"]
mod synthetic_exam;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!(
            "usage: synthetic_exam <workspace-dir> [--bare]\n  writes three graded synthetic exams \
             (CS 211 Midterm 1, CS 212 Quiz 2, CS 214 Final with key sheets) below the folder, \
             or with --bare only CS 213 Quiz 1 (no roster, no CLOs, an inferred key); give a new \
             or empty folder"
        );
        return ExitCode::from(2);
    };
    let specs: Vec<&synthetic_exam::Spec> = if std::env::args().nth(2).as_deref() == Some("--bare")
    {
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
    }
    ExitCode::SUCCESS
}
