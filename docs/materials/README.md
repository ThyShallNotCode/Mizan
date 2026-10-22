# Mizan brochure and user manual

Material about Mizan, in English with Arabic accents, in the app's own brand (`docs/design/DESIGN.md`).
It is not an official university publication and must not carry the university's name or logo.

| File | What it is |
|---|---|
| `brochure/brochure.pdf` | 4-page A4 brochure, print-ready |
| `manual/manual.pdf` | 33-page A4 user manual, preview edition |
| `brochure/brochure.html`, `manual/manual.html` | The same documents as web pages (generated, do not edit) |
| `diagrams/` | The four diagrams, each as a standalone `.html` and an inline-ready `.svg` |
| `assets/` | Generated images (no text or logos), masked synthetic screenshots, brand marks, IBM Plex fonts |
| `_src/` | The sources: edit these, then rebuild |

## Rebuild

```sh
python docs/materials/_src/brochure/build.py      # writes brochure/brochure.html
python docs/materials/_src/manual/build.py        # writes manual/manual.html
python docs/materials/render.py docs/materials/brochure/brochure.html
python docs/materials/render.py docs/materials/manual/manual.html --dpi 110
```

`render.py` prints each page with headless Chrome to `<name>.pdf` and writes page previews to
`_preview/`. Look at every preview after a change. After the manual's chapters move, update
`_src/manual/pages.json` so the contents page numbers match the PDF.

Screenshots come from `docs/design/screens/real/` (synthetic data) through
`_src/*/mask_screens.py`. These scripts mask the operator's name and anything that carries the
institution's name or initials. Re-run `_src/brochure/fix_images.py` after the brochure's mask script.

## Before printing or sharing

- A native Arabic speaker should review the Arabic lines. None has been reviewed yet.
- Replace `[contact]` on the brochure's back page with a real contact.
- Confirm the preview wording: the desktop app has not yet run in a real window, and Windows builds are
  unsigned test builds.
- The proof figures are aggregates only. Never add figures comparing Mizan with Remark: they stay inside
  the university until the licence review closes.
