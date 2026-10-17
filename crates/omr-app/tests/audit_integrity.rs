//! The project's audit integrity (§14.4), proven on synthetic IAU sheets only:
//! - the session snapshot is a cache bound to the event chain: a snapshot edited outside the
//!   app is refused on open, read-only too (SYS-011), and so are exports of it (SEC-01);
//! - a forged binding does not help: the responses, students and key that decide the grades
//!   are replayed from the store, whose decision tables are replayed against their events;
//! - checkpoints are signed with the device key and kept outside the package, so an older copy
//!   or a cut-short chain is refused, and `manifest.json` carries a verifiable signature
//!   (SEC-02);
//! - an interrupted save is completed on the next open; a damaged snapshot is SYS-010 with
//!   remediation, never a panic (SEC-10).

mod common;

use common::*;
use omr_app::{Action, Decision, ExamProject, ItemKind, KeySource, RosterStudent};
use omr_store::{Actor, NewEvent, OpenOptions, Project};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const N: u16 = 20;

fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

fn student(k: u16) -> String {
    format!("22104567{k:02}")
}

/// Five sheets of a two-form exam: some wrong answers, one sheet with a faint second mark
/// (a review item); the last sheet bubbles a student number that is not on the roster (an
/// identity item), whose rightful owner, student 4, handed in no other sheet.
fn exam(dir: &Path) -> (PathBuf, ExamProject) {
    let e = iau();
    let scans = dir.join("scans");
    for k in 0..5u16 {
        let answers: Vec<Option<u8>> = (0..N)
            .map(|q| {
                Some(if (q + k) % 4 == 0 {
                    (key(k % 2, q) + 1) % 5
                } else {
                    key(k % 2, q)
                })
            })
            .collect();
        let id = if k == 4 { student(9) } else { student(k) };
        let mut m = Marks::new(&answers, &id, Some((k % 2) as u8));
        if k == 1 {
            let o = (key(1, 3) + 2) % 5;
            m.light
                .push(bubble(&e, omr_forms::legacy::LegacyField::Answers, 3, o));
        }
        write(
            &scans,
            &format!("sheet-{k:02}.png"),
            &png(&sheet(&e, &m, 300 + u64::from(k))),
        );
    }
    let mut csv = String::from("question,A,B\n");
    for q in 0..N {
        csv.push_str(&format!(
            "{},{},{}\n",
            q + 1,
            char::from(b'A' + key(0, q)),
            char::from(b'A' + key(1, q))
        ));
    }
    let forms = vec!["A".to_string(), "B".to_string()];
    let (k, _) = omr_app::exam::read_key_file(csv.as_bytes(), &forms, N, 5).unwrap();
    let mut def = legacy_def("Midterm", N, &["A", "B"], KeySource::Provided { forms: k });
    def.roster = Some(
        (0..5)
            .map(|k| RosterStudent {
                id: student(k),
                name: format!("Student {k}"),
                name_ar: String::new(),
                section: "101".into(),
                numeric_cell: false,
            })
            .collect(),
    );
    def.exam_id = def.derived_exam_id();
    def.validate().unwrap();
    let out = dir.join("out");
    let p = grade(&def, &[scans], &out, 2);
    (out, p)
}

fn package(out: &Path) -> PathBuf {
    out.join("project.omrproj")
}

fn session_path(out: &Path) -> PathBuf {
    package(out).join("cache").join("session.json")
}

/// Every answer of booklet `b` set to its form's key in the snapshot (the session header
/// holds the key, the first booklet chunk the booklet): what a forger wants.
fn forge_full_marks(out: &Path, b: usize) {
    let header: Value = serde_json::from_slice(&std::fs::read(session_path(out)).unwrap()).unwrap();
    let chunk = package(out).join("cache").join("booklets-00000.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&chunk).unwrap()).unwrap();
    let form = v[b]["form"].as_u64().unwrap() as usize;
    let key = header["key"]["forms"][form].clone();
    let qs = v[b]["questions"].as_array_mut().unwrap();
    for (q, c) in qs.iter_mut().zip(key.as_array().unwrap()) {
        q["current"] = json!({"Single": c["Options"][0]});
    }
    std::fs::write(&chunk, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

fn err_text(r: Result<ExamProject, omr_app::AppError>) -> String {
    match r {
        Ok(_) => panic!("opened although it must be refused"),
        Err(e) => {
            assert_eq!(e.exit_code(), 4, "{e}");
            e.to_string()
        }
    }
}

fn store_opts() -> OpenOptions {
    OpenOptions {
        check_location: false,
        ..OpenOptions::default()
    }
}

#[test]
fn a_session_snapshot_edited_outside_the_app_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let (out, p) = exam(d.path());
    let before = std::fs::read(out.join("results.csv")).unwrap();
    p.close().unwrap();
    // The reproduction of SEC-01: full marks for booklet 2 in the snapshot only.
    forge_full_marks(&out, 1);
    for e in [
        err_text(ExamProject::open(&package(&out), "principal:test")),
        err_text(ExamProject::open_read_only(&package(&out))),
    ] {
        assert!(e.contains("SYS-011"), "{e}");
        assert!(e.contains("Restore the project folder"), "{e}");
    }
    // Nothing was exported from it.
    assert_eq!(std::fs::read(out.join("results.csv")).unwrap(), before);
}

#[test]
fn a_forged_snapshot_binding_is_caught_by_the_store_replay() {
    let d = tempfile::tempdir().unwrap();
    let (out, p) = exam(d.path());
    let exam_id = p.session.definition.exam_id.to_string();
    p.close().unwrap();
    forge_full_marks(&out, 1);
    // The forger also rewrites the chunk index and appends a chained `SnapshotSaved` with the
    // new hashes.
    let cache = package(&out).join("cache");
    let h = |n: &str| omr_store::jcs::sha384_hex(&std::fs::read(cache.join(n)).unwrap());
    let mut index: Vec<String> =
        serde_json::from_slice(&std::fs::read(cache.join("booklets.json")).unwrap()).unwrap();
    index[0] = h("booklets-00000.json");
    std::fs::write(
        cache.join("booklets.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    let mut st = Project::open(&package(&out), store_opts()).unwrap();
    st.append_event(&NewEvent::new(
        "SnapshotSaved",
        "exam",
        &exam_id,
        Actor::system(),
        json!({
            "session_sha384": h("session.json"),
            "pages_sha384": h("pages.json"),
            "booklets_index_sha384": h("booklets.json"),
        }),
    ))
    .unwrap();
    st.close().unwrap();
    // The device's external checkpoint now lags the chain (an appended tail is allowed):
    // the replay of the decisions is what refuses it.
    let e = err_text(ExamProject::open(&package(&out), "principal:test"));
    assert!(e.contains("SYS-011"), "{e}");
    assert!(e.contains("audited review decisions"), "{e}");
}

#[test]
fn a_decision_table_edited_behind_the_chain_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let (out, p) = exam(d.path());
    p.close().unwrap();
    let st = Project::open(&package(&out), store_opts()).unwrap();
    st.conn()
        .execute(
            "UPDATE identity_decision SET student_id = '2200000000' WHERE student_id IS NOT NULL",
            (),
        )
        .unwrap();
    drop(st);
    let e = err_text(ExamProject::open(&package(&out), "principal:test"));
    assert!(
        e.contains("SYS-011") && e.contains("identity_decision"),
        "{e}"
    );
}

#[test]
fn an_older_copy_of_the_project_is_refused_against_the_device_checkpoint() {
    let d = tempfile::tempdir().unwrap();
    let (out, p) = exam(d.path());
    p.close().unwrap();
    let copy = d.path().join("copy");
    copy_dir(&package(&out), &package(&copy));
    // The original goes on: a review decision, an export, close (signed checkpoints).
    let mut p = ExamProject::open(&package(&out), "principal:test").unwrap();
    let id = p.queue(None)[0].id;
    p.apply(id, Action::Defer).unwrap();
    p.export(&out).unwrap();
    p.close().unwrap();
    let e = err_text(ExamProject::open(&package(&copy), "principal:test"));
    assert!(e.contains("SYS-011") && e.contains("older copy"), "{e}");
    // The current copy still opens.
    ExamProject::open(&package(&out), "principal:test")
        .unwrap()
        .close()
        .unwrap();
}

#[test]
fn the_export_manifest_is_signed_with_the_device_key() {
    let d = tempfile::tempdir().unwrap();
    let (out, p) = exam(d.path());
    p.close().unwrap();
    let text = std::fs::read(out.join("manifest.json")).unwrap();
    let mut v: Value = serde_json::from_slice(&text).unwrap();
    let sig = v.as_object_mut().unwrap().remove("signature").unwrap();
    let canonical = omr_store::jcs::value_to_jcs(&v).unwrap();
    // The signature opens over the canonical manifest without it…
    use p384::ecdsa::signature::Verifier;
    let vk = p384::ecdsa::VerifyingKey::from_sec1_bytes(
        &hex::decode(sig["public_key"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let s =
        p384::ecdsa::Signature::from_slice(&hex::decode(sig["value"].as_str().unwrap()).unwrap())
            .unwrap();
    vk.verify(canonical.as_bytes(), &s).unwrap();
    assert!(
        vk.verify(canonical.replace("results", "resultz").as_bytes(), &s)
            .is_err()
    );
    // …and the checkpoint it carries is signed by the same key.
    let cp = &sig["checkpoint"];
    let cp = omr_store::Checkpoint {
        project: cp["project"].as_str().unwrap().into(),
        seq: cp["seq"].as_i64().unwrap(),
        head_hash: cp["head_hash"].as_str().unwrap().into(),
        hlc_ts: cp["hlc_ts"].as_str().unwrap().parse().unwrap(),
        alg: cp["alg"].as_str().unwrap().into(),
        key_id: cp["key_id"].as_str().unwrap().into(),
        public_key: hex::decode(cp["public_key"].as_str().unwrap()).unwrap(),
        signature: hex::decode(cp["signature"].as_str().unwrap()).unwrap(),
    };
    let mut trust = omr_store::TrustedKeys::new();
    trust
        .trust_sec1(&hex::decode(sig["public_key"].as_str().unwrap()).unwrap())
        .unwrap();
    use omr_store::CheckpointVerifier;
    trust.verify(&cp).unwrap();
}

#[test]
fn an_interrupted_save_is_completed_on_the_next_open() {
    let d = tempfile::tempdir().unwrap();
    let (out, p) = exam(d.path());
    p.close().unwrap();
    let before = std::fs::read(session_path(&out)).unwrap();
    let mut p = ExamProject::open(&package(&out), "principal:test").unwrap();
    let id = p.queue(None)[0].id;
    p.apply(id, Action::Defer).unwrap();
    drop(p);
    // The process stopped after the snapshot's event and before its rename.
    let cache = package(&out).join("cache");
    std::fs::rename(session_path(&out), cache.join("session.json.tmp")).unwrap();
    std::fs::write(session_path(&out), &before).unwrap();
    let p = ExamProject::open(&package(&out), "principal:test").unwrap();
    assert_eq!(
        p.session.review.item(id).unwrap().state,
        omr_app::ItemState::Deferred
    );
    assert!(!cache.join("session.json.tmp").exists());
    p.close().unwrap();
}

#[test]
fn a_damaged_or_missing_snapshot_is_sys010_with_remediation_never_a_panic() {
    let d = tempfile::tempdir().unwrap();
    let (out, p) = exam(d.path());
    // Indices outside the exam (a booklet 9999) are refused by the structural check.
    let mut s = p.session.clone();
    s.review.items[0].booklet = Some(9999);
    let e = omr_app::audit::validate(&s).unwrap_err().to_string();
    assert!(e.contains("SYS-010") && e.contains("Restore"), "{e}");
    p.close().unwrap();
    // A snapshot the audit log records but the disk lost.
    std::fs::remove_file(session_path(&out)).unwrap();
    let e = err_text(ExamProject::open(&package(&out), "principal:test"));
    assert!(e.contains("SYS-010") && e.contains("missing"), "{e}");
    // An emptied snapshot.
    std::fs::write(session_path(&out), b"").unwrap();
    let e = err_text(ExamProject::open(&package(&out), "principal:test"));
    assert!(e.contains("SYS-011"), "{e}");
}

#[test]
fn undoing_an_identity_decision_keeps_the_store_and_the_exam_equal() {
    let d = tempfile::tempdir().unwrap();
    let (out, mut p) = exam(d.path());
    let it = p
        .queue(None)
        .iter()
        .find(|i| i.kind == ItemKind::Identity)
        .map(|i| (i.id, i.booklet.unwrap()))
        .expect("the not-on-roster sheet is queued");
    let before = p.session.booklets[it.1].identity.student.clone();
    p.apply_with_reason(
        it.0,
        Action::Resolve {
            decision: Decision::Student(student(4)),
            accepted_suggestion: false,
            note: String::new(),
        },
        Some(omr_app::review::ReviewReason::code("ID_CORRECTION")),
    )
    .unwrap();
    p.close().unwrap();
    // The decision reopens (the store's latest decision is the reviewer's)…
    let mut p = ExamProject::open(&package(&out), "principal:test").unwrap();
    assert_eq!(
        p.session.booklets[it.1].identity.student.as_deref(),
        Some(student(4).as_str())
    );
    // …and so does its undo (a compensating decision is recorded).
    p.undo().unwrap();
    p.close().unwrap();
    let p = ExamProject::open(&package(&out), "principal:test").unwrap();
    assert_eq!(p.session.booklets[it.1].identity.student, before);
    p.close().unwrap();
}
