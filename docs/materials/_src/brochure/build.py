"""Assemble docs/materials/brochure/brochure.html from brochure.src.html, inlining two diagrams.

how-it-works.svg: the reports are written when grading ends (an arrow from "Read and score" to
"Export reports"), and release adds the Blackboard upload (a label on the arrow from "Release
results"); the description says the same. data-local.svg loses its three footnote lines (the
page sets them as text under the diagram, same wording), its viewBox is trimmed to the drawing,
and the private key folder holds "log checkpoints", not "check copies".
"""

import re
from pathlib import Path

HERE = Path(__file__).parent
REPO = Path(__file__).resolve().parents[4]
DIAG = REPO / "docs/materials/diagrams"
OUT = REPO / "docs/materials/brochure/brochure.html"

MONO = "font-family=\"'IBM Plex Mono', Consolas, monospace\""


def swap(text: str, old: str, new: str) -> str:
    assert text.count(old) == 1, old
    return text.replace(old, new)


def svg(name: str) -> str:
    text = (DIAG / f"{name}.svg").read_text(encoding="utf-8").strip()
    text = re.sub(r"^<\?xml[^>]*>\s*", "", text)
    if name == "how-it-works":
        text = swap(
            text,
            "Mizan reads every mark without seeing the answer key and scores it, you decide each "
            "doubtful mark and release the results, and Mizan then writes the reports; every "
            "decision is recorded.",
            "Mizan reads every mark without seeing the answer key, scores it and writes the "
            "reports when grading ends, and you decide each doubtful mark and release the "
            "results, which adds the Blackboard upload; the reports are written again whenever "
            "you export, and every decision is recorded.",
        )
        text = swap(
            text,
            '<path d="M404,280 H496 Q504,280 504,272 V184"',
            '<path d="M404,264 H496 Q504,264 504,256 V184"',
        )
        text = swap(
            text,
            "  <!-- nodes -->",
            '  <path d="M404,300 H776" fill="none" stroke="#55606E" stroke-width="1.5" '
            'marker-end="url(#how-it-works-arrow)"/>\n'
            f'  <text x="640" y="291" fill="#55606E" font-size="12" {MONO} '
            'text-anchor="middle" letter-spacing="0.06em">WHEN GRADING ENDS</text>\n'
            f'  <text x="858" y="190" fill="#55606E" font-size="12" {MONO} '
            'letter-spacing="0.06em">+ UPLOAD</text>\n'
            "  <!-- nodes -->",
        )
    if name == "data-local":
        start = text.index("  <!-- top -->")
        end = text.index("</svg>")
        text = text[:start] + text[end:]
        text = swap(text, 'viewBox="0 0 960 560"', 'viewBox="0 18 960 440"')
        if "signing key and check copies" in text:  # the shared diagram now has the fix itself
            text = swap(text, "signing key and check copies", "signing key and log checkpoints")
    return text


def main() -> None:
    src = (HERE / "brochure.src.html").read_text(encoding="utf-8")
    for name in ("how-it-works", "data-local"):
        marker = f"<!--INLINE:{name}-->"
        assert marker in src, marker
        src = src.replace(marker, svg(name))
    OUT.write_text(src, encoding="utf-8", newline="\n")
    print(f"wrote {OUT} ({len(src):,} chars)")


if __name__ == "__main__":
    main()
