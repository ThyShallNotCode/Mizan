"""Render a Mizan materials page (an HTML fragment) to an A4 PDF and PNG previews.

The .html sources are fragments: they carry their own <title> and <style> but no
<!doctype>, <html>, <head> or <body>, because the web version is published as-is.
This script wraps a fragment in a full document, prints it with headless Chrome
and rasterizes every PDF page for review.

    python docs/materials/render.py docs/materials/brochure/brochure.html
    python docs/materials/render.py docs/materials/manual/manual.html --dpi 110

Writes <name>.pdf next to the source and previews to <source dir>/_preview/<name>-pNN.png.
"""

import argparse
import pathlib
import re
import shutil
import subprocess
import sys

import fitz  # PyMuPDF

CHROMES = [
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
]


def chrome() -> str:
    for c in CHROMES:
        if pathlib.Path(c).exists():
            return c
    sys.exit("render.py: no Chrome or Edge found")


FONTS = pathlib.Path(__file__).resolve().parent / "assets" / "fonts"


def static_fonts() -> str:
    """@font-face rules for the static IBM Plex Sans files, for the PDF only.

    Google Fonts serves IBM Plex Sans as a variable font, which Chrome embeds in a PDF as
    unnamed Type 3 fonts. Declared after the page's own stylesheet, these same-named faces
    take precedence, so the PDF embeds the static TrueType files under their own names.
    The published page itself still loads its fonts from Google Fonts only.
    """
    faces = [(400, "IBMPlexSans-Regular.ttf"), (500, "IBMPlexSans-Medium.ttf"),
             (600, "IBMPlexSans-SemiBold.ttf")]
    rules = [
        f"@font-face{{font-family:'IBM Plex Sans';font-style:normal;font-weight:{w};"
        f"src:url('{(FONTS / f).as_uri()}') format('truetype');}}"
        for w, f in faces
        if (FONTS / f).exists()
    ]
    return f"<style>{''.join(rules)}</style>\n" if rules else ""


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("source", type=pathlib.Path)
    ap.add_argument("--dpi", type=int, default=96, help="preview resolution")
    ap.add_argument("--no-preview", action="store_true")
    args = ap.parse_args()

    src = args.source.resolve()
    fragment = src.read_text(encoding="utf-8")
    # Tap-to-zoom links around screenshots are for the web page; in a PDF they would become
    # links to local file paths, so the print copy drops them.
    fragment = re.sub(r'<a class="zoom" href="[^"]*">(.*?)</a>', r"\1", fragment, flags=re.S)
    # The wrapper sits next to the source so relative asset paths resolve the same way.
    wrapper = src.with_name(f".{src.stem}.print.html")
    wrapper.write_text(
        '<!doctype html>\n<html lang="en"><head><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width, initial-scale=1"></head>'
        f"<body>\n{fragment}\n{static_fonts()}</body></html>\n",
        encoding="utf-8",
    )
    pdf = src.with_suffix(".pdf")
    try:
        subprocess.run(
            [
                chrome(),
                "--headless=new",
                "--disable-gpu",
                "--no-pdf-header-footer",
                "--run-all-compositor-stages-before-draw",
                "--virtual-time-budget=15000",
                f"--print-to-pdf={pdf}",
                wrapper.as_uri(),
            ],
            check=True,
            capture_output=True,
            timeout=180,
        )
    finally:
        wrapper.unlink(missing_ok=True)

    doc = fitz.open(pdf)
    sizes = {(round(p.rect.width * 25.4 / 72), round(p.rect.height * 25.4 / 72)) for p in doc}
    print(f"{pdf.name}: {doc.page_count} pages, page size(s) mm {sorted(sizes)}")
    if args.no_preview:
        return
    out = src.parent / "_preview"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir()
    for i, page in enumerate(doc, 1):
        page.get_pixmap(dpi=args.dpi).save(out / f"{src.stem}-p{i:02d}.png")
    print(f"previews: {out} ({doc.page_count} files at {args.dpi} dpi)")


if __name__ == "__main__":
    main()
