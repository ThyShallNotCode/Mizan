# OMR-Correction

A native, local-first platform for correcting optical-mark-recognition (OMR) exams. It aims to beat Remark Office OMR on speed, accuracy, identity handling and learning-outcome analytics.

**Status:** design phase. The technical design is in [`docs/TECHNICAL_DESIGN.md`](docs/TECHNICAL_DESIGN.md).

## What the design covers

| Area | Sections |
|---|---|
| Executive summary, owner decisions, effort | §1 |
| Goals, success metrics, competitive baseline vs Remark / Scantron / Gradescope | §2–§3 |
| Architecture (Rust core + thin UI shell, process model) and technology stack | §4–§5 |
| Answer-sheet generation, capture and ingestion | §6–§7 |
| Recognition engine (registration, mark classification, calibrated abstention) | §8 |
| Student identity extraction and roster reconciliation | §9 |
| Scoring and the CLO/PLO learning-outcome engine | §10–§11 |
| Item, test and integrity analytics; review and exception handling | §12–§13 |
| Data, audit and reproducibility; server mode; integrations | §14–§16 |
| Reporting, AI governance, security and privacy, i18n and accessibility | §17–§20 |
| Performance, reliability, quality assurance | §21–§23 |
| Packaging and commercial model, delivery plan, risks | §24–§26 |
| Exception code catalogue, glossary | Appendices A–B |

## Key decisions at a glance

- **Language and UI.** A Rust core with a thin native UI shell: Slint first, Qt 6/cxx-qt as fallback. No Electron.
- **Determinism and audit.** Grading is deterministic and key-blind. AI only advises and never makes final decisions.
- **Accuracy contract.** A published silent-error-rate target, backed by calibrated abstention and a fast, keyboard-first review queue.
- **Identity.** Student identity is matched against the roster as a closed set, fusing several independent recognition channels.
- **Outcomes.** A versioned CLO/PLO outcome engine with weighted mappings, notes, attainment policies and accreditation report packs.
- **Local-first.** Works offline, with encrypted evidence and a hash-chained audit log. An optional self-hosted server adds multi-user review and LMS integration.
