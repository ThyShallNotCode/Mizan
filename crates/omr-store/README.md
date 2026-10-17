# omr-store

The project store (spec §14): the `Exam.omrproj/` package, its SQLite database with the §14.2
configuration, the §14.3 logical model (MVP subset), the append-only event log with a SHA-384
hash chain and ECDSA P-384 signed checkpoints (§14.4), and the content-addressed object store
(§14.5). This crate does file and database I/O by design; it creates no threads (R4: the writer
actor thread belongs to `omr-app`).

## Public API outline

| Area | Items |
|---|---|
| Package (§14.1) | `Project::{create(root, NewProject, OpenOptions), open(root, OpenOptions)}`; `OpenOptions { device_id, clock, check_location, lock, verify_chain, signer, verifier, external }` (a checkpoint verifier and externally held checkpoints, checked on open); `PackagePaths` (`project.omr`, `project.db`, `objects/`, `spool/tmp/`, `snapshots/`, `cache/`, `exports/`, `.lock`); `ProjectHeader`; `FORMAT_MAJOR/MINOR`, `DEFAULT_LABEL` = `restricted` |
| Writes (§14.2 lanes) | `Project::decision(|tx| …)` (synchronous FULL), `Project::bulk(|tx| …)` (NORMAL, ≤ `BULK_GROUP_PAGES` = 50 pages), `Project::command(command_id, |tx| …)` → `CommandOutcome::{Applied, AlreadyApplied}`; every closure runs in one transaction under `catch_unwind` (error or panic → rollback, `StoreError::Panicked`); a failed automatic checkpoint after a commit never fails the committed command (`Project::auto_checkpoint_error`, retried after the next commit, returned by `batch_end`/`close`) |
| `Tx` model writes (each appends its event in the same transaction) | `create_exam`, `put_template_revision`, `put_form_map` (master items, π_f/σ_f), `put_key_version` (append-only, event carries only the payload hash) and `put_key_version_from` (the same with the key's provenance in the event, e.g. `source: key_sheet`, the scans and the operator; no key content), `put_policy_version`, `pin(PinnedKind, body)`, `put_roster_snapshot`, `create_batch`/`close_batch`, `insert_object`, `add_page`, `spool_page` (CAP-011 on a byte-identical duplicate), `set_page_state`, `create_booklet`, `renumber_booklets` (a later batch moves the rank serials of sheets without a printed serial; two phases keep (exam, serial) unique; booklets left without pages are `dissolved` past the top; `BookletRenumbered`), `attach_page`, `create_recognition_run`, `record_page_recognition` (per-question response, auto-accepted, logit, codes; exceptions; registration quality; page → `read`), `raise_exception`, `create_identity_run`, `put_identity_decision` (plus an `identity_decision_log` row per decision), `record_review_action` (response correction + `ExceptionResolved`), `resolve_exceptions`
(human decision without a correction: identity, form, page-level items), `reopen_exceptions`
(`ExceptionReopened`, the compensating event of an undone review action), `put_score_run`, `put_mapping_set`, `record_export`, `append_event`, `write_checkpoint`. Engine actors (`engine:<component>`) are refused for corrections, identity confirmations, key and policy versions (§14.4, §15.5); exceptions outside Appendix A are refused; one policy version number names one body |
| Reads | `Project::reader()` / `Reader::new(&conn)`: `exam`, `template_revision`, `form_map`, `key_versions`, `key_version`, `policy_version`, `pinned`, `roster_rows`, `batch`, `pages`, `page`, `pages_for_object`, `question_results`, `exceptions(state)` (rebuilt through `Exception::new`: a stored code outside Appendix A is SYS-010), `identity_decisions`, `identity_history(run, booklet)` (every decision with the nonce that opens its event commitment), `corrections`, `score_run`, `mapping_set`, `objects`, `events`, `checkpoints`, `exports` (content-addressed rows are re-hashed on read; a mismatch is SYS-010) |
| Records | `ExamRecord`, `AssessmentKind`, `Anonymity`, `IdentityMode`, `TemplateOrigin`, `RosterSnapshotIn`/`RosterRowIn`, `BatchRecord`, `BookletRecord`, `PageRecord` (source SHA-384, DPI, capture kind, registration JSON), `PageState` (§14.8), `RecognitionRunRecord`/`RunKind`, `PageRecognition`, `StoredQuestion`, `StoredException`, `IdentityRunRecord`, `IdentityDecisionRecord`/`IdentityStatus`, `IdentityHistoryEntry`, `ReviewAction`, `StoredCorrection`, `ExportRecord`, `SpoolOutcome` |
| Event log (§14.4) | `NewEvent::new(type, entity_type, entity_id, Actor, payload)`, `.with_reason(Reason)`, `.with_engine_manifest`; `Actor::{user, engine, system}`; `Reason::{code, with_note}` with `REASON_CODES` and the note rule; `EventRecord::{canonical_body, compute_hash}`; `genesis_hash(uuid)`; `verify_chain(conn, uuid, Option<&dyn CheckpointVerifier>, external_checkpoints)` → `ChainReport`; `ChainError::{Gap, BrokenLink, HashMismatch, Malformed, CheckpointMismatch, Truncated, BadSignature, UntrustedSigner, WrongProject}` |
| Checkpoints | `Checkpoint { project, seq, head_hash, hlc_ts, alg, key_id, public_key, signature }` with `Checkpoint::message`; traits `CheckpointSigner`, `CheckpointVerifier`; `EcdsaP384Signer::from_secret_bytes` (RFC 6979, SHA-384); `TrustedKeys::{trust_sec1, trust_signer}`; `Project::{checkpoint(&signer), set_signer, batch_end, close}`; automatic checkpoint every `CHECKPOINT_EVERY` = 1,000 events and at batch end / close when a signer is set |
| Objects (§14.5) | `ObjectStore::{stage, stage_bytes, path_of, read, read_verified, verify, list, sweep}`; `StagedObject { blake3, sha384, len, already_present }`; `ObjectKind`; `ObjectManifest { algorithms, entries }` + `sha384()`; `Project::{object_manifest, verify_objects, sweep_orphans}` |
| SQLite (§14.2) | `db::{open_writer, open_reader, configure_writer, set_lane, Lane, quick_check, integrity_check, foreign_key_check, wal_checkpoint_truncate, snapshot_into, check_sqlite_version}`, `APPLICATION_ID` = `0x4F4D5231`, `MIN_SQLITE_VERSION` = 3.51.3 |
| Schema (§14.7) | `schema::{MIGRATIONS, LATEST_VERSION, migrate, migrate_to, user_version, schema_sha384, canonical_schema_sha384, verify_schema}` |
| Other | `jcs::{to_jcs, value_to_jcs, jcs_sha384_hex, sha384, sha384_hex, es_number}` (RFC 8785); `clock::{Clock, SystemClock, StepClock, hlc_next, rfc3339_utc}`; `location::{check_location, unsafe_reason, mount_type_for}` (SYS-015); `StoreError` with `exception()` → SYS-010 / SYS-011 / SYS-015 / SYS-016 |

## Spec sections implemented

- **§14.1** Package layout, `project.omr` marker written last (a crash before it leaves no
  openable project), OS advisory `.lock` (`File::try_lock`; a second writer gets
  `StoreError::Locked`), a `project_lease` heartbeat row, refusal to overwrite a project, the
  canonical-schema hash check on open (an extra or altered table, index or trigger is refused).
- **§14.2** SQLite ≥ 3.51.3 check; WAL; FULL/NORMAL lanes per transaction; `fullfsync` and
  `checkpoint_fullfsync` (effective on macOS); foreign keys; `busy_timeout` 5,000 ms;
  `journal_size_limit` 64 MiB; `secure_delete`; `application_id`/`user_version`; STRICT tables
  and `WITHOUT ROWID` for hash-keyed tables; `trusted_schema = OFF` and
  `SQLITE_DBCONFIG_DEFENSIVE`; 64 MiB writer and 16 MiB `query_only` reader caches with a
  clamped `mmap_size`; `quick_check`/`integrity_check`/`foreign_key_check` (SYS-010);
  `wal_checkpoint(TRUNCATE)` at batch end and close; idempotent `command_id`; rollback under
  `catch_unwind`; SYS-015 location checks (Linux mount types and remote FUSE subtypes, sync
  markers, home sync folders, macOS sync paths, UNC paths).
- **§14.3** MVP subset: `project_header`, `pinned`, `exam` (= assessment), `template_family`,
  `template_revision`, `master_item`, `form`, `form_map`, `key_version` (append-only trigger),
  `policy_version`, `student`, `pii_vault`, `roster_snapshot`, `roster_row`, `batch`, `object`,
  `source_file`, `booklet`, `page`, `recognition_run`, `question_result`, `exception`,
  `identity_run`, `identity_decision`, `response_correction`, `score_run`, `mapping_set`,
  `item_clo_map`, `export`, `events`, `checkpoint`, `commands`, `project_lease`, and
  `identity_decision_log` (schema v3: the projection keeps the latest identity decision, the
  log keeps every one, so a superseded human decision is never lost; v2 projects are
  back-filled with their current decisions on migration) and `review_note` (schema v4: a
  reviewer's note under a random reference with a salted SHA-384; the event log carries only
  `note:<ref>#sha384:…`, so a note can be erased without breaking the chain:
  `Tx::put_note`, `Reader::note`).
  Content-addressed rows are keyed by the SHA-384 of their JCS form and never updated.
- **§14.4** Hybrid model; `events` columns as specified; append-only triggers;
  `hash = SHA-384(prev_hash ‖ JCS(row without hash))`, genesis
  `SHA-384("omr-chain-v1" ‖ project UUID)`; canonical reason codes with the mandatory-note
  rule and `self_approved`; per-device sequence and HLC; payload guards (no PII-named scalar
  members, no floats); student-linked values as `{ref, sha384}` commitments; signed
  checkpoints every 1,000 events and at batch end and close; full verification on open and
  against externally held checkpoints (rewritten, removed, reordered or truncated history).
  On open, `Reader::verify_projections` also replays the decision tables against their
  events (every identity log row and correction opens its event commitment, every identity
  projection row equals the latest log row, every key version equals its event); a table
  edited behind the chain is `ChainError::ProjectionMismatch` (SYS-011).
  `Project::open_read_only` runs the same checks on a read-only connection (dashboards).
  `restrict_to_owner` makes the package and its folders 0700.
- **§14.5** Write order stage → fsync → link without replace → fsync dir → row; BLAKE3 dedup
  address (non-security) and SHA-384 integrity; CAP-011 only for an exact duplicate confirmed
  by equal SHA-384 and length (an address hit with another SHA-384 is `ObjectCorrupt`, never a
  silently dropped page); verified reads;
  orphan and spool sweep after 24 h; the SHA-384 object manifest.
- **§14.7** Forward-only migrations, one transaction each, `VACUUM INTO` snapshot before
  migrating an existing project, `foreign_key_check` + `quick_check` after, SYS-016 for a newer
  schema.
- **§14.8** Page states as a typed enum; one transaction per batch.

## Tests

`cargo test -p omr-store` (all synthetic, temporary directories):
- `tests/tamper.rs`: every column of every event edited → `HashMismatch` at that seq;
  whitespace-only payload edit; deleted, swapped and truncated events; a consistently
  rewritten tail caught by the stored checkpoint, then by an off-machine checkpoint after the
  attacker deletes the stored one; forged checkpoints (attacker key, spoofed key ID); SYS-011 on
  open; automatic checkpoint at 1,000 events.
- `tests/migrations.rs`: migration from empty (canonical schema, all tables STRICT), the §14.2
  pragmas, a v1 project migrated on open with its snapshot and chain intact, SYS-016,
  foreign-database refusal, altered schema refused (SYS-010), writer lock, no overwrite.
- `tests/roundtrip.rs`: every entity through a reopened project, append-only and conflict
  rules, CAP-011 deduplication, object corruption and orphan sweep, BLAKE3/SHA-384 reference
  values.
- `tests/decisions.rs`: superseded identity decisions kept in the log and every identity and
  correction commitment opened against its event; engines refused for decision actions;
  Appendix A validation on write and read; CAP-011 needs an equal SHA-384; policy-version
  conflicts; a failed automatic checkpoint does not fail the committed command; v2 → v3
  back-fill; decision tables edited behind the chain refused on open; reviewer notes stored
  by salted reference and erasable.
- `tests/crash.rs`: error and panic mid-batch roll back the whole batch; idempotent command
  retry; a real `abort()` of a child process in the middle of a second batch leaves the first
  batch intact and nothing of the second.
- `tests/properties.rs` (proptest): JCS fixed point; ECMAScript number round trip; any payload
  edit in a random chain caught at exactly that event.
- `src/checkpoint.rs`: the RFC 6979 A.2.6 P-384/SHA-384 test vector.
- `tests/perf.rs` (ignored timing probe, `--release -- --ignored`): 500 pages × 100 questions
  through the bulk lane in groups of 50 (staging, `spool_page`, `record_page_recognition`)
  took 757 ms (≈ 660 pages/s) on this shared 4-vCPU container; full chain verification of the
  resulting 1,004 events took 10 ms.
- `tests/reopen.rs`: exceptions resolved and reopened by humans only, through events; the
  chain verifies.
- `tests/real_data.rs`: optional; runs only with `OMR_IAU_DIR` (page images) and
  `OMR_SCRATCH_DIR` (where the temporary project is created), re-feeds the first page to check
  CAP-011 on real bytes, and prints aggregate counts only.

## Deviations and deferrals

- **Checkpoint signing** uses a software ECDSA P-384 key supplied as a 48-byte secret
  (`EcdsaP384Signer`). Key generation, the platform/TPM/HSM key store, the hardware-protected
  KEK, rotation and certification of device keys (§19.2, §15.4) are not implemented; the
  `CheckpointSigner` trait is the seam. Tests use fixed, test-only keys. RFC 3161 timestamps
  are not implemented.
- **Encryption at rest is deferred** (docs/IMPLEMENTATION.md §2): objects are stored as
  plaintext originals (no AES-GCM envelopes), roster fields and the key payload are stored
  with `key_id = 'plain'`, and event commitments are `SHA-384(nonce ‖ JCS(value))` with the
  nonce in the projection row rather than ciphertext digests. Crypto-shredding is not possible
  until the vault exists.
- `rusqlite_migration` is not used: the forward-only runner is ~40 lines over `user_version`.
- Hash-keyed tables use hex TEXT keys; `score_run.results` and `key_version.payload` are JSON
  (not postcard); `page_record`/`booklet_marks` blobs are replaced by the queryable
  `question_result` rows the task asked for.
- Not implemented: `.omrx` archives and the untrusted-import path (§14.1 steps 2–3), packs,
  `cache/` management, rolling snapshot schedules and restore drills (§14.7), monthly
  projection replay, incremental (rather than full) chain verification on open, SIEM
  forwarding, server-mode Merkle log, the workspace library (§14.9), `sqlite3_limit` caps,
  Windows `GetDriveTypeW`/Cloud Files and macOS `statfs` location checks, and the remaining
  §14.3 entities (sittings, rubrics, appeals, approvals, tasks, outbox, retention, leases for
  review).
- Event payload guards are heuristics (PII-named scalar members, floats); they do not prove
  the absence of PII.
