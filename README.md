# OMR-Correction

A native, local-first platform for correcting optical-mark-recognition (OMR) exams, built by a Saudi university for its own colleges, campuses and branches and developed solely by AI agents. It aims to beat Remark Office OMR on speed, accuracy, identity handling and learning-outcome evidence for NCAAA and ABET accreditation.

**Status:** design phase. The technical design is in [`docs/TECHNICAL_DESIGN.md`](docs/TECHNICAL_DESIGN.md) (draft v0.2, revision of 2026-10-16 for a Saudi university owner).

## What the design covers

| Area | Sections |
|---|---|
| Executive summary; owner decisions, resolved and remaining; size of the job | §1 |
| Goals, success metrics, competitive baseline vs Remark Office OMR | §2–§3 |
| Architecture (Rust core + thin UI shell, process model, in-Kingdom deployment modes) and technology stack | §4–§5 |
| Answer-sheet generation, capture and ingestion | §6–§7 |
| Recognition engine (registration, mark classification, calibrated abstention) | §8 |
| Student identity extraction and roster reconciliation (Arabic names, Saudi student-ID and national-ID rules) | §9 |
| Scoring (Saudi grade scale and status codes) and the CLO/PLO engine with NCAAA and ABET report packs | §10–§11 |
| Item, test and integrity analytics; review and exception handling | §12–§13 |
| Data, audit and reproducibility; server mode; integrations (Blackboard, Banner Ethos) | §14–§16 |
| Reporting, AI governance, security and privacy, i18n and accessibility | §17–§20 |
| Saudi regulatory obligations: NCA, SDAIA, CST and DGA | §19.6 |
| Telemetry & crash reporting policy; compliance evidence pack | §19.8 |
| Performance, reliability, quality assurance; oracle integrity under agent-only development | §21–§23 |
| Packaging, distribution and provenance | §24 |
| Delivery plan: phases, spikes and agent operating model | §25 |
| Risks and open questions | §26 |
| Exception code catalogue, glossary | Appendices A–B |

## Key decisions at a glance

- **Owner and posture.** An in-house platform of a Saudi university (public assumed), not a commercial product, with no licence enforcement. Student data stays on university devices or a server in the Kingdom.
- **Built by agents, decided by people.** AI agents build, test and release the software under machine-checkable gates; humans act only at listed institutional touchpoints. Grades, identities, releases and appeals stay with university staff.
- **Language and UI.** A Rust core with a thin native UI shell: Slint first, decided subject to spike S1; Qt 6/cxx-qt only if S1 fails. No Electron. English UI with Arabic data entry; Arabic and English reports; mirrored Arabic UI in P3.
- **Determinism and audit.** Grading is deterministic and key-blind. AI only advises, on local models, and never makes final decisions.
- **Accuracy contract.** A silent-error-rate target certified on a controlled slice (robot-rig sheets, truth by construction) and reported as an achieved bound on field scans until certified, backed by calibrated abstention and a fast, keyboard-first review queue.
- **Identity.** Student identity is matched against the roster as a closed set, fusing several independent recognition channels; the national ID/Iqama is treated as a secret.
- **Outcomes.** A versioned CLO/PLO/SO outcome engine with weighted mappings and attainment policies, producing the NCAAA (TP-151, TP-152, TP-154, TP-158) and ABET packs as DOCX, PDF/A and XLSX.
- **Compliance and cryptography.** PDPL, NCA ECC/DCC/CCC and the NCS cryptographic profile (SHA-384, ECDSA P-384, HPKE, AES-KWP), CST data residency, SDAIA AI governance, DGA WCAG 2.2 AA.
- **Delivery.** P0 from November 2026, supervised pilots at the 1448 AH second-semester finals, internal GA before the 1449 first-semester finals (≈ December 2027).
