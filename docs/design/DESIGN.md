# OMR Correction design system

This is the visual and interaction system for the OMR Correction desktop app (Rust + Slint 1.18). It turns the design concepts in `concepts/` into one set of tokens and component rules. `crates/shell-slint/ui/theme.slint` implements them.

- **Spec sections served.** §5.2 (ADR-001 shell), §13.3 (review UI), §20.2 (RTL and Arabic), §20.3 (app accessibility), §10.6 (Saudi grade scale), §11.3 (CLO attainment display), §12.1–12.2 (item analysis flags) and Appendix A (severity, exception codes).
- **Plan of record.** A11 in `docs/IMPLEMENTATION.md` (screens render headlessly; review flow driven through accessible labels).

| Artifact | Path |
|---|---|
| Tokens and components (Slint) | `crates/shell-slint/ui/theme.slint` |
| Fonts (OFL 1.1), app icon, icon set, girih tiles | `crates/shell-slint/ui/assets/` |
| Design concepts (HTML + 1440×900 PNG) | `docs/design/concepts/` |
| Slint proof gallery and headless renders | `docs/design/slint/` (`gallery.slint`, `harness/`, `renders/`) |

---

## 1. How the concepts were produced

| Screen | Source | Files |
|---|---|---|
| Review queue | AIDesigner `generate_design` (classic, desktop) | `concepts/review-queue.{html,png}` |
| Exams dashboard | AIDesigner `generate_design` (classic, desktop) | `concepts/exams-dashboard.{html,png}` |
| Results and outcomes | Hand-authored after the AIDesigner credits ran out, already on the final tokens | `concepts/results-outcomes.{html,png}`, `results-outcomes-dark.png` |
| Scan and recognise (live batch) | Hand-authored, same reason | `concepts/scan-live.{html,png}` |
| App icon | Hand-drawn SVG (no image credits left), rendered with Chromium | `assets/icon.svg`, `assets/icon.png` (512 px) |

The account had 5 AIDesigner credits. They went as follows:
- one credit was lost to a client-side 60 s timeout before an editor session existed, so its result could not be retrieved;
- the review and dashboard concepts were retrieved from the linked editor session;
- the results generation was refused for lack of credits after the balance had reached 0.

The PNG screenshots were taken with Playwright and Chromium at 1440×900. Tailwind's Play CDN is blocked by the proxy, so it was served from a vendored copy (`tailwindcss-cdn@3.4.10`) for the screenshot only. The HTML files are unchanged generator output.

**What we kept from the concepts.**
- The deep-teal nav rail with a faint girih tile.
- Warm sand canvas with near-white cards.
- The three-pane review layout: list 300, canvas flexible, decision 340.
- Keycap hints on every action.
- Severity shapes (diamond and triangle).
- Selected-row teal bar.
- Stacked severity mini-bars and status pills in the exams table.
- The side drawer for exam detail.

**What we fixed while synthesising.**
- The review concept's suggestion card lost its answer letter, and its option group was mislabelled "Resolve identity". The specification shows the letter at display size, and the group is "What is marked?".
- Its fill chart rendered broken bars. The spec (§3.3) is a vertical bar per bubble with a threshold line and a hysteresis band.
- The ambiguous amber `#B26B00` and the gold accent were too close to each other, and too light on scan grey. The mark colours were re-derived and validated (§4).
- In the dashboard, Arabic course names were forced `text-align:left` inside LTR runs, so the Arabic-Indic digit `١` landed on the wrong side. Arabic strings are now isolated runs set in `font-arabic` (§5.3).
- The dashboard's 10 px table text and 9 px keycaps are now at least 11 px.
- The concepts used `#B26B00` warning text at 4.13:1. Warning text is now `#9A5B00` (5.34:1).
- Cards use one radius (8 px) everywhere; the concepts mixed 4 to 12 px.

---

## 2. Principles

1. **Paper first.** The scan is the evidence. Overlay colours do not change with the theme, and chrome never sits on top of a bubble. Overlays use shape as well as colour.
2. **Calm authority.** Deep teal, warm sand, ink and a sparing gold give an academic, trustworthy tone with no generic SaaS blue. Gold marks *where you are* (active nav, focus, engine suggestion); it is never decoration.
3. **Density with air.** The base is 13 px body text on a 4 px grid, with 36–44 px rows. Numbers, codes and IDs are monospaced, so columns align.
4. **Keyboard is the primary input.** Every action shows its key as a keycap, and focus is always visible as a gold ring 2 px outside the control.
5. **Never colour alone** (WCAG 1.4.1):
   - severities: circle, triangle, diamond, octagon;
   - statuses: icon and word;
   - mark states: solid, dashed, hatched, dotted;
   - charts: direct labels and a table view.
6. **Ornament is an accent.** The girih (8-point star) pattern appears only in the nav rail (≤ 7 % opacity), page-header dividers and the app mark. It never sits behind data.

---

## 3. Colour tokens

All tokens live in the `Theme` global. `Theme.dark` selects the column. "Contrast" is the WCAG 2.x ratio against the named background, computed by script (§11). Text needs at least 4.5:1, while non-text UI and graphics need at least 3:1.

### 3.1 Surfaces, lines, text

| Token | Light | Dark | Role |
|---|---|---|---|
| `canvas` | `#F5F0E6` | `#0F1716` | App background (warm sand / deep ink-teal) |
| `surface` | `#FFFDF8` | `#16211F` | Cards, panes, top bar |
| `surface-raised` | `#FFFFFF` | `#1D2A28` | Inputs, keycaps, option chips, popovers |
| `surface-sunken` | `#F0EADF` | `#121B1A` | Table headers, group headers, status bar, wells |
| `surface-hover` / `surface-pressed` | `#F3EDE2` / `#E9E1D2` | `#22312E` / `#2A3B38` | Interaction states |
| `scan-backdrop` | `#E8E2D5` | `#0B1211` | Behind the scanned crop |
| `scrim` | `#1C243066` | `#000000A6` | Modal backdrop |
| `border` | `#E3DACB` | `#2A3936` | Decorative hairlines (not a control boundary) |
| `border-strong` | `#8C8270` | `#6B7C78` | Control boundaries: inputs, secondary buttons, chips (≥ 3:1) |
| `track` | `#E8E1D3` | `#2A3936` | Progress and meter tracks |
| `ink` | `#1C2430` | `#ECE6DA` | Primary text |
| `ink-2` | `#55606E` | `#B7BFBB` | Secondary text, labels |
| `ink-3` | `#5D6672` | `#8F9C98` | Placeholders and captions (still ≥ 4.5:1) |
| `ink-disabled` | `#A39B8B` | `#5E6B68` | Disabled only (exempt from 1.4.3) |
| `ink-inverse` | `#FFFDF8` | `#0F1716` | Text on `ink` fills (selected filter chip) |

### 3.2 Brand, navigation, status

| Token | Light | Dark | Role |
|---|---|---|---|
| `primary` | `#0E5E57` | `#5CC2B1` | Deep teal: primary actions, selection, links |
| `primary-hover` / `primary-pressed` | `#0B4A45` / `#083A36` | `#7AD0C1` / `#46A999` | States |
| `on-primary` | `#FFFFFF` | `#06231F` | Text and icons on `primary` |
| `primary-soft` | `#DCEBE6` | `#173A35` | Selected rows, suggestion card, active tab |
| `accent` | `#B0832A` | `#D9AE55` | Gold marks: active-nav bar, suggestion underline, nav badge |
| `accent-ink` / `accent-soft` | `#7A5815` / `#F6EBD3` | `#E2BD6E` / `#3A2F17` | Gold as text / tint |
| `focus` | `#8A6414` | `#E2BD6E` | Focus ring on surfaces |
| `nav-focus` | `#E2BD6E` | `#E2BD6E` | Focus ring on `nav-bg` |
| `nav-bg` | `#0B3B37` | `#08201E` | Nav rail |
| `nav-fg` / `nav-fg-2` | `#FFFFFF` / `#A8C3BE` | `#FFFFFF` / `#9FB8B3` | Active / idle nav label and icon |
| `nav-active-bg` / `nav-hover-bg` | `#FFFFFF17` / `#FFFFFF0D` | same | Nav item fills |
| `success` / `success-soft` | `#2A7148` / `#E2EFE5` | `#6BCB8F` / `#18352A` | Released, attained, auto-accepted |
| `warning` / `warning-soft` | `#9A5B00` / `#FAEBD3` | `#E7A846` / `#3A2C12` | WARN, in review, within CI |
| `error` / `error-soft` | `#B3261E` / `#F7E0DC` | `#F08A7E` / `#3E1F1C` | ERROR, not attained, mis-key |
| `blocker` | `#7F1D17` | `#FFB4A8` | BLOCKER (octagon) |
| `info` / `info-soft` | `#3D5A80` / `#E2E8F0` | `#9DB4DA` / `#1E2A3D` | INFO, scanning (muted slate, not SaaS blue) |
| `on-error` | `#FFFFFF` | `#2A0B08` | Danger button text |

### 3.3 Contrast audit (computed)

| Pair | Needs | Light | Dark |
|---|---|---|---|
| ink on canvas | 4.5:1 | 13.75 PASS | 14.64 PASS |
| ink on surface | 4.5:1 | 15.37 PASS | 13.29 PASS |
| ink-2 on surface | 4.5:1 | 6.29 PASS | 8.80 PASS |
| ink-2 on surface-sunken | 4.5:1 | 5.34 PASS | 9.34 PASS |
| ink-3 on canvas | 4.5:1 | 5.12 PASS | 6.39 PASS |
| ink-3 on surface-sunken | 4.5:1 | 4.86 PASS | 6.16 PASS |
| ink-3 on surface-raised | 4.5:1 | 5.82 PASS | 5.22 PASS |
| ink on primary-soft (selected row) | 4.5:1 | 12.70 PASS | 9.98 PASS |
| primary text on surface | 4.5:1 | 7.49 PASS | 7.71 PASS |
| primary text on primary-soft | 4.5:1 | 6.19 PASS | 5.79 PASS |
| on-primary on primary | 4.5:1 | 7.62 PASS | 7.73 PASS |
| on-primary on primary-hover | 4.5:1 | 10.10 PASS | 9.16 PASS |
| on-primary on primary-pressed | 4.5:1 | 12.58 PASS | 5.83 PASS |
| ink-inverse on ink (selected chip) | 4.5:1 | 15.37 PASS | 14.64 PASS |
| nav-fg on nav-bg | 4.5:1 | 12.39 PASS | 16.99 PASS |
| nav-fg-2 on nav-bg | 4.5:1 | 6.62 PASS | 8.08 PASS |
| accent-ink on surface | 4.5:1 | 6.39 PASS | 9.23 PASS |
| accent-ink on accent-soft | 4.5:1 | 5.49 PASS | 7.34 PASS |
| success on success-soft | 4.5:1 | 4.98 PASS | 6.68 PASS |
| warning on warning-soft | 4.5:1 | 4.62 PASS | 6.51 PASS |
| error on error-soft | 4.5:1 | 5.18 PASS | 6.10 PASS |
| info on info-soft | 4.5:1 | 5.73 PASS | 6.86 PASS |
| on-error on error (danger button) | 4.5:1 | 6.54 PASS | 7.52 PASS |
| blocker on surface | 4.5:1 | 9.88 PASS | 9.71 PASS |
| border-strong on surface (control edge) | 3:1 | 3.73 PASS | 3.76 PASS |
| border-strong on canvas | 3:1 | 3.34 PASS | 4.14 PASS |
| focus on surface | 3:1 | 5.28 PASS | 9.23 PASS |
| focus on canvas | 3:1 | 4.72 PASS | 10.17 PASS |
| nav-focus on nav-bg | 3:1 | 6.93 PASS | 9.50 PASS |
| accent (nav marker) on nav-bg | 3:1 | 3.61 PASS | 8.20 PASS |
| primary (progress fill) on track | 3:1 | 5.85 PASS | 5.65 PASS |
| chart-target on surface | 3:1 | 15.37 PASS | 13.29 PASS |

`focus` (`#8A6414`) on `nav-bg` is only 2.31:1 in light mode, which is why `nav-focus` exists. The gold `accent` reaches only 3.02–3.37:1 on light surfaces, so it is never used for text. Text in gold uses `accent-ink`.

### 3.4 Data-visualisation palette

| Slot | Hue | Light | Dark |
|---|---|---|---|
| `series-1` | teal | `#08856A` | `#16A384` |
| `series-2` | ochre | `#B7810F` | `#BD871C` |
| `series-3` | plum | `#A25089` | `#B25E98` |
| `series-4` | olive | `#648828` | `#6F9436` |
| `series-5` | indigo | `#5658AB` | `#7275CC` |
| `series-6` | terracotta | `#C06143` | `#CA6A4C` |

Other chart tokens are `chart-grid` (`#ECE5D8` / `#24312F`), `chart-axis` (`#C9BFAE` / `#3A4A47`) and `chart-target` (= `ink`).

**Validation.** The palette was checked with the dataviz skill's `validate_palette.js`: Machado 2009 CVD simulation, OKLab ΔE ×100, on `surface`. The slot order is the only order of these six hues, with teal fixed in slot 1, that passes every gate in both modes.

| Check | Light | Dark |
|---|---|---|
| Lightness band | all inside 0.43–0.77 | all inside 0.48–0.67 |
| Chroma | all ≥ 0.10 | all ≥ 0.10 |
| Worst adjacent CVD ΔE | 10.1 (target ≥ 8) | 11.2 |
| Worst adjacent normal-vision ΔE | 19.5 (floor ≥ 15) | 18.4 |
| Contrast vs surface | all ≥ 3:1 | all ≥ 3:1 |
| All-pairs (scatter, small multiples) | slots 1–2 pass (CVD 10.1) | slots 1–2 pass (CVD 11.2) |

The all-pairs result sets a **series cap of 2** for scatter, bubble and small-multiple forms. A third series needs facets or folds into "Other". The worst adjacent tritan ΔE in dark mode is 5.6; it is reported but not gated, as tritanopia is rare.

**Rules.**
- Assign slots in order, never cycled.
- A single series uses `series-1` and gets no legend box. The grade distribution is one series in teal; F is drawn in `error` with a hatch and the word "Fail", because it *means* fail.
- Values and labels use ink, never the series colour.
- The target is an `ink` line with a diamond cap, labelled "Target 70%". It is never gold, because gold sits too close to `series-2`.
- Status colours are reserved and never reused as series colours.
- Every chart has a table view, which is the relief for any sub-3:1 case and for screen readers.

---

## 4. Mark overlays on the scan

Overlays do not change with the theme because they sit on paper, which is light in both themes. Each overlay ring gets a 1 px white halo (`mark-halo`) so it reads over pencil.

| State | Token | Colour | Shape | Badge | Contrast on paper `#F4F1EA` / scan grey `#E9E9E9` |
|---|---|---|---|---|---|
| Filled / selected | `mark-selected` | `#007F68` | Solid ring, 3 px | Check | 4.40 / 4.08 |
| Ambiguous (MRK-001 band, multi-candidate) | `mark-ambiguous` | `#B06C00` | Dashed ring (12 dashes) | `?` | 3.73 / 3.47 |
| Unreadable (`UNREADABLE`, never `EMPTY`) | `mark-unreadable` | `#9E1B32` | Hatched square, 2 px | `!` | 7.01 / 6.51 |
| Blank | `mark-blank` | `#6E6A62` | Dotted ring (24 dots) | `–` | 4.77 / 4.43 |

**CVD validation.** The three hued states were checked all-pairs on paper and on scan grey, since any two can be neighbours. The worst pair is deep crimson against teal: deuteranopia ΔE 8.7 (target ≥ 8), normal-vision ΔE 18.5. Even before colour, the four shapes differ in kind: closed, dashed, square and dotted.

**Behaviour.**
- `Space` toggles all overlays. `F2` switches between the original and the registered image.
- Blank overlays draw only when "show all" is on. By default, only the selected and ambiguous bubbles in the current question get overlays.
- Blind verify items (§13.4) show **no** overlays, fill bars or suggestion. The canvas shows only the crop and the zoom.
- The fill-bar chart uses the same state colours, with a `warning` threshold line and a `warning-soft` hysteresis band.

---

## 5. Typography

### 5.1 Families

All three families are vendored in `assets/fonts/` with `OFL.txt` (SIL Open Font License 1.1, © IBM Corp., Reserved Font Name "Plex"). They are embedded via `import` in `theme.slint`, so screenshots do not depend on the host.

| Token | Family | Files | Use |
|---|---|---|---|
| `font-sans` | IBM Plex Sans | Regular, Medium, SemiBold | All UI text |
| `font-arabic` | IBM Plex Sans Arabic | Regular, SemiBold | Every Arabic string: names, notes, Arabic grade letters, course titles. It includes Latin glyphs, so mixed strings shape in one face |
| `font-mono` | IBM Plex Mono | Regular, Medium | Codes (`MRK-001`), IDs, serials, hashes, keycaps, numeric table columns (tabular by construction) |

The spec (§20.2) names Noto Naskh Arabic, IBM Plex Sans Arabic and Noto Sans for UI and PDFs. The UI uses the Plex family, and reports may add Noto Naskh Arabic for long-form Arabic.

The font weights were checked in the Slint software renderer. Weight 500/600 with family "IBM Plex Sans" picks the Medium/SemiBold files through their typographic family name. An Arabic string *without* `font-family: Theme.font-arabic` falls back to a system font, so always set it.

### 5.2 Scale (desktop, dense)

| Token | Size | Weight | Use |
|---|---|---|---|
| `text-2xs` | 11 px | 500–600 | Keycaps, nav labels, small-caps tile labels (+0.6 px tracking) |
| `text-xs` | 12 px | 400–500 | Captions, chips, pills, table meta, secondary lines |
| `text-sm` | 13 px | 400 | **Body default**, table cells, buttons |
| `text-md` | 14 px | 500–600 | Large buttons, list titles (Q17) |
| `text-lg` | 16 px | 600 | Card titles |
| `text-xl` | 20 px | 600 | Page titles, option letters A–E |
| `text-2xl` | 24 px | 600 | Stat values |
| `text-3xl` | 32 px | 600 | Gauge value |
| `text-display` | 40 px | 600 | Suggested answer letter |

The weights are `weight-regular` 400, `weight-medium` 500 and `weight-semibold` 600. No text is below 11 px.

### 5.3 Arabic, bidi and digits (§20.2)

- **GA is an English UI with Arabic data.** Arabic strings are separate runs with their own `Text`, `font-arabic` and natural RTL shaping. They are not mirrored containers. In a table, the Arabic line sits under the English line, left-aligned with the column; the run itself flows RTL.
- **Arabic grade letters** (أ+ أ ب+ ب ج+ ج د+ د هـ) appear under the Latin letters in the grade chart. As RTL runs, "أ+" displays with the plus to the left, as in official Arabic documents.
- **Digits** default to Latin in the UI. Report templates choose `latn` or `arab`. IDs, course codes and paths inside Arabic text are LTR-isolated (LRI…PDI).
- **Never mirrored:** the scan canvas, bubble grids, item order and numeric axes.

---

## 6. Space, shape, elevation, motion

| Group | Tokens |
|---|---|
| Spacing (4 px grid) | `space-2xs` 2 · `space-xs` 4 · `space-sm` 8 · `space-md` 12 · `space-lg` 16 · `space-xl` 24 · `space-2xl` 32 · `space-3xl` 48 |
| Radii | `radius-xs` 3 (keycaps, tags) · `radius-sm` 4 (bars, swatches) · `radius-md` 6 (buttons, inputs) · `radius-lg` 8 (cards, tiles, option chips) · `radius-xl` 12 (drawers, dialogs) · `radius-full` (pills) |
| Elevation | `shadow-1`: blur 8, y 2, `#1C243014` / `#00000066` (cards, tiles). `shadow-2`: blur 24, y 8, `#1C24301F` / `#0000008C` (drawers, popovers). Nothing else casts a shadow |
| Layout sizes | `nav-rail-width` 72 · `top-bar-height` 56 · `status-bar-height` 28 · `list-pane-width` 300 · `decision-pane-width` 340 · `drawer-width` 360 · `control-height` 32 / `-lg` 40 · `row-height` 36 / `-compact` 28 · `target-min` 24 (WCAG 2.5.8) · icons 16 / 20 / 24 · `focus-width` 2, `focus-offset` 2 |
| Motion | `duration-fast` 120 ms (hover, press) · `duration-base` 180 ms (selection, toggles) · `duration-slow` 260 ms (progress, drawers), ease-out |

**Motion rules.**
- Nothing moves in the review canvas between items. The next item swaps in instantly (≤ 50 ms budget, §13.2).
- `Theme.reduce-motion = true` sets every duration to 0 ms. The shell sets it from the OS preference. **Headless screenshot tests must set it too**: with frozen test time, animated colours are captured mid-transition, which was observed in the first dark-mode render.

---

## 7. Iconography

- **App mark** (`assets/icon.svg`, `icon.png` 512 px).
  - A 3×3 bubble-sheet dot grid on a deep-teal rounded square.
  - The check mark is drawn *through* three grid bubbles, the "marked answers", in gold `#E2BD6E`.
  - A faint 8-point girih star sits behind the grid at 7 % and disappears at small sizes.
  - Verified legible at 16, 32 and 64 px on light and dark.
- **UI icons** (`assets/icons/*.svg`, 25 icons).
  - Drawn for this product on a 24 px grid: 1.75 px stroke, round caps and joins, single colour.
  - Applied with `colorize` in Slint.
  - Set: `exams setup scan review results outcomes export settings search sun moon check chevron-right chevron-down plus filter keyboard help defer escalate rescan undo zoom-in overlay release`.
  - Icon-only buttons always carry an accessible label and a tooltip.
- **Shape-coded glyphs** (`SeverityGlyph`, `MarkGlyph`, `StatusPill`) are vector paths in `theme.slint`, not icon files, so they scale and recolour:

| Glyph | Shape |
|---|---|
| INFO | Ring |
| WARN | Triangle |
| ERROR | Diamond |
| BLOCKER | Octagon |
| success | Ring + check |

- **Ornament.** `assets/girih-tile.svg` is a 48 px seamless 8-point-star lattice for the nav rail; `assets/girih-strip.svg` is a 32 px rule with a star, used for page-header dividers. Both are single-colour and colourised at use.

---

## 8. Components

The behaviour shared by all components:
- Interactive components are a `TouchArea` plus a `FocusScope`.
- `Enter` or `Space` activates the component, and so does `accessible-action-default`.
- Focus shows as an outside `FocusRing`.
- Accessible names carry the visible text plus state, e.g. "B, suggested" or "Review, 38 open".

| Component | Anatomy and sizes | States | Accessibility |
|---|---|---|---|
| `NavRail` | 72 px column, `nav-bg` + girih tile at `pattern-opacity` (7 % light / 5 % dark); logo 36 px; items top-aligned; Settings pinned at the bottom with a stretch spacer | — | Container |
| `NavItem` | 72×60, 24 px icon over an 11 px label; gold 3×28 bar on the left when active; gold count badge (mono 10 px, ink text) at the top right | idle `nav-fg-2`; hover `nav-hover-bg`; active `nav-active-bg` + `nav-fg` + semibold; focus `nav-focus` ring | role tab, checked = active, label "Review, 38 open" |
| `TopBar` | 56 px, `surface`, bottom hairline; `Breadcrumb` (last part semibold), lane or view chips, progress, `SearchField` (`/`), theme `IconButton` | — | — |
| `StatusBar` | 28 px, `surface-sunken`, top hairline; save `StatusPill`, audit-chain head (mono), policy notes, engine version | — | Polite announcements ≤ 1 per 5 s (4.1.3) |
| `Card` | `surface`, 1 px `border`, `radius-lg`, `shadow-1`; title 16/600 + subtitle 12 `ink-2` with 16 px padding; content padding `inset` (0 for edge-to-edge tables) | `elevated: false` for nested cards | Title exposed as text |
| `StatTile` | min 160 wide; small-caps label 11/600 `ink-2`; value 24/600 `ink`; unit 12; caption 12 tinted by tone | — | One label "Open exceptions: 147, 38 ERROR" |
| Data table (`TableRow`, `TableHeaderCell`, `TableCell`) | header 32 px `surface-sunken`, 12/500 `ink-2`; rows 44 px (36 compact), 1 px `border` divider; numeric columns mono, right-aligned; second line 12 px `ink-2` (Arabic → `font-arabic`) | hover `surface-hover`; selected `primary-soft` + 3 px `primary` left bar | Row role list-item, checked = selected. Virtualise long tables with `ListView` in the shell |
| `Chip` (filter / segment) | 28 px pill, 12/500 text + mono count; hugs content | idle `surface` + `border-strong`; hover; selected = `ink` fill + `ink-inverse` text | role button, checkable |
| `StatusPill` | 22 px pill, `tone-soft` fill, tone text 12/500, 10 px shape glyph | tones: success, warning, error, info, neutral, primary, accent | Text role with the word |
| `Tag` | 18 px, `radius-xs`, mono 11/500; neutral = sunken + border | tone variants | — |
| `Keycap` | 18 px tall, ≥ 18 wide, mono 11/500, `border-strong` + 1 px bottom shadow; `inverse` on primary fills uses `on-primary` | — | Hidden (the key is in the parent's description) |
| `Button` (`Primary`, `Secondary`, `Ghost`, `Danger`) | height 32 (`large` 40), radius 6, padding 12/12 (16/12 large); optional 16 px icon; trailing `Keycap`; label 13 (14 large); 600 on filled, 500 otherwise | hover, pressed, disabled (`surface-sunken` + `ink-disabled`), focus ring | role button, description = key hint |
| `IconButton` | 32×32, 20 px icon | hover; `active` = `primary-soft` + primary icon | Label required |
| `OptionChip` (review decision) | A–E: ≥ 52×60, letter 20/600 over keycap. Specials (Blank, Multi, Unreadable): ≥ 92×52 with `MarkGlyph` | idle `surface-raised` + `border-strong`; **suggested**: gold border + 3 px gold underline; **selected**: `primary` fill, `on-primary` text, check badge at top right | role button, checkable, "B, suggested", description = key |
| `ProgressBar` | 6 px track, rounded; fill by tone | animated width (`duration-slow`) | progress-indicator, value % |
| `MeterBar` (CLO, attainment) | 8 px bar on a track; target = 2 px `ink` line with diamond cap; optional Wilson 95% CI whisker (1 px `ink-2` with end ticks); mono value right | — | progress-indicator, value text |
| `Gauge` | 180×120 semicircle, 12 px round-capped arc on `track`; floor tick (`ink`, e.g. 40 pages/s); value 32/600 + unit 12 | — | label "Throughput", value "62 pages / s" |
| `BubbleOverlay` | Size about 1.6× bubble diameter; ring or square path + halo; badge 36 % of size, top right | four `MarkState`s (§4) | image role, "B: filled" |
| `GroupHeader` | 32 px `surface-sunken`; severity glyph, mono code, title, count `Tag` | expanded / collapsed | button, "MRK-001 Low-confidence mark, 41 items" |
| `SearchField` | 32 px, `surface-raised`, `border-strong` (primary when focused), search icon, placeholder `ink-3`, key hint `/` | focus | text-input with label = placeholder |
| `SeverityGlyph`, `MarkGlyph` | 10–14 px vector shapes (§7) | — | image role with the severity word |
| `GirihDivider` | 10 px tiled strip in `border` colour under page titles | — | Hidden |
| `FocusRing` | 2 px ring, 2 px outside the parent, radius + 4 | `ring-color` defaults to `focus` | — |

**Not in `theme.slint` yet.** These belong in the shell, built from these tokens:
- dropdowns or selects;
- tooltips;
- dialogs (`radius-xl`, `shadow-2`, `scrim`);
- toasts (bottom-left, never covering focus, 2.4.11);
- the scan canvas itself;
- virtualised tables.

---

## 9. Screen layouts

All screens are 1440×900 nominal and work down to 1280×800. The frame is:
- `NavRail` (72);
- a content column holding `TopBar` (56), the body and `StatusBar` (28);
- 24 px body padding, a 16 px gutter between cards and 12 px between tiles.

Nav order: **Exams, Setup, Scan, Review, Results, Outcomes, Export**, then Settings at the bottom. `Ctrl+1`…`Ctrl+7` jump between them; these chords do not collide with review keys (§13.3).

### 9.1 Exams dashboard (concept: `exams-dashboard.png`)

- **Top bar.**
  - Left: wordmark "OMR Correction" and the term selector ("Term 1448-1 (Fall 2026)").
  - Right: global search (`Ctrl+K`), primary **New exam**, theme toggle and user (name · role).
- **Page header.** "Exams" (20/600), with a `GirihDivider` under it and filter `Chip`s with counts: All, Setup, Scanning, In review, Ready to release, Released. A view toggle (table / cards) sits on the right.
- **Stat row.** Four `StatTile`s:
  - Sheets scanned this week;
  - Open exceptions, with an ERROR caption;
  - Median review time (s/item);
  - Throughput (pages/s).
- **Exams table** in a `Card` with `inset: 0`. The columns are:

| Column | Content |
|---|---|
| Course | Mono code + English title; Arabic title on line 2 in `font-arabic` |
| Assessment | Midterm, Final or Quiz |
| Date | Gregorian; Hijri `… AH` on line 2 |
| Forms | Mono `Tag`s A–D |
| Sheets | scanned/expected + `ProgressBar` (success when complete, warning when short) |
| Exceptions | Severity-stacked mini bar + counts with glyphs, or "None" |
| Status | `StatusPill`: Setup (neutral), Scanning (info), In review (warning), Ready (success ring), Released (success check) |
| Actions | Row actions on hover or focus: Scan, Review, Results, Export (`IconButton`s) |

- **Detail drawer.** Selecting a row (`Enter`) opens a 360 px drawer (`shadow-2`) with:
  - the course header;
  - a status call-out such as "38 ERROR block release" with primary **Continue review (38)**;
  - configuration: Answer key "v2 published" and a CLO mapping coverage ring "18/20 items";
  - logistics: roster count, sections, last scan, batch ID;
  - **Open results**.
- **Keyboard.** `↑`/`↓` move rows, `Enter` opens the drawer, `Esc` closes it, `N` starts a new exam (when the table has focus).
- **Empty state.** The girih line art, "No exams this term", and **New exam** / **Import from Remark**.

### 9.2 Exam setup: template, key and CLO mapping (no concept; spec §6, §10.3, §11.2)

- **Stepper tabs** in the top bar: 1 Template · 2 Forms and key · 3 CLO mapping · 4 Roster · 5 Policies. Each step shows a status glyph (ring = todo, check = done, triangle = lint warnings).
- **Template** (two panes).
  - Left 300: field list (ID grid, name box, MCQ blocks, page code) with lint results (`GroupHeader` per code).
  - Centre: sheet preview at fit-width, with the selected zone outlined in `primary` and 0.1 / 1 mm arrow-key nudging (2.5.7).
  - Right 340: properties (question count, options A–E, label set Latin or Arabic, forms 1–4).
- **Forms and key.** A grid with one row per master item and columns:
  - Q;
  - one column per form (A–D), showing the printed position, mono;
  - Key (option chips A–E, toggle for multi-key);
  - Points (mono, right);
  - Policy (Tag: bonus, void, drop).
  The key version `Tag` ("Key v2 · draft") goes with **Publish key…**, which needs four-eyes after release.
- **CLO mapping matrix.** A table with:
  - rows = master items (Q01…Q50);
  - columns = CLO1…CLOn;
  - each cell is a weight input (mono, right; empty = no mapping; `w` > 0 tinted `primary-soft`);
  - a **Weight** column showing Σ w for split mode (error tint if ≠ 1) and a **Note** column (free bilingual text, Arabic in `font-arabic`);
  - a status `Tag` per row: draft, proposed (gold `accent` tone), approved (success).

  Rows sit in a sticky header grid with the CLO titles truncated and the full text in a tooltip. Below it, a **coverage strip** shows one `MeterBar` per CLO for Σ e<sub>i,c</sub> share and lint chips (unmapped items, thin evidence SCO-014, Bloom gap SCO-015). Keyboard: arrows move cells; digits type the weight; `Delete` clears; `Ctrl+Enter` approves the selected rows (role-gated).
- **Roster.** An import drop zone and a table with masked national IDs (`••••••3456`, mono).

### 9.3 Scan and recognise, live batch (concept: `scan-live.png`)

- **Top bar.**
  - Breadcrumb "CHEM 101 / Midterm 1 / Batch 3 of 5" and source `Tag` (hot folder or file import);
  - device pill (model · dpi · mode);
  - **Pause** (`Ctrl+Space`) and ghost **Stop**;
  - primary **Open review · N** (`Ctrl+4`).
- **Body.** A grid of a flexible left column plus a 360 px right column.
- **Left column, row 1.** A `Gauge` card (pages/s, floor tick at the 40 pages/s spec floor, max = stretch target ×1.2) and four `StatTile`s:
  - Pages (n of expected, %);
  - Auto-accepted (%, abstention rate AR);
  - Time to first result (s; target < 3 s);
  - Remaining (ETA).
- **Left column, Pipeline card.** A stacked bar over done, in-flight and queued, plus mono stage counters: Ingest, Decode, Register, Recognise, Identify, Quarantined.
- **Left column, Sheets card.** A thumbnail grid (10 columns at 1440; at least 96 px wide). Each thumbnail is a page render with a status tag strip at the bottom: glyph + serial, or glyph + code.

| Thumbnail state | Appearance |
|---|---|
| auto | success ring-check + serial |
| review | warning triangle + code |
| identity | error diamond + `IDN-004` |
| rejected | blocker diamond + `CAP-002`, desaturated |
| reading | info ring + scan-line sweep (static when `reduce-motion`) |
| queued | 45 % opacity |

  Arrows move through the grid, `Enter` opens the sheet, and the focus ring is gold. A filter-pill row sits in the card header.
- **Right column.**
  - **Exceptions by code**: rows of glyph · mono code · bar (`ink-3` on `track`) · count, sorted by severity then count; `Enter` on a row opens the review queue filtered to that code.
  - **Batch reconciliation**: key–value table (header expected, scanned, missing serials in error, duplicates CAP-011, blank backs CAP-012) and **Rescan pull list** (`Ctrl+R`).
  - **Activity**: a mono log of the last 20 events.
- **Status bar.** "Recognising", the engine manifest, the worker count, and a determinism note (A5).

### 9.4 Review queue (concept: `review-queue.png`; Slint proof: `slint/renders/slint-review-*.png`)

- **Top bar.**
  - Breadcrumb;
  - lane `Chip`s with counts: Marks, Identity, Capture, Scoring (only lanes the role may see, §13.2);
  - "128 of 412 resolved" + `ProgressBar`;
  - `SearchField` "Roster search" (`/`);
  - theme and help (`F1` opens help for the current code).
- **Left pane, 300 px: queue.** A header "Exception queue" with a filter `IconButton`, then a `GroupHeader` per code in lane order (severity glyph, mono code, title, count). Each item row is 62 px:
  - Q number (14/600, primary when selected) and "Form B";
  - severity glyph at the right;
  - booklet serial (mono 12);
  - a 5-dot fill sparkline (dot opacity = darkness).

  The selected row gets `primary-soft` and a 3 px primary bar.
- **Centre pane (flexible): canvas** on `scan-backdrop`.
  - A centred chip row: Registered/Original (`F2`), Overlay (`Space`), Zoom 3× (`+`/`-`), Contact sheet (n) for group confirm.
  - The paper crop of the answer row, at least 620 px wide, with `shadow-2`, showing the item number and the bubbles with `BubbleOverlay`s.
  - Under the crop, two cards. **Fill by bubble** is a vertical bar per option with mono values, a `warning` threshold line and a `warning-soft` hysteresis band. **Student's other answers** is a strip of the student's fills with "typical full fill 0.79", and a sunken line carrying the AccessKit summary ("Question 17, form B. B filled 0.82, D light 0.31. Suggestion B, engine.").
  - **No key is ever shown.**
- **Right pane, 340 px: decision.**
  - Suggestion card (`primary-soft`): the letter at `text-display` in primary, "Engine suggestion", mono "Filled 0.82 · margin 0.51", code `Tag`.
  - "What is marked?" followed by `OptionChip` A–E (suggested = gold underline; current choice = filled).
  - The special chips Blank (Backspace), Multi (`Ctrl+M`) and Unreadable (`Ctrl+U`).
  - Large primary **Accept suggestion** (`Enter`), then secondary **Apply to group · 40** (`Shift+Enter`, disabled with the reason when §13.2 conditions fail).
  - Ghost rows: Defer (`Ctrl+D`), Escalate to instructor (`Ctrl+E`), Add to rescan list (`Ctrl+R`).
  - Reason select + note field, labelled "Notes are personal data (PDPL)".
  - Keyboard legend grid with keycaps and a "median 1.4 s / item" tag.
- **Identity items** reuse the frame:
  - the centre shows name, ID-box and ID-grid crops at 2×;
  - the right pane shows the top 3 candidates as option rows (keys `1`–`3`), each with per-channel agree/disagree glyphs (check-ring vs cross-diamond), masked national ID, section, seat;
  - actions: `N` not on roster, `D` duplicate.
- **Blind verify items** hide the suggestion card, fill chart and overlays; a `Tag` reads "Verify (blind)".
- **Status bar.** Save state, audit head, "Two-person rule: on", engine version.

### 9.5 Results and outcomes (concept: `results-outcomes.png`, `results-outcomes-dark.png`)

- **Top bar.**
  - Breadcrumb and segmented tabs (Grades and outcomes · Items · Integrity · Students);
  - `Tag` "Key v2 · published";
  - secondary **What-if preview** (`Ctrl+P`);
  - primary **Release results…**, which is disabled while blocked and shows a reason (e.g. "38 ERROR open").
- **Stat strip.** Six `StatTile`s: Students (N of roster), Mean (median, SD), Pass rate, Cronbach α (Feldt CI), SEM, Open exceptions.
- **Left column, 430 px.**
  - **Grade distribution** (Saudi scale): 9 bars A+…F, counts above in mono, Latin letter + Arabic letter below, F hatched in `error` with the word "Fail". The footnote reads "CUA 1444/9/1 · GPA basis 5.00", and there is a **Table** toggle.
  - **Release readiness**: a checklist (glyph + sentence + action keycap) covering open ERRORs by lane, mis-key flags, blind-verify disagreement, batch reconciliation and the two-person rule. The overall `StatusPill` is Blocked or Ready.
- **Right column.**
  - **CLO attainment**: one row per CLO, made of the mono id + short title, two `MeterBar`s, mono values and a verdict `StatusPill`. The bars are average μ (`series-1`) and threshold-proportion π (`series-2`) with a Wilson 95% CI whisker. Both share a 70 % target marker. The verdicts are Attained (success check), Not attained (error diamond), or **Within CI** (warning ±) when the CI contains T (§11.3). The legend is at the top right.
  - **Item analysis**: table columns Q, p, r<sub>ir</sub> (negative in `error`), a 5-step discrimination meter + band word, an options A–E distribution sparkline (keyed option in `series-1`), and flag `StatusPill`s: "Probable mis-key: D · SCO-001", "Top group prefers C · SCO-002", "Too easy", "Too hard or guessed". The key is hidden by default.
  - Gold note: "NCAAA TP-154 tables ready → Export".

### 9.6 Export (no concept; spec §16.3, §17)

- **Left, 300 px:** export catalogue grouped as Grades (LMS CSV, SIS CSV), Accreditation (TP-154 A.1, B.1, PLO roll-up, ABET FCAR/EAMU), Item analysis, Student reports, Audit evidence pack. Each row shows its format `Tag`s (XLSX, CSV, HTML).
- **Centre:** a live preview of the selected table, rendered with data-table rules, plus a classification banner: "Restricted" (`warning-soft`, lock glyph).
- **Right, 340 px:** options (language Arabic/English, digits `latn`/`arab`, Hijri dates on/off, include provisional), destination path (mono), and primary **Export** (`Ctrl+E` in this view). Below that, an "Export history" list with hashes (mono) and the SUPERSEDED watermark state.
- An export that would carry identities before release shows a blocking `error` call-out with the §17.4 reason.

---

## 10. Accessibility checklist (WCAG 2.2 AA, §20.3)

| Criterion | How the system meets it |
|---|---|
| 1.4.1 Use of colour | Severity, status, mark and verdict all have shape or text twins (§4, §7) |
| 1.4.3 / 1.4.11 Contrast | Computed table §3.3; controls use `border-strong` (≥ 3:1); focus ≥ 3:1 everywhere, including `nav-focus` |
| 1.4.4 / 1.4.10 Resize, reflow | Panes reflow at 200–400 % into one column (queue → canvas → decision); the scan canvas zooms instead |
| 2.1.1 / 2.1.4 Keyboard | Every action has a key; single-letter keys are active only while the review canvas has focus, with a keymap editor and an off switch |
| 2.4.7 / 2.4.11 Focus | 2 px gold ring 2 px outside; toasts and drawers never cover the focused control |
| 2.5.7 Dragging | Template zones nudge by arrows; no drag-only action |
| 2.5.8 Target size | All targets ≥ 24 × 24 (`target-min`); option chips ≥ 52 × 60 |
| 2.3.3 / motion | `reduce-motion` sets durations to 0; no auto-playing motion |
| 4.1.2 Name, role, value | Components set `accessible-role`, `-label`, `-checked`, `-value`, `-description` (key hint) |
| 4.1.3 Status | Batch progress announced politely, at most once per 5 s |

---

## 11. Verification performed

1. **`theme.slint` compiles** with Slint 1.18.1 (slint-build) and renders through the software renderer. `docs/design/slint/harness` is a stand-alone crate outside the workspace. It compiles `docs/design/slint/gallery.slint`, which imports the theme by relative path, and writes `renders/slint-review-{light,dark}.png` and `renders/slint-components-{light,dark}.png` at 1440×900. Every exported component is used in the gallery. To reproduce:

   ```sh
   cd docs/design/slint/harness
   CARGO_TARGET_DIR=../../../../target-ui-design cargo run --offline -- ../renders/slint
   ```

2. **Contrast** was checked by script for every token pair in §3.3 and every overlay colour in §4.
3. **The categorical and overlay palettes** were run through the dataviz skill's `validate_palette.js` (§3.4, §4).
4. **Concept screenshots** were taken with Playwright + Chromium at 1440×900. `results-outcomes-dark.png` shows the dark tokens.

### Slint implementation notes (for the shell owner)

- **Global and dark mode.**
  - `import { Theme, … } from "theme.slint";` and set `default-font-family: Theme.font-sans; background: Theme.canvas;` on the window.
  - From Rust: `ui.global::<Theme>().set_dark(…)` and `set_reduce_motion(…)`. Export `Theme` from `app.slint` (`export { Theme }`) so Rust can see it.
- **Fonts** are registered by the `import "assets/fonts/*.ttf"` lines at the top of `theme.slint`. Nothing is loaded at run time.
- **Layout gotcha.** A fixed-width child (`width: 620px`, a row of fixed 12 px bars) caps a layout's *maximum* width. A pane can then refuse to stretch, which leaves a gap at the window edge. Pad with `Rectangle { horizontal-stretch: 1; }` spacers instead of `alignment: center`.
- **`accessible-role` naming.** Inside a component that has a property called `text`, `accessible-role: text;` resolves to the property. The theme always writes `AccessibleRole.text`, etc.
- **No `parent` in a component root.** A component's root bindings cannot use `parent`. That is why `FocusRing` fills its parent and draws an inner rectangle.
- **SVG assets** need Slint's SVG support. The default `slint` features have it. With `EmbedForSoftwareRenderer`, they are rasterised at build time.
- **`forbid(unsafe_code)` conflicts with Slint codegen.** Slint 1.18.1 generated code expands `ItemTreeVTable_static!`, which carries `#[allow(unsafe_code)]`. Under a crate-level `#![forbid(unsafe_code)]`, rustc rejects it with E0453.
  - **Reproduced on the scaffold:** `cargo check -p shell-slint` fails today at `out/app.rs` (`ItemTreeVTable_static! (static VT for self::InnerAppWindow)`), against `crates/shell-slint/src/main.rs:2`. The same thing happened in the render harness, which therefore omits `forbid`.
  - **Fix options for the shell owner:**
    - an audited R6 exception for `shell-slint` (crate-level `#![deny(unsafe_code)]`, with the generated module included under `#[allow(unsafe_code)] mod ui { slint::include_modules!(); }`);
    - or keeping `forbid` and moving generated code into a tiny generated-UI crate with a documented exception.

  `omr-archtest` R6 currently says "no audited exceptions yet", so this needs a decision.

---

## 12. Deviations and deferrals

- **Only two of four concepts come from AIDesigner.** The account had 5 credits. One was lost to a client timeout before an editor session existed, and the generation for the third screen was refused once the balance hit zero. Results/outcomes and scan-live were hand-authored on the final tokens. The app icon was drawn by hand rather than with `generate_image`.
- **Exam setup and export have layout specs only** (§9.2, §9.6), no concept renders.
- **Overlay colours don't follow the concepts.** They were re-derived from `#B26B00` and the concept crimson to pass the CVD all-pairs gate on scan grey.
- **Not built in `theme.slint`:** dropdown, tooltip, dialog, toast and virtualised table components. The shell builds them from these tokens.
- **The mirrored Arabic UI is P3** (§20.2). Tokens are direction-neutral: logical padding and no left/right-specific colours, except the selection bar, which moves to the inline-start edge when mirroring lands.
