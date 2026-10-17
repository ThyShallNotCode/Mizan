# shell-slint: the OMR Correction desktop shell

Rust + Slint 1.18.1 (spec §5.2, ADR-001). The shell renders and binds; it decides nothing. Every
value on screen comes from a `Backend` as plain-Rust view-models, and every user decision goes
back as a typed call. `omr-app` will implement the trait; until then `DemoBackend` serves
synthetic data.

```sh
export CARGO_TARGET_DIR=$PWD/target-gui
cargo run -p shell-slint                                   # windowed (winit), demo data
cargo run -p shell-slint -- --theme dark                   # windowed, dark theme
cargo run -p shell-slint -- --screenshot docs/design/screens            # every screen, both themes
cargo run -p shell-slint -- --screenshot /tmp/shots --theme light       # one theme
cargo run -p shell-slint -- --screenshot /tmp/shots --size 1280x800     # the minimum window
cargo test -p shell-slint                                  # unit tests + A11 acceptance tests
```

`--screenshot` needs no display: it installs a Slint platform with the software renderer at
1440 × 900 (or `--size`) and writes `<dir>/<screen>-<theme>.png` (`-<w>x<h>` is appended for
other sizes). The screens are exams, setup, scan, review, review-identity, review-blind, results,
outcomes and settings.

Every screen fits 1440 × 900 nominal and the 1280 × 800 minimum (DESIGN §9). `app.slint` hands
each screen the window size (set by the platform, never by the content, so there is no layout
loop): below 1400 px side columns narrow and the exam drawer overlays the table; below 880 px the
review keyboard legend gives way and long lists scroll. `req_a11_every_screen_fits_nominal_and_minimum_windows`
asserts it.

## Layout

| Path | Role |
|---|---|
| `src/backend.rs` | The `Backend` trait and its view-models: the integration boundary to `omr-app` |
| `src/demo.rs` | `DemoBackend`: two fully modelled exams (IAU legacy `iau-ccsit-v1`, 2 versions × 29 questions; a generated 40 × 4 sheet with 4 forms) and six summary-only exams |
| `src/crops.rs` | Synthetic scans drawn with `tiny-skia`: answer rows, ID grids, page thumbnails. No real scan anywhere |
| `src/keymap.rs` | Review and global keys, mapped to physical positions (Arabic 101 layout, abjad letters, Arabic-Indic digits) |
| `src/bind.rs` | `Shell`: fills the Slint globals from the backend, handles every callback |
| `src/screenshot.rs` | Headless platform, capture and PNG encoding |
| `ui/theme.slint` | Design tokens and components (DESIGN.md) |
| `ui/state.slint` | View-model globals (`App`, `ExamsVM`, `SetupVM`, `ScanVM`, `ReviewVM`, `ResultsVM`, `OutcomesVM`) |
| `ui/widgets.slint` | Shell widgets: switch, segments, editable cells, scan paper, fill bars, queue rows |
| `ui/screens/*.slint` | One file per screen |
| `tests/req_a11.rs` | A11 acceptance tests |

## Integrating `omr-app`

Implement `shell_slint::backend::Backend` for a wrapper around the `omr-app` handle (inside this
crate, so the orphan rule holds), then pass it to `Shell::bind`:

```rust
let ui = AppWindow::new()?;
let backend: Rc<RefCell<dyn Backend>> = Rc::new(RefCell::new(AppBackend::open(path)?));
let shell = Shell::bind(&ui, backend);
shell.start_live_updates(); // calls Backend::pump about twice a second
ui.run()
```

Contract highlights (the doc comments in `backend.rs` are authoritative):
- queries are `&self` snapshots; commands return `Result<_, BackendError>` whose message the
  shell shows as is;
- lists arrive in display order (queue order is the core's, §13.2);
- display strings the core owns arrive formatted (term labels, Hijri dates, TP-154 cells);
- review view-models never carry key-derived values (§13.2, §19.3); blind items have no
  suggestion, fills or overlays (§13.4);
- blind items carry bubble geometry only (no fill, no state); the shell masks them again so a
  backend slip cannot leak the engine's reading;
- `apply` returns the next item in queue order; `undo` compensates the latest event and returns
  the item it re-opened;
- crops are RGBA8 (`Arc<[u8]>`) with bubble geometry in crop pixels;
- `ReadinessCheck::target` says where a readiness action leads (review lane, scan, setup,
  outcomes); setup lints use Appendix A codes (SCO-011 split weights, SCO-013 unmapped item,
  SCO-014 thin evidence, SCO-032 unapproved mapping);
- item-analysis rows may arrive in any order: the shell lists flagged items first (worst flag,
  stable before "proposed", then question).

## Accessibility and agent operability (DESIGN §8, §10; ADR-001 S7)

Every interactive control is one AT node with a unique label; inner texts and keycaps are hidden.
Screens are instantiated only while visible, so labels stay unique per screen. The tests drive
the review flow through those labels (`find_by_accessible_label`, `invoke_accessible_default_action`,
`set_accessible_value`) and through key events, and check label uniqueness on every screen in
both themes.

Single-key shortcuts act only while the review canvas has focus and can be switched off in
Settings (WCAG 2.1.4). Every keycap shown on a button works: Ctrl+1…6 and Ctrl+, everywhere;
Ctrl+K and Ctrl+N on Exams (whose table is one tab stop: ↑/↓ move, Enter opens the drawer, Esc
closes it); Ctrl+Space and Ctrl+R on Scan; Ctrl+P on Results. A page change hands focus back
to the window's key scope (Review focuses its canvas). Slint 1.18 reports produced text, not scan codes, so physical positions
are recovered from the Arabic (101) layout (ADR-001 S4 remains open for true scan codes).

## Rules

- R1: only this crate depends on Slint. R8: `i-slint-backend-testing` is a dev-dependency
  (pinned `=1.18.1`, no features).
- R6: `#![deny(unsafe_code)]`; the only `allow(unsafe_code)` sits on the private
  `mod generated { slint::include_modules!(); }`, re-exported as `shell_slint::ui`.
- No real student data: invented names, 10-digit IDs starting with 9, masked national IDs,
  procedurally drawn scans.
