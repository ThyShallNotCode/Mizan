//! Optional real-data ingest check (restricted data; never copied into the repository).
//!
//! Runs only when both variables are set, and skips cleanly otherwise:
//! - `OMR_IAU_DIR`: a directory of page images (e.g. the 200-dpi IAU pages);
//! - `OMR_SCRATCH_DIR`: where the temporary project is created (the session scratchpad).
//!
//! Prints aggregate counts and rates only: no file names, IDs or per-student values.

mod common;

use common::*;
use omr_store::{Actor, BatchRecord, SpoolOutcome};
use std::time::Instant;

#[test]
fn real_pages_ingest_dedup_and_verify() {
    let (Some(src), Some(scratch)) = (
        std::env::var_os("OMR_IAU_DIR"),
        std::env::var_os("OMR_SCRATCH_DIR"),
    ) else {
        eprintln!("skipped: set OMR_IAU_DIR and OMR_SCRATCH_DIR to run the real-data ingest check");
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(&src)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "tif" | "tiff" | "pdf"
                )
            })
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no images in OMR_IAU_DIR");
    let dir = tempfile::Builder::new()
        .prefix("omr-store-real-")
        .tempdir_in(&scratch)
        .unwrap();
    let root = dir.path().join("Exam.omrproj");
    let mut p = project_with_exam(&root);
    p.decision(|tx| {
        tx.create_batch(
            &BatchRecord {
                batch_id: "real".into(),
                exam_id: exam_id(),
                header: None,
                campus: None,
                operator: None,
                device_profile: None,
                expected_count: Some(files.len() as i64),
                state: "open".into(),
            },
            &instructor(),
        )
    })
    .unwrap();
    let t0 = Instant::now();
    let mut bytes = 0u64;
    let (mut spooled, mut dup) = (0usize, 0usize);
    for chunk in files.chunks(50).enumerate() {
        let (c, group) = chunk;
        let staged: Vec<_> = group
            .iter()
            .map(|f| {
                let s = p.objects().stage(std::fs::File::open(f).unwrap()).unwrap();
                bytes += s.len;
                s
            })
            .collect();
        let outs = p
            .decision(|tx| {
                staged
                    .iter()
                    .enumerate()
                    .map(|(k, s)| {
                        let i = c * 50 + k;
                        tx.spool_page(
                            "real",
                            &format!("p{i:04}"),
                            i as i64,
                            s,
                            &Actor::engine("ingest"),
                        )
                    })
                    .collect::<omr_store::Result<Vec<_>>>()
            })
            .unwrap();
        for o in outs {
            match o {
                SpoolOutcome::Spooled { .. } => spooled += 1,
                SpoolOutcome::Duplicate { .. } => dup += 1,
            }
        }
    }
    let ingest = t0.elapsed();
    // Re-feeding the first real page is a byte-identical duplicate: CAP-011, no new page.
    let again = p
        .objects()
        .stage(std::fs::File::open(&files[0]).unwrap())
        .unwrap();
    assert!(again.already_present);
    let refeed = p
        .decision(|tx| tx.spool_page("real", "refeed", -1, &again, &Actor::engine("ingest")))
        .unwrap();
    assert!(
        matches!(&refeed, SpoolOutcome::Duplicate { exception, .. } if exception.code == "CAP-011")
    );
    let t1 = Instant::now();
    let verified = p.verify_objects().unwrap();
    let rep = p.verify_chain(None, &[]).unwrap();
    let verify = t1.elapsed();
    p.integrity_check().unwrap();
    let man = p.object_manifest().unwrap();
    assert_eq!(spooled + dup, files.len());
    assert_eq!(verified, man.entries.len());
    assert_eq!(rep.events as usize, 2 + 1 + spooled);
    eprintln!(
        "real-data ingest: {} files, {:.1} MB, {} pages spooled, {} byte-identical duplicates (CAP-011), \
         ingest {:.0} ms ({:.1} MB/s, FULL sync, groups of 50), object+chain verify {:.0} ms, {} events; \
         re-fed page → CAP-011",
        files.len(),
        bytes as f64 / 1e6,
        spooled,
        dup,
        ingest.as_secs_f64() * 1e3,
        bytes as f64 / 1e6 / ingest.as_secs_f64(),
        verify.as_secs_f64() * 1e3,
        rep.events
    );
    p.close().unwrap();
}
