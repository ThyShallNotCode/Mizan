//! `omr-store`: the project store (spec §14).
//!
//! - [`project`]: the `Exam.omrproj/` package (§14.1) and its single writer with decision and
//!   bulk durability lanes, idempotent commands and rollback on error or panic (§14.2);
//! - [`db`]: the §14.2 SQLite configuration (version floor, WAL, FULL/NORMAL lanes, STRICT
//!   tables, `trusted_schema = OFF`, DEFENSIVE, checks, snapshots);
//! - [`schema`]: the §14.3 logical model (MVP subset) and forward-only migrations (§14.7);
//! - [`model`]: typed records with their writes (each with its event) and reads;
//! - [`events`]: the append-only event log with its SHA-384 hash chain and verification
//!   (§14.4); [`checkpoint`]: ECDSA P-384 signed checkpoints behind a signer trait;
//! - [`objects`]: the content-addressed object store (BLAKE3 dedup address, SHA-384 integrity,
//!   §14.5); [`location`]: refused locations (SYS-015);
//! - [`jcs`]: RFC 8785 canonical JSON and SHA-384 helpers.
//!
//! This crate does file and database I/O by design; it creates no thread pools (R4).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod checkpoint;
pub mod clock;
pub mod db;
pub mod error;
pub mod events;
pub mod jcs;
pub mod location;
pub mod model;
pub mod objects;
pub mod project;
pub mod schema;

pub use checkpoint::{
    ALG_ECDSA_P384_SHA384, CHECKPOINT_EVERY, Checkpoint, CheckpointSigner, CheckpointVerifier,
    EcdsaP384Signer, TrustedKeys,
};
pub use clock::{Clock, StepClock, SystemClock};
pub use db::Lane;
pub use error::{Result, StoreError};
pub use events::{
    Actor, ChainError, ChainReport, EventRecord, NewEvent, REASON_CODES, Reason, genesis_hash,
    verify_chain,
};
pub use model::{
    Anonymity, AssessmentKind, BatchRecord, BookletRecord, ExamRecord, ExportRecord,
    IdentityDecisionRecord, IdentityHistoryEntry, IdentityMode, IdentityRunRecord, IdentityStatus,
    PageRecognition, PageRecord, PageState, PinnedKind, Reader, RecognitionRunRecord, ReviewAction,
    RosterRowIn, RosterSnapshotIn, RunKind, SpoolOutcome, StoredCorrection, StoredException,
    StoredQuestion, TemplateOrigin,
};
pub use objects::{ManifestEntry, ObjectKind, ObjectManifest, ObjectStore, StagedObject};
pub use project::{
    CommandOutcome, NewProject, OpenOptions, PackagePaths, Project, ProjectHeader, ReadOnlyProject,
    Tx,
};
