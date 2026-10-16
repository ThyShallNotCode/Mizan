//! The audit/event log: append-only rows chained by SHA-384 (spec §14.4).
//!
//! `hash = SHA-384(prev_hash ‖ JCS(row without hash))`, where `prev_hash` enters the digest as
//! its 48 raw bytes and the JCS row carries every other column (the HLC as a decimal string,
//! since it exceeds the JCS-safe integer range). Genesis:
//! `prev_hash = SHA-384("omr-chain-v1" ‖ project UUID)` with the UUID's 16 raw bytes.
//!
//! State lives in ordinary tables; every grading-relevant fact is appended here in the same
//! transaction as its state change (hybrid model). Payloads carry no PII and no plaintext
//! student-linked values: student-linked values enter as `{ref, sha384}` commitments.

use crate::checkpoint::{Checkpoint, CheckpointVerifier};
use crate::clock::{Clock, hlc_next, rfc3339_utc};
use crate::error::{Result, StoreError};
use crate::jcs;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;
use uuid::Uuid;

/// Domain-separation prefix of the genesis hash.
pub const CHAIN_DOMAIN: &[u8] = b"omr-chain-v1";

/// Canonical reason codes (§14.4), stable strings; adding one is a minor format change.
pub const REASON_CODES: &[&str] = &[
    // Marks
    "AMBIGUOUS_MARK",
    "ERASURE_RESIDUE",
    "STRAY_MARK",
    "MULTI_MARK_INTENT_NOTE",
    "SCANNER_ARTIFACT",
    "TEMPLATE_MISALIGN",
    "ROW_SHIFT_CORRECTION",
    "ACCOMMODATION_TRANSCRIPTION",
    // Identity, collation
    "ID_CORRECTION",
    "NOT_ON_ROSTER",
    "DUPLICATE_RESOLVED",
    "ROSTER_DIFF_ACCEPTED",
    "PROVISIONAL_STUDENT_ADDED",
    "RESCAN_REPLACED",
    "PAGE_ATTACHED",
    "NOT_EXAM_MATERIAL",
    "MISSING_PAGE_EXCUSED",
    // Scoring
    "KEY_ERROR",
    "KEY_CHALLENGE_UPHELD",
    "ITEM_VOIDED_FLAWED",
    "BONUS_AWARDED",
    "CLERICAL_CORRECTION",
    "SPECIAL_CONSIDERATION",
    "POLICY_CORRECTION",
    // Appeals, QA
    "APPEAL_UPHELD",
    "APPEAL_DENIED",
    "ADJUDICATION",
    "RERUN_ACCEPTED",
    // Governance
    "ERASURE_REQUEST",
    "RETENTION_EXPIRY",
    "LEGAL_HOLD",
    "BREAK_GLASS",
    "SECURITY_INCIDENT",
    "SOLO_SELF_APPROVAL",
    "KEY_OWNERSHIP_TRANSFER",
    "OTHER",
];

/// Reason codes that require a note (§14.4).
pub const NOTE_REQUIRED: &[&str] = &["SOLO_SELF_APPROVAL", "OTHER"];

/// Payload member names refused as plaintext PII (a guard, not a proof: §14.4 forbids PII and
/// plaintext student-linked values in payloads).
pub const PII_KEYS: &[&str] = &[
    "name",
    "name_ar",
    "name_en",
    "student_name",
    "national_id",
    "iqama",
    "email",
    "phone",
    "mobile",
    "student_id",
    "student",
];

/// Who acted. Engines act as `engine:<component>` and never perform decision actions (§15.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    /// Principal ID (or `engine:<component>`).
    pub id: String,
    /// Role (Cedar role name, or `engine`).
    pub role: String,
}

impl Actor {
    /// A human principal.
    pub fn user(id: &str, role: &str) -> Self {
        Self {
            id: id.into(),
            role: role.into(),
        }
    }

    /// An engine component, e.g. `Actor::engine("recog")`.
    pub fn engine(component: &str) -> Self {
        Self {
            id: format!("engine:{component}"),
            role: "engine".into(),
        }
    }

    /// The system itself (project creation, checkpoints).
    pub fn system() -> Self {
        Self::engine("store")
    }
}

/// A reason code with its optional note reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    /// One of [`REASON_CODES`].
    pub code: String,
    /// Reference to the (encrypted) note; mandatory for `SOLO_SELF_APPROVAL` and `OTHER`.
    pub note_ref: Option<String>,
    /// A self-approved action (carried in the payload as `self_approved = true`).
    pub self_approved: bool,
}

impl Reason {
    /// A reason without a note.
    pub fn code(code: &str) -> Self {
        Self {
            code: code.into(),
            note_ref: None,
            self_approved: false,
        }
    }

    /// A reason with a note reference.
    pub fn with_note(code: &str, note_ref: &str) -> Self {
        Self {
            code: code.into(),
            note_ref: Some(note_ref.into()),
            self_approved: false,
        }
    }

    /// Checks the code and the note rule.
    pub fn validate(&self) -> Result<()> {
        if !REASON_CODES.contains(&self.code.as_str()) {
            return Err(StoreError::Invalid(format!(
                "unknown reason code {}",
                self.code
            )));
        }
        if NOTE_REQUIRED.contains(&self.code.as_str())
            && self.note_ref.as_deref().is_none_or(str::is_empty)
        {
            return Err(StoreError::Invalid(format!(
                "reason {} requires a note",
                self.code
            )));
        }
        Ok(())
    }
}

/// An event to append.
#[derive(Clone, Debug, PartialEq)]
pub struct NewEvent {
    /// PascalCase type, e.g. `PageSpooled`.
    pub event_type: String,
    /// Payload schema version (payloads are upcast on read, never rewritten).
    pub schema_ver: u32,
    /// Entity type, e.g. `page`.
    pub entity_type: String,
    /// Entity ID.
    pub entity_id: String,
    /// Who acted.
    pub actor: Actor,
    /// JSON object; no PII.
    pub payload: Value,
    /// Reason code, if any.
    pub reason: Option<Reason>,
    /// Engine manifest hash, for engine-produced facts.
    pub engine_manifest_hash: Option<String>,
    /// Idempotency key (UUIDv7); generated when `None`.
    pub event_id: Option<Uuid>,
}

impl NewEvent {
    /// An event with schema version 1 and no reason.
    pub fn new(
        event_type: &str,
        entity_type: &str,
        entity_id: &str,
        actor: Actor,
        payload: Value,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            schema_ver: 1,
            entity_type: entity_type.into(),
            entity_id: entity_id.into(),
            actor,
            payload,
            reason: None,
            engine_manifest_hash: None,
            event_id: None,
        }
    }

    /// Adds a reason.
    pub fn with_reason(mut self, r: Reason) -> Self {
        self.reason = Some(r);
        self
    }

    /// Adds the engine manifest hash.
    pub fn with_engine_manifest(mut self, h: &str) -> Self {
        self.engine_manifest_hash = Some(h.into());
        self
    }
}

/// One stored event row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    /// Local monotonic key (1-based, contiguous).
    pub seq: i64,
    /// UUIDv7 idempotency key.
    pub event_id: String,
    /// Event type.
    pub event_type: String,
    /// Payload schema version.
    pub schema_ver: u32,
    /// Entity type.
    pub entity_type: String,
    /// Entity ID.
    pub entity_id: String,
    /// Actor ID.
    pub actor_id: String,
    /// Actor role.
    pub actor_role: String,
    /// Payload object.
    pub payload: Value,
    /// Device that wrote it.
    pub device_id: String,
    /// Per-device sequence.
    pub device_seq: i64,
    /// Hybrid logical clock.
    pub hlc_ts: i64,
    /// Wall-clock time (informational).
    pub wall_ts_utc: String,
    /// Reason code.
    pub reason_code: Option<String>,
    /// Note reference.
    pub reason_note_ref: Option<String>,
    /// Engine manifest hash.
    pub engine_manifest_hash: Option<String>,
    /// Previous event's hash (genesis for seq 1).
    pub prev_hash: String,
    /// This event's hash.
    pub hash: String,
}

impl EventRecord {
    /// JCS of the row without `hash`.
    pub fn canonical_body(&self) -> Result<String> {
        let v = json!({
            "seq": self.seq,
            "event_id": self.event_id,
            "type": self.event_type,
            "schema_ver": self.schema_ver,
            "entity_type": self.entity_type,
            "entity_id": self.entity_id,
            "actor_id": self.actor_id,
            "actor_role": self.actor_role,
            "payload": self.payload,
            "device_id": self.device_id,
            "device_seq": self.device_seq,
            "hlc_ts": self.hlc_ts.to_string(),
            "wall_ts_utc": self.wall_ts_utc,
            "reason_code": self.reason_code,
            "reason_note_ref": self.reason_note_ref,
            "engine_manifest_hash": self.engine_manifest_hash,
            "prev_hash": self.prev_hash,
        });
        Ok(jcs::value_to_jcs(&v)?)
    }

    /// `SHA-384(prev_hash ‖ JCS(row without hash))`, hex.
    pub fn compute_hash(&self) -> Result<String> {
        let prev = hex::decode(&self.prev_hash).map_err(|_| {
            StoreError::Chain(ChainError::Malformed {
                seq: self.seq,
                detail: "prev_hash is not hex".into(),
            })
        })?;
        let mut buf = prev;
        buf.extend_from_slice(self.canonical_body()?.as_bytes());
        Ok(jcs::sha384_hex(&buf))
    }
}

/// Why the chain failed verification (SYS-011).
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ChainError {
    /// A missing, extra or reordered sequence number.
    #[error("sequence gap: expected seq {expected}, found {found}")]
    Gap {
        /// Expected seq.
        expected: i64,
        /// Found seq.
        found: i64,
    },
    /// `prev_hash` does not link to the previous event.
    #[error("event {seq}: prev_hash does not link to the previous event")]
    BrokenLink {
        /// Offending seq.
        seq: i64,
    },
    /// The stored hash does not match the row content.
    #[error("event {seq}: content does not match its hash")]
    HashMismatch {
        /// Offending seq.
        seq: i64,
    },
    /// A row could not be read or canonicalised.
    #[error("event {seq}: malformed ({detail})")]
    Malformed {
        /// Offending seq.
        seq: i64,
        /// Detail.
        detail: String,
    },
    /// A checkpoint's head hash differs from the chain at its seq (rewritten history).
    #[error("checkpoint at seq {seq} does not match the chain")]
    CheckpointMismatch {
        /// Checkpoint seq.
        seq: i64,
    },
    /// A checkpoint covers events the chain no longer has (truncation or rollback).
    #[error("checkpoint at seq {seq} is beyond the chain head {head}")]
    Truncated {
        /// Checkpoint seq.
        seq: i64,
        /// Current head seq.
        head: i64,
    },
    /// A checkpoint signature is invalid.
    #[error("checkpoint at seq {seq}: bad signature")]
    BadSignature {
        /// Checkpoint seq.
        seq: i64,
    },
    /// A checkpoint is signed by an untrusted key.
    #[error("checkpoint at seq {seq}: untrusted signer {key_id}")]
    UntrustedSigner {
        /// Checkpoint seq.
        seq: i64,
        /// Key ID.
        key_id: String,
    },
    /// The checkpoint names another project.
    #[error("checkpoint at seq {seq} belongs to another project")]
    WrongProject {
        /// Checkpoint seq.
        seq: i64,
    },
}

/// Result of a successful verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainReport {
    /// Events verified.
    pub events: u64,
    /// Head seq (0 for an empty chain).
    pub head_seq: i64,
    /// Head hash (genesis for an empty chain).
    pub head_hash: String,
    /// Checkpoints verified.
    pub checkpoints: usize,
}

/// `SHA-384("omr-chain-v1" ‖ project UUID bytes)`, hex.
pub fn genesis_hash(project: &Uuid) -> String {
    let mut buf = CHAIN_DOMAIN.to_vec();
    buf.extend_from_slice(project.as_bytes());
    jcs::sha384_hex(&buf)
}

/// Writer-side event context: device identity, clock and the last HLC issued.
pub(crate) struct EventCtx {
    pub(crate) device_id: String,
    pub(crate) clock: Box<dyn Clock>,
    pub(crate) hlc_last: i64,
    pub(crate) genesis: String,
}

fn validate_new(ev: &NewEvent) -> Result<()> {
    let t = &ev.event_type;
    if t.is_empty()
        || !t.starts_with(|c: char| c.is_ascii_uppercase())
        || !t.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(StoreError::Invalid(format!(
            "event type {t:?} is not PascalCase"
        )));
    }
    if ev.entity_type.is_empty() || ev.actor.id.is_empty() || ev.actor.role.is_empty() {
        return Err(StoreError::Invalid(
            "entity type and actor are required".into(),
        ));
    }
    if let Some(r) = &ev.reason {
        r.validate()?;
    }
    let Value::Object(map) = &ev.payload else {
        return Err(StoreError::Invalid("payload must be a JSON object".into()));
    };
    check_no_pii(map)?;
    Ok(())
}

fn check_no_pii(map: &Map<String, Value>) -> Result<()> {
    for (k, v) in map {
        // A PII-named member may hold a `{ref, sha384}` commitment object, never a scalar.
        if PII_KEYS.contains(&k.to_ascii_lowercase().as_str())
            && matches!(v, Value::String(_) | Value::Number(_))
        {
            return Err(StoreError::Invalid(format!(
                "payload member {k:?} looks like plaintext PII; use a {{ref, sha384}} commitment"
            )));
        }
        match v {
            Value::Object(m) => check_no_pii(m)?,
            Value::Array(a) => {
                for x in a {
                    if let Value::Object(m) = x {
                        check_no_pii(m)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Head (seq, hash) of the chain; `(0, genesis)` when empty.
pub fn head(conn: &Connection, genesis: &str) -> Result<(i64, String)> {
    Ok(conn
        .query_row(
            "SELECT seq, hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or_else(|| (0, genesis.to_owned())))
}

/// Appends `ev` inside the caller's transaction.
pub(crate) fn append(conn: &Connection, ctx: &mut EventCtx, ev: &NewEvent) -> Result<EventRecord> {
    validate_new(ev)?;
    let (prev_seq, prev_hash) = head(conn, &ctx.genesis)?;
    let device_seq: i64 = conn.query_row(
        "SELECT coalesce(max(device_seq), 0) + 1 FROM events WHERE device_id = ?1",
        [&ctx.device_id],
        |r| r.get(0),
    )?;
    let now = ctx.clock.now_unix_ms();
    let hlc = hlc_next(ctx.hlc_last, now);
    ctx.hlc_last = hlc;
    let mut payload = ev.payload.clone();
    if let (Some(r), Value::Object(m)) = (&ev.reason, &mut payload)
        && r.self_approved
    {
        m.insert("self_approved".into(), Value::Bool(true));
    }
    let mut rec = EventRecord {
        seq: prev_seq + 1,
        event_id: ev.event_id.unwrap_or_else(Uuid::now_v7).to_string(),
        event_type: ev.event_type.clone(),
        schema_ver: ev.schema_ver,
        entity_type: ev.entity_type.clone(),
        entity_id: ev.entity_id.clone(),
        actor_id: ev.actor.id.clone(),
        actor_role: ev.actor.role.clone(),
        payload,
        device_id: ctx.device_id.clone(),
        device_seq,
        hlc_ts: hlc,
        wall_ts_utc: rfc3339_utc(now),
        reason_code: ev.reason.as_ref().map(|r| r.code.clone()),
        reason_note_ref: ev.reason.as_ref().and_then(|r| r.note_ref.clone()),
        engine_manifest_hash: ev.engine_manifest_hash.clone(),
        prev_hash,
        hash: String::new(),
    };
    rec.hash = rec.compute_hash()?;
    conn.execute(
        "INSERT INTO events (seq, event_id, type, schema_ver, entity_type, entity_id, actor_id, \
         actor_role, payload, device_id, device_seq, hlc_ts, wall_ts_utc, reason_code, \
         reason_note_ref, engine_manifest_hash, prev_hash, hash) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            rec.seq,
            rec.event_id,
            rec.event_type,
            rec.schema_ver,
            rec.entity_type,
            rec.entity_id,
            rec.actor_id,
            rec.actor_role,
            jcs::value_to_jcs(&rec.payload)?,
            rec.device_id,
            rec.device_seq,
            rec.hlc_ts,
            rec.wall_ts_utc,
            rec.reason_code,
            rec.reason_note_ref,
            rec.engine_manifest_hash,
            rec.prev_hash,
            rec.hash,
        ],
    )?;
    Ok(rec)
}

const SELECT_EVENTS: &str = "SELECT seq, event_id, type, schema_ver, entity_type, entity_id, \
    actor_id, actor_role, payload, device_id, device_seq, hlc_ts, wall_ts_utc, reason_code, \
    reason_note_ref, engine_manifest_hash, prev_hash, hash FROM events";

fn read_row(r: &Row<'_>) -> rusqlite::Result<(EventRecord, String)> {
    let payload_text: String = r.get(8)?;
    Ok((
        EventRecord {
            seq: r.get(0)?,
            event_id: r.get(1)?,
            event_type: r.get(2)?,
            schema_ver: r.get(3)?,
            entity_type: r.get(4)?,
            entity_id: r.get(5)?,
            actor_id: r.get(6)?,
            actor_role: r.get(7)?,
            payload: Value::Null,
            device_id: r.get(9)?,
            device_seq: r.get(10)?,
            hlc_ts: r.get(11)?,
            wall_ts_utc: r.get(12)?,
            reason_code: r.get(13)?,
            reason_note_ref: r.get(14)?,
            engine_manifest_hash: r.get(15)?,
            prev_hash: r.get(16)?,
            hash: r.get(17)?,
        },
        payload_text,
    ))
}

/// Events with `seq ≥ from`, at most `limit`, in order.
pub fn read_events(conn: &Connection, from: i64, limit: usize) -> Result<Vec<EventRecord>> {
    let sql = format!("{SELECT_EVENTS} WHERE seq >= ?1 ORDER BY seq LIMIT ?2");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![from, limit as i64], read_row)?;
    let mut out = Vec::new();
    for row in rows {
        let (mut rec, text) = row?;
        rec.payload = serde_json::from_str(&text)?;
        out.push(rec);
    }
    Ok(out)
}

/// Checkpoints stored in the project, in seq order.
pub fn read_checkpoints(conn: &Connection) -> Result<Vec<Checkpoint>> {
    let mut stmt = conn.prepare(
        "SELECT project, seq, head_hash, hlc_ts, alg, key_id, public_key, signature \
         FROM checkpoint ORDER BY seq",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Checkpoint {
            project: r.get(0)?,
            seq: r.get(1)?,
            head_hash: r.get(2)?,
            hlc_ts: r.get(3)?,
            alg: r.get(4)?,
            key_id: r.get(5)?,
            public_key: r.get(6)?,
            signature: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Full verification: contiguous seq from 1, every link, every hash; then every stored
/// checkpoint (head hash at its seq and, with a verifier, its signature) and every
/// `external` checkpoint held off the machine (detects rewritten or truncated history).
pub fn verify_chain(
    conn: &Connection,
    project: &Uuid,
    verifier: Option<&dyn CheckpointVerifier>,
    external: &[Checkpoint],
) -> Result<ChainReport> {
    let genesis = genesis_hash(project);
    let mut stmt = conn.prepare(&format!("{SELECT_EVENTS} ORDER BY seq"))?;
    let mut rows = stmt.query([])?;
    let mut expected = 1i64;
    let mut prev = genesis.clone();
    // Only (seq → hash) is kept to check checkpoints afterwards.
    let mut hashes: Vec<String> = Vec::new();
    while let Some(row) = rows.next()? {
        let (mut rec, text) = read_row(row)?;
        if rec.seq != expected {
            return Err(ChainError::Gap {
                expected,
                found: rec.seq,
            }
            .into());
        }
        rec.payload = serde_json::from_str(&text).map_err(|e| ChainError::Malformed {
            seq: rec.seq,
            detail: e.to_string(),
        })?;
        if rec.prev_hash != prev {
            return Err(ChainError::BrokenLink { seq: rec.seq }.into());
        }
        let recomputed = rec.compute_hash().map_err(|e| ChainError::Malformed {
            seq: rec.seq,
            detail: e.to_string(),
        })?;
        if recomputed != rec.hash {
            return Err(ChainError::HashMismatch { seq: rec.seq }.into());
        }
        // The stored payload text must itself be canonical, so the row cannot carry bytes
        // that differ from what was hashed.
        if jcs::value_to_jcs(&rec.payload).ok().as_deref() != Some(text.as_str()) {
            return Err(ChainError::HashMismatch { seq: rec.seq }.into());
        }
        prev = rec.hash.clone();
        hashes.push(rec.hash);
        expected += 1;
    }
    let head_seq = expected - 1;
    let project_s = project.to_string();
    let mut checked = 0;
    let stored = read_checkpoints(conn)?;
    for cp in stored.iter().chain(external.iter()) {
        if cp.project != project_s {
            return Err(ChainError::WrongProject { seq: cp.seq }.into());
        }
        if cp.seq > head_seq {
            return Err(ChainError::Truncated {
                seq: cp.seq,
                head: head_seq,
            }
            .into());
        }
        let at = usize::try_from(cp.seq - 1)
            .ok()
            .and_then(|i| hashes.get(i))
            .ok_or(ChainError::CheckpointMismatch { seq: cp.seq })?;
        if *at != cp.head_hash {
            return Err(ChainError::CheckpointMismatch { seq: cp.seq }.into());
        }
        if let Some(v) = verifier {
            v.verify(cp)?;
        }
        checked += 1;
    }
    Ok(ChainReport {
        events: hashes.len() as u64,
        head_seq,
        head_hash: prev,
        checkpoints: checked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_validated() {
        Reason::code("AMBIGUOUS_MARK").validate().unwrap();
        assert!(Reason::code("MADE_UP").validate().is_err());
        assert!(Reason::code("OTHER").validate().is_err());
        Reason::with_note("OTHER", "note:1").validate().unwrap();
        assert!(Reason::code("SOLO_SELF_APPROVAL").validate().is_err());
    }

    #[test]
    fn pii_keys_are_refused_at_any_depth() {
        let ev = NewEvent::new(
            "ResponseCorrected",
            "booklet",
            "b1",
            Actor::user("u1", "Reviewer"),
            json!({"position": 3, "who": {"Name": "x"}}),
        );
        assert!(validate_new(&ev).is_err());
        let ok = NewEvent::new(
            "ResponseCorrected",
            "booklet",
            "b1",
            Actor::user("u1", "Reviewer"),
            json!({"position": 3}),
        );
        validate_new(&ok).unwrap();
        let bad_type = NewEvent { event_type: "page_spooled".into(), ..ok };
        assert!(validate_new(&bad_type).is_err());
    }

    #[test]
    fn genesis_depends_on_the_project() {
        let a = genesis_hash(&Uuid::from_u128(1));
        let b = genesis_hash(&Uuid::from_u128(2));
        assert_ne!(a, b);
        assert_eq!(a.len(), 96);
    }
}
