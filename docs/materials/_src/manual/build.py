"""Build docs/materials/manual/manual.html from manual.src.html.

- <!--SVG:name--> is replaced by diagrams/name.svg from diagrams_manual.py (inline, so the page fonts apply).
- {{pg:id}} is replaced by the page number in pages.json (from toc.py), or "00" before the first pass.

    python build.py            # build
    python build.py --toc      # build, render, find chapter pages, rebuild, render, check
    python build.py --guide    # export the side-by-side test guide from manual.pdf
"""

import json
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).parent
MAT = pathlib.Path(__file__).resolve().parents[2]
SRC = HERE / "manual.src.html"
OUT = MAT / "manual" / "manual.html"
PDF = OUT.with_suffix(".pdf")
PAGES = HERE / "pages.json"
RENDER = MAT / "render.py"
GUIDE = MAT / "manual" / "side-by-side-guide.pdf"
FONTS = MAT / "assets" / "fonts"


def build() -> None:
    text = SRC.read_text(encoding="utf-8")

    # The manual's own diagram variants (diagrams_manual.py): print-sized text, no legends
    # (the captions carry them), already cropped.
    viewboxes: dict[str, str] = {}

    def svg(m: re.Match) -> str:
        name = m.group(1)
        s = (HERE / "diagrams" / f"{name}.svg").read_text(encoding="utf-8")
        s = re.sub(r"<\?xml[^>]*\?>\s*", "", s).strip()
        if name in viewboxes:
            s = re.sub(r'viewBox="[^"]*"', f'viewBox="{viewboxes[name]}"', s, count=1)
        return s

    text = re.sub(r"<!--SVG:([a-z-]+)-->", svg, text)
    pages = json.loads(PAGES.read_text()) if PAGES.exists() else {}
    text = re.sub(r"\{\{pg:([a-zA-Z0-9-]+)\}\}", lambda m: str(pages.get(m.group(1), "00")), text)
    OUT.write_text(text, encoding="utf-8", newline="\n")
    print(f"wrote {OUT} ({len(text):,} chars)")


def chapters() -> list[tuple[str, str]]:
    """(id, title) of every chapter and of every guide inside one (each listed in the contents),
    from the source's h2 and guide-title headings."""
    text = SRC.read_text(encoding="utf-8")
    out = []
    for m in re.finditer(
        r'<section class="(?:chapter|guide)[^"]*" id="([^"]+)"[^>]*>.*?'
        r'<(h2|h3 class="guide-title")[^>]*>(.*?)</h[23]>',
        text,
        re.S,
    ):
        title = re.sub(r"<[^>]+>", "", m.group(3)).strip()
        out.append((m.group(1), title))
    return out


def find_pages() -> dict[str, int]:
    import fitz

    doc = fitz.open(PDF)
    found: dict[str, int] = {}
    heads = []  # (page, text) of every large line
    for pno, page in enumerate(doc, 1):
        for block in page.get_text("dict")["blocks"]:
            for line in block.get("lines", []):
                # chapter titles print at 24pt, a guide's title at 18pt
                spans = [s for s in line["spans"] if s["size"] >= 17.5]
                if spans:
                    heads.append((pno, "".join(s["text"] for s in spans).strip()))
    for cid, title in chapters():
        key = re.sub(r"\s+", " ", title)[:18]
        for pno, txt in heads:
            if pno > 2 and re.sub(r"\s+", " ", txt).startswith(key):
                found[cid] = pno
                break
        else:
            print(f"!! chapter not found: {cid} {title!r}")
    return found


def render(preview: bool) -> None:
    args = [sys.executable, str(RENDER), str(OUT), "--dpi", "110"]
    if not preview:
        args.append("--no-preview")
    r = subprocess.run(args, capture_output=True, text=True)
    print(r.stdout.strip(), r.stderr.strip())


def export_guide() -> None:
    """The side-by-side test guide as its own PDF: its pages of manual.pdf, unchanged, with a
    note above the first page's heading saying where they come from."""
    import fitz

    pages = json.loads(PAGES.read_text())
    first = pages["side-by-side"]
    last = min(p for p in pages.values() if p > first) - 1
    if last - first + 1 > 2:
        sys.exit(f"the guide runs over {last - first + 1} pages (from page {first}); keep it to 2")
    src = fitz.open(PDF)
    doc = fitz.open()
    doc.insert_pdf(src, from_page=first - 1, to_page=last - 1, links=False)
    note = [
        f"From the Mizan user manual, Chapter 10, pages {first} and {last}. "
        "Chapter and figure references are to the manual.",
        "The manual is not an official publication of any institution.",
    ]
    page = doc[0]
    muted = tuple(int("55606E"[i:i + 2], 16) / 255 for i in (0, 2, 4))
    for k, line in enumerate(note):
        page.insert_text((45, 28 + k * 10.5), line, fontsize=8, color=muted,
                         fontname="IBMPlexSans-Regular",
                         fontfile=str(FONTS / "IBMPlexSans-Regular.ttf"))
    doc.set_metadata({
        "title": "Run a side-by-side test with Remark",
        "subject": "Mizan user manual, Chapter 10: side-by-side test guide",
        "creator": src.metadata.get("creator", ""),
        "producer": src.metadata.get("producer", ""),
    })
    doc.subset_fonts()
    doc.save(GUIDE, garbage=4, deflate=True)
    print(f"wrote {GUIDE.name}: manual pages {first} to {last}, {doc.page_count} pages")


if __name__ == "__main__":
    if "--guide" in sys.argv:
        export_guide()
        sys.exit()
    build()
    if "--toc" in sys.argv:
        render(preview=False)
        pages = find_pages()
        PAGES.write_text(json.dumps(pages, indent=1), newline="\n")
        print("pages:", pages)
        build()
        render(preview=True)
        again = find_pages()
        print("check:", "OK, contents match" if again == pages else f"MISMATCH {again}")
    elif "--render" in sys.argv:
        render(preview=True)
