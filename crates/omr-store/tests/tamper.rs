//! Tamper evidence of the SHA-384 event chain and its ECDSA P-384 checkpoints (§14.4).
//!
//! The attacker here has the database file: triggers are dropped with raw SQL, rows edited,
//! deleted, reordered or recomputed. Verification must detect every edit up to an externally
//! held checkpoint, and rewritten history whenever a signed checkpoint covers it.

mod common;

use common::*;
use omr_store::events::read_events;
use omr_store::{
    Actor, ChainError, Checkpoint, CheckpointSigner, EventRecord, NewEvent, Project, StoreError,
    TrustedKeys, genesis_hash,
};
use rusqlite::{Connection, params};
use serde_json::json;
use std::path::Path;
use uuid::Uuid;

/// A project with `n` extra events after the exam, closed; returns its root.
fn chain_of(dir: &Path, n: usize) -> std::path::PathBuf {
    let root = dir.join("Exam.omrproj");
    let mut p = project_with_exam(&root);
    for k in 0..n {
        p.append_event(&NewEvent::new(
            "NoteAdded",
            "exam",
            &exam_id().to_string(),
            instructor(),
            json!({"k": k, "text_ref": format!("note:{k}")}),
        ))
        .unwrap();
    }
    p.close().unwrap();
    root
}

fn raw(root: &Path) -> Connection {
    let c = Connection::open(root.join("project.db")).unwrap();
    c.execute_batch(
        "DROP TRIGGER events_append_only_update; DROP TRIGGER events_append_only_delete; \
         DROP TRIGGER checkpoint_append_only_update; DROP TRIGGER checkpoint_append_only_delete;",
    )
    .unwrap();
    c
}

/// Restores the append-only triggers so the schema check passes and the chain check is what
/// catches the edit.
fn restore_triggers(c: &Connection) {
    c.execute_batch(
        "CREATE TRIGGER events_append_only_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER events_append_only_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER checkpoint_append_only_update BEFORE UPDATE ON checkpoint
BEGIN SELECT RAISE(ABORT, 'checkpoints are append-only'); END;
CREATE TRIGGER checkpoint_append_only_delete BEFORE DELETE ON checkpoint
BEGIN SELECT RAISE(ABORT, 'checkpoints are append-only'); END;",
    )
    .unwrap();
}

fn verify(root: &Path, external: &[Checkpoint]) -> Result<omr_store::ChainReport, StoreError> {
    let c = Connection::open(root.join("project.db")).unwrap();
    omr_store::verify_chain(&c, &Uuid::from_u128(PROJECT), None, external)
}

fn chain_err(r: Result<omr_store::ChainReport, StoreError>) -> ChainError {
    match r {
        Err(StoreError::Chain(e)) => e,
        other => panic!("expected a chain error, got {other:?}"),
    }
}

#[test]
fn untouched_chain_verifies_and_links_from_genesis() {
    let d = tempfile::tempdir().unwrap();
    let root = chain_of(d.path(), 5);
    let rep = verify(&root, &[]).unwrap();
    assert_eq!(rep.events, 7);
    let c = Connection::open(root.join("project.db")).unwrap();
    let ev = read_events(&c, 1, 100).unwrap();
    assert_eq!(ev[0].prev_hash, genesis_hash(&Uuid::from_u128(PROJECT)));
    for w in ev.windows(2) {
        assert_eq!(w[1].prev_hash, w[0].hash);
        assert!(w[1].hlc_ts > w[0].hlc_ts);
        assert_eq!(w[1].device_seq, w[0].device_seq + 1);
    }
    assert_eq!(rep.head_hash, ev.last().unwrap().hash);
}

#[test]
fn append_only_triggers_block_edits_through_sql() {
    let d = tempfile::tempdir().unwrap();
    let root = chain_of(d.path(), 1);
    let c = Connection::open(root.join("project.db")).unwrap();
    assert!(
        c.execute("UPDATE events SET payload = '{}' WHERE seq = 2", [])
            .is_err()
    );
    assert!(c.execute("DELETE FROM events WHERE seq = 3", []).is_err());
}

#[test]
fn every_column_edit_of_any_event_is_detected() {
    let d = tempfile::tempdir().unwrap();
    let root = chain_of(d.path(), 3);
    let cols: &[(&str, &str)] = &[
        ("event_id", "'00000000-0000-7000-8000-000000000000'"),
        ("type", "'NoteRemoved'"),
        ("schema_ver", "schema_ver + 1"),
        ("entity_type", "'booklet'"),
        ("entity_id", "'x'"),
        ("actor_id", "'principal:mallory'"),
        ("actor_role", "'Admin'"),
        ("payload", "'{\"k\":99,\"text_ref\":\"note:1\"}'"),
        ("device_id", "'device:other'"),
        ("device_seq", "device_seq + 100"),
        ("hlc_ts", "hlc_ts + 1"),
        ("wall_ts_utc", "'2001-01-01T00:00:00.000Z'"),
        ("reason_code", "'OTHER'"),
        ("reason_note_ref", "'note:x'"),
        ("engine_manifest_hash", "'ab'"),
    ];
    for target in 1..=5i64 {
        for (col, expr) in cols {
            let c = raw(&root);
            c.execute_batch("SAVEPOINT s").unwrap();
            c.execute(
                &format!("UPDATE events SET {col} = {expr} WHERE seq = ?1"),
                [target],
            )
            .unwrap();
            let e = chain_err(omr_store::verify_chain(
                &c,
                &Uuid::from_u128(PROJECT),
                None,
                &[],
            ));
            assert_eq!(
                e,
                ChainError::HashMismatch { seq: target },
                "edit of {col} at seq {target}"
            );
            c.execute_batch("ROLLBACK TO s; RELEASE s").unwrap();
            restore_triggers(&c);
        }
    }
    verify(&root, &[]).unwrap();
}

#[test]
fn whitespace_in_a_stored_payload_is_detected() {
    // Same JSON value, different bytes: the stored text must be the canonical one.
    let d = tempfile::tempdir().unwrap();
    let root = chain_of(d.path(), 1);
    let c = raw(&root);
    c.execute(
        "UPDATE events SET payload = '{\"k\": 0,\"text_ref\":\"note:0\"}' WHERE seq = 3",
        [],
    )
    .unwrap();
    assert_eq!(
        chain_err(verify(&root, &[])),
        ChainError::HashMismatch { seq: 3 }
    );
}

#[test]
fn deleted_event_is_a_gap() {
    let d = tempfile::tempdir().unwrap();
    let root = chain_of(d.path(), 4);
    raw(&root)
        .execute("DELETE FROM events WHERE seq = 3", [])
        .unwrap();
    assert_eq!(
        chain_err(verify(&root, &[])),
        ChainError::Gap {
            expected: 3,
            found: 4
        }
    );
}

#[test]
fn swapped_events_break_the_links() {
    let d = tempfile::tempdir().unwrap();
    let root = chain_of(d.path(), 4);
    let c = raw(&root);
    c.execute_batch(
        "UPDATE events SET seq = -1 WHERE seq = 3; UPDATE events SET seq = 3 WHERE seq = 4; \
         UPDATE events SET seq = 4 WHERE seq = -1;",
    )
    .unwrap();
    assert!(matches!(
        chain_err(verify(&root, &[])),
        ChainError::BrokenLink { seq: 3 } | ChainError::HashMismatch { seq: 3 }
    ));
}

/// Rewrites every event from `from` on with a consistent, recomputed chain (an attacker who
/// holds the file but not the signing key).
fn rewrite_tail(c: &Connection, from: i64) {
    let mut ev = read_events(c, 1, 10_000).unwrap();
    let mut prev = if from == 1 {
        genesis_hash(&Uuid::from_u128(PROJECT))
    } else {
        ev[(from - 2) as usize].hash.clone()
    };
    for e in ev.iter_mut().filter(|e| e.seq >= from) {
        if e.seq == from {
            e.payload = json!({"k": 1000, "text_ref": "note:forged"});
        }
        e.prev_hash = prev.clone();
        e.hash = e.compute_hash().unwrap();
        prev = e.hash.clone();
        let payload = omr_store::jcs::value_to_jcs(&e.payload).unwrap();
        c.execute(
            "UPDATE events SET payload = ?2, prev_hash = ?3, hash = ?4 WHERE seq = ?1",
            params![e.seq, payload, e.prev_hash, e.hash],
        )
        .unwrap();
    }
}

#[test]
fn rewritten_history_is_caught_by_a_signed_checkpoint() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    let signer = test_signer();
    let mut trust = TrustedKeys::new();
    trust.trust_signer(&signer).unwrap();
    let held_off_machine: Checkpoint;
    {
        let mut p = project_with_exam(&root);
        for k in 0..6 {
            p.append_event(&NewEvent::new(
                "NoteAdded",
                "exam",
                "e",
                instructor(),
                json!({"k": k}),
            ))
            .unwrap();
        }
        held_off_machine = p.checkpoint(&signer).unwrap();
        assert_eq!(held_off_machine.seq, 8);
        p.append_event(&NewEvent::new(
            "NoteAdded",
            "exam",
            "e",
            instructor(),
            json!({"k": 99}),
        ))
        .unwrap();
        let rep = p.verify_chain(Some(&trust), &[]).unwrap();
        assert_eq!((rep.events, rep.checkpoints), (10, 1));
        p.close().unwrap();
    }
    // 1. Consistent rewrite of the tail from seq 4: the bare chain verifies…
    let c = raw(&root);
    rewrite_tail(&c, 4);
    let bare = Connection::open(root.join("project.db")).unwrap();
    // …but the stored checkpoint no longer matches.
    assert_eq!(
        chain_err(omr_store::verify_chain(
            &bare,
            &Uuid::from_u128(PROJECT),
            None,
            &[]
        )),
        ChainError::CheckpointMismatch { seq: 8 }
    );
    // 2. The attacker also deletes the stored checkpoint: the chain alone now verifies, which
    //    is exactly the limit §14.4 states; the externally held checkpoint still catches it.
    c.execute("DELETE FROM checkpoint", []).unwrap();
    verify(&root, &[]).unwrap();
    assert_eq!(
        chain_err(verify(&root, std::slice::from_ref(&held_off_machine))),
        ChainError::CheckpointMismatch { seq: 8 }
    );
    // 3. Re-signing with the attacker's own key is refused by the trust store.
    let forged_head = read_events(&c, 8, 1).unwrap()[0].hash.clone();
    let mallory = other_signer();
    let msg = Checkpoint::message(&held_off_machine.project, 8, &forged_head, 1);
    let forged = Checkpoint {
        project: held_off_machine.project.clone(),
        seq: 8,
        head_hash: forged_head,
        hlc_ts: 1,
        alg: mallory.algorithm().into(),
        key_id: mallory.key_id(),
        public_key: mallory.public_key(),
        signature: mallory.sign(&msg).unwrap(),
    };
    let r = omr_store::verify_chain(
        &bare,
        &Uuid::from_u128(PROJECT),
        Some(&trust),
        std::slice::from_ref(&forged),
    );
    assert!(matches!(
        chain_err(r),
        ChainError::UntrustedSigner { seq: 8, .. }
    ));
    // Re-using the genuine key ID with the attacker's public key fails too.
    let spoof = Checkpoint {
        key_id: held_off_machine.key_id.clone(),
        ..forged
    };
    let r = omr_store::verify_chain(
        &bare,
        &Uuid::from_u128(PROJECT),
        Some(&trust),
        std::slice::from_ref(&spoof),
    );
    assert!(matches!(chain_err(r), ChainError::BadSignature { seq: 8 }));
}

#[test]
fn truncated_tail_is_caught_by_an_external_checkpoint() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    let signer = test_signer();
    let cp = {
        let mut p = project_with_exam(&root);
        for k in 0..3 {
            p.append_event(&NewEvent::new(
                "NoteAdded",
                "exam",
                "e",
                instructor(),
                json!({"k": k}),
            ))
            .unwrap();
        }
        let cp = p.checkpoint(&signer).unwrap();
        p.close().unwrap();
        cp
    };
    let c = raw(&root);
    c.execute("DELETE FROM checkpoint", []).unwrap();
    c.execute("DELETE FROM events WHERE seq >= 4", []).unwrap();
    verify(&root, &[]).unwrap(); // a shorter chain is internally consistent…
    assert_eq!(
        chain_err(verify(&root, std::slice::from_ref(&cp))),
        ChainError::Truncated { seq: 5, head: 3 }
    );
}

#[test]
fn tampered_project_refuses_to_open_with_sys_011() {
    let d = tempfile::tempdir().unwrap();
    let root = chain_of(d.path(), 2);
    let c = raw(&root);
    c.execute(
        "UPDATE events SET actor_id = 'principal:mallory' WHERE seq = 2",
        [],
    )
    .unwrap();
    restore_triggers(&c);
    drop(c);
    let e = Project::open(&root, opts()).unwrap_err();
    assert!(
        matches!(e, StoreError::Chain(ChainError::HashMismatch { seq: 2 })),
        "{e}"
    );
    let x = e.exception().unwrap();
    assert_eq!(
        (x.code.as_str(), x.severity),
        ("SYS-011", omr_model::Severity::Blocker)
    );
}

#[test]
fn automatic_checkpoint_every_thousand_events() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    let mut p = new_project(&root);
    p.set_signer(Some(Box::new(test_signer())));
    p.bulk(|tx| {
        for k in 0..1000 {
            tx.append_event(&NewEvent::new(
                "NoteAdded",
                "exam",
                "e",
                Actor::engine("test"),
                json!({"k": k}),
            ))?;
        }
        Ok(())
    })
    .unwrap();
    let cps = p.reader().checkpoints().unwrap();
    assert_eq!(cps.len(), 1);
    assert_eq!(cps[0].seq, 1001);
    let last: Vec<EventRecord> = p.reader().events(1002, 10).unwrap();
    assert_eq!(last[0].event_type, "ChainCheckpoint");
    let mut trust = TrustedKeys::new();
    trust.trust_signer(&test_signer()).unwrap();
    let rep = p.verify_chain(Some(&trust), &[]).unwrap();
    assert_eq!((rep.events, rep.checkpoints), (1002, 1));
    // Close writes a final checkpoint over the ChainCheckpoint event.
    let cp = p.close().unwrap().unwrap();
    assert_eq!(cp.seq, 1002);
}
