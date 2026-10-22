"""Build docs/materials/manual/manual.html from manual.src.html.

- <!--SVG:name--> is replaced by diagrams/name.svg from diagrams_manual.py (inline, so the page fonts apply).
- {{pg:id}} is replaced by the page number in pages.json (from toc.py), or "00" before the first pass.

    python build.py            # build
    python build.py --toc      # build, render, find chapter pages, rebuild, render, check
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
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT} ({len(text):,} chars)")


def chapters() -> list[tuple[str, str]]:
    """(id, title) of every chapter, from the source's h2 headings."""
    text = SRC.read_text(encoding="utf-8")
    out = []
    for m in re.finditer(r'<section class="chapter[^"]*" id="([^"]+)">.*?<h2[^>]*>(.*?)</h2>', text, re.S):
        title = re.sub(r"<[^>]+>", "", m.group(2)).strip()
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
                spans = [s for s in line["spans"] if s["size"] >= 19]
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


if __name__ == "__main__":
    build()
    if "--toc" in sys.argv:
        render(preview=False)
        pages = find_pages()
        PAGES.write_text(json.dumps(pages, indent=1))
        print("pages:", pages)
        build()
        render(preview=True)
        again = find_pages()
        print("check:", "OK, contents match" if again == pages else f"MISMATCH {again}")
    elif "--render" in sys.argv:
        render(preview=True)
