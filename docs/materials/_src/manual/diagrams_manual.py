"""The manual's diagrams: the four Mizan diagrams redrawn with print-sized text.

The shared diagrams in docs/materials/diagrams use 12/16-unit text, which prints at about 6pt
when a 960-unit diagram fills the A4 text column. These variants use 15-unit sub-labels and
18-unit names (about 8pt and 9.5pt at 178 mm), the brand palette only (no off-palette greys or
pure white), and the manual's corrected wording. Writes <slug>.svg into ./diagrams next to this
script; manual/build.py inlines them. Text widths are checked against the vendored Plex fonts.
"""

from __future__ import annotations

import pathlib
import sys
from xml.sax.saxutils import escape

from PIL import ImageFont

REPO = pathlib.Path(__file__).resolve().parents[4]
FONTS = REPO / "docs/materials/assets/fonts"
OUT = pathlib.Path(__file__).parent / "diagrams"

PAPER = "#FFFDF8"   # surface
SUNKEN = "#F0EADF"
HAIR = "#E3DACB"
STRONG = "#8C8270"
INK = "#1C2430"
MUTED = "#55606E"
ACCENT = "#0E5E57"
ATINT = "#DCEBE6"
LINK = "#3D5A80"

SANS = "'IBM Plex Sans', 'Segoe UI', sans-serif"
MONO = "'IBM Plex Mono', Consolas, monospace"

ERRORS: list[str] = []


def font(name: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONTS / name), size)


def up4(v: float) -> int:
    return int(-(-v // 4) * 4)


def fmt(v) -> str:
    v = float(v)
    return str(int(v)) if v.is_integer() else f"{v:.1f}"


def overlap(a, b) -> bool:
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    return ax < bx + bw and bx < ax + aw and ay < by + bh and by < ay + ah


class Diagram:
    def __init__(self, slug, w, h, title, desc, *, name=18, sub=15, label=15):
        self.slug, self.w, self.h, self.title, self.desc = slug, w, h, title, desc
        self.name_size, self.sub_size, self.label_size = name, sub, label
        self.f_name = font("IBMPlexSans-SemiBold.ttf", name)
        self.f_sub = font("IBMPlexMono-Regular.ttf", sub)
        self.f_label = font("IBMPlexMono-Regular.ttf", label)
        self.f_zone = font("IBMPlexMono-Medium.ttf", label)
        self.pitch = sub + 5
        self.layers = {k: [] for k in ("zones", "zonelabels", "arrows", "labels", "nodes")}
        self.boxes: list[tuple[str, tuple]] = []
        self.masks: list[tuple[str, tuple]] = []

    # ---- measuring --------------------------------------------------------
    def sub_w(self, s):
        return self.f_sub.getlength(s)

    def label_w(self, s, ls=0.06):
        return self.f_label.getlength(s) + len(s) * ls * self.label_size

    def zone_w(self, s, ls=0.1):
        return self.f_zone.getlength(s) + len(s) * ls * self.label_size

    # ---- primitives -------------------------------------------------------
    def text(self, layer, x, y, s, *, size, weight=None, family=SANS, fill=INK, anchor="middle",
             ls=None):
        a = [f'x="{fmt(x)}"', f'y="{fmt(y)}"', f'fill="{fill}"', f'font-size="{size}"']
        if weight:
            a.append(f'font-weight="{weight}"')
        a.append(f'font-family="{family}"')
        if anchor != "start":
            a.append(f'text-anchor="{anchor}"')
        if ls:
            a.append(f'letter-spacing="{ls}em"')
        self.layers[layer].append(f'<text {" ".join(a)}>{escape(s)}</text>')

    def eyebrow(self, x, y, s, *, fill=MUTED, anchor="start", ls=0.1):
        self.text("zones", x, y, s, size=self.label_size, weight=500, family=MONO, fill=fill,
                  anchor=anchor, ls=ls)

    def zone(self, x, y, w, h, *, fill, stroke, dash=None, label=None, label_fill=ACCENT,
             label_bg=PAPER):
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.layers["zones"].append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="8" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="1.2"{d}/>'
        )
        if label:
            mw = up4(self.zone_w(label) + 16)
            if mw > w - 24:
                ERRORS.append(f"{self.slug}: zone label '{label}' {mw}px > {w - 24}")
            lh = self.label_size + 4
            self.layers["zonelabels"].append(
                f'<rect x="{x + 12}" y="{y - lh // 2}" width="{mw}" height="{lh}" rx="2" '
                f'fill="{label_bg}"/>'
            )
            self.text("zonelabels", x + 20, y + round(self.label_size * 0.36), label,
                      size=self.label_size, weight=500, family=MONO, fill=label_fill,
                      anchor="start", ls=0.1)

    STYLES = {
        "step": (PAPER, INK, None),
        "focal": (ATINT, ACCENT, None),
        "store": (SUNKEN, MUTED, None),
        "quiet": (PAPER, MUTED, None),
        "external": (PAPER, STRONG, None),
        "optional": (PAPER, STRONG, "4,3"),
    }

    def node(self, x, y, w, h, kind, name, subs=(), *, align="middle", rx=6):
        fill, stroke, dash = self.STYLES[kind]
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.layers["nodes"].append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="1.2"{d}/>'
        )
        self.boxes.append((name, (x, y, w, h)))
        pad = 32 if align == "start" else 16
        nw = self.f_name.getlength(name)
        if nw > w - pad:
            ERRORS.append(f"{self.slug}: name '{name}' {nw:.0f}px > {w - pad}")
        for s in subs:
            sw = self.sub_w(s)
            if sw > w - (28 if align == "start" else 12):
                ERRORS.append(f"{self.slug}: sub '{s}' {sw:.0f}px > {w - 12}")
        cap = round(self.name_size * 0.72)
        gap = self.sub_size + 9           # name baseline to first sub baseline
        desc = round(self.sub_size * 0.25)
        block = cap + (gap + self.pitch * (len(subs) - 1) + desc if subs else 0)
        top = y + (h - block) / 2
        base = round(top + cap)
        tx, anchor = (x + w / 2, "middle") if align == "middle" else (x + 16, "start")
        self.text("nodes", tx, base, name, size=self.name_size, weight=600, fill=INK,
                  anchor=anchor)
        for i, s in enumerate(subs):
            self.text("nodes", tx, base + gap + self.pitch * i, s, size=self.sub_size,
                      family=MONO, fill=MUTED, anchor=anchor)

    def oval(self, cx, cy, w, h, name, sub, kind="external"):
        fill, stroke, _ = self.STYLES[kind]
        x, y = cx - w // 2, cy - h // 2
        self.layers["nodes"].append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{h // 2}" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="1.2"/>'
        )
        self.boxes.append((name, (x, y, w, h)))
        if self.f_name.getlength(name) > w - 32 or self.sub_w(sub) > w - 40:
            ERRORS.append(f"{self.slug}: oval '{name}' text too wide")
        self.text("nodes", cx, cy - 3, name, size=self.name_size, weight=600, fill=INK)
        self.text("nodes", cx, cy + 20, sub, size=self.sub_size, family=MONO, fill=MUTED)

    def diamond(self, cx, cy, hw, hh, s):
        pts = f"{cx},{cy - hh} {cx + hw},{cy} {cx},{cy + hh} {cx - hw},{cy}"
        self.layers["nodes"].append(
            f'<polygon points="{pts}" fill="{PAPER}" stroke="{INK}" stroke-width="1.2"/>')
        self.boxes.append(("diamond " + s, (cx - hw, cy - hh, 2 * hw, 2 * hh)))
        dy = 14
        avail = 2 * hw * (1 - dy / hh) - 16
        if self.f_name.getlength(s) > avail:
            ERRORS.append(f"{self.slug}: diamond '{s}' too wide")
        self.text("nodes", cx, cy + 6, s, size=self.name_size, weight=600, fill=INK)

    def path(self, d, *, color="muted", dash=None, marker=True):
        stroke = {"muted": MUTED, "accent": ACCENT, "link": LINK}[color]
        mk = {"muted": "arrow", "accent": "arrow-accent", "link": "arrow-link"}[color]
        m = f' marker-end="url(#{self.slug}-{mk})"' if marker else ""
        ds = f' stroke-dasharray="{dash}"' if dash else ""
        self.layers["arrows"].append(
            f'<path d="{d}" fill="none" stroke="{stroke}" stroke-width="1.5"{ds}{m}/>')

    def _label(self, mx, my, mw, s, fill, bg):
        lh = self.label_size + 4
        self.layers["labels"].append(
            f'<rect x="{fmt(mx)}" y="{fmt(my)}" width="{mw}" height="{lh}" rx="2" fill="{bg}"/>')
        self.text("labels", mx + mw / 2, my + lh / 2 + round(self.label_size * 0.36), s,
                  size=self.label_size, family=MONO, fill=fill, ls=0.06)
        self.masks.append((s, (mx, my, mw, lh)))

    def label_h(self, cx, line_y, s, *, fill=MUTED, bg=PAPER, below=False):
        mw = up4(self.label_w(s) + 12)
        lh = self.label_size + 4
        my = line_y + 6 if below else line_y - 6 - lh
        self._label(round(cx - mw / 2), my, mw, s, fill, bg)

    def label_v(self, line_x, cy, s, *, side="right", fill=MUTED, bg=PAPER):
        mw = up4(self.label_w(s) + 12)
        lh = self.label_size + 4
        mx = line_x + 8 if side == "right" else line_x - 8 - mw
        self._label(mx, cy - lh / 2, mw, s, fill, bg)

    # ---- output -----------------------------------------------------------
    def check(self):
        for ln, m in self.masks:
            for nn, b in self.boxes:
                if overlap(m, b):
                    ERRORS.append(f"{self.slug}: label '{ln}' overlaps node '{nn}'")
            x, y, w, h = m
            if x < 4 or x + w > self.w - 4:
                ERRORS.append(f"{self.slug}: label '{ln}' too close to the edge")
        for i, (an, a) in enumerate(self.boxes):
            for bn, b in self.boxes[i + 1:]:
                if overlap(a, b):
                    ERRORS.append(f"{self.slug}: node '{an}' overlaps node '{bn}'")
            x, y, w, h = a
            if x < 8 or y < 8 or x + w > self.w - 8 or y + h > self.h - 8:
                ERRORS.append(f"{self.slug}: node '{an}' too close to the edge")

    def write(self):
        self.check()
        s = self.slug
        defs = "".join(
            f'<marker id="{s}-{k}" markerWidth="8" markerHeight="6" refX="7" refY="3" '
            f'orient="auto"><polygon points="0 0, 8 3, 0 6" fill="{c}"/></marker>'
            for k, c in (("arrow", MUTED), ("arrow-accent", ACCENT), ("arrow-link", LINK))
        )
        body = []
        for k, lines in self.layers.items():
            if lines:
                body.append(f"  <!-- {k} -->")
                body.extend("  " + ln for ln in lines)
        svg = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" '
            f'width="100%" role="img" aria-labelledby="{s}-title {s}-desc">\n'
            f'  <title id="{s}-title">{escape(self.title)}</title>\n'
            f'  <desc id="{s}-desc">{escape(self.desc)}</desc>\n'
            f"  <defs>{defs}</defs>\n"
            f'  <rect width="{self.w}" height="{self.h}" fill="{PAPER}"/>\n'
            + "\n".join(body) + "\n</svg>\n"
        )
        (OUT / f"{s}.svg").write_text(svg, encoding="utf-8", newline="\n")


# ===========================================================================
def how_it_works():
    d = Diagram(
        "how-it-works", 960, 368, "How Mizan grades an exam",
        "Process in five steps: you scan the sheets and confirm the setup, Mizan reads every "
        "mark without seeing the answer key and scores it, you decide each doubtful mark and "
        "release the results, and Mizan then writes the reports; every decision is recorded.",
    )
    nw, nh, gap = 152, 104, 24
    nx = [88 + j * (nw + gap) for j in range(5)]      # 88 264 440 616 792
    cx = [x + nw // 2 for x in nx]
    you_y, mz_y = 88, 240
    you_c, mz_c = you_y + nh // 2, mz_y + nh // 2

    Z = d.layers["zones"]
    for y in (52, 224, 360):
        Z.append(f'<line x1="16" y1="{y}" x2="944" y2="{y}" stroke="{HAIR}" stroke-width="1"/>')
    Z.append(f'<line x1="76" y1="52" x2="76" y2="360" stroke="{HAIR}" stroke-width="1"/>')
    d.eyebrow(46, you_c + 5, "YOU", anchor="middle", ls=0.06)
    d.eyebrow(46, mz_c + 5, "MIZAN", anchor="middle", ls=0.06)
    for j, c in enumerate(cx):
        Z.append(f'<circle cx="{c}" cy="26" r="14" fill="{SUNKEN}"/>')
        d.text("zones", c, 31, str(j + 1), size=15, weight=500, family=MONO, fill=INK)

    d.zone(nx[2] - 12, 68, nx[3] + nw + 12 - (nx[2] - 12), 140, fill=SUNKEN,
           stroke="rgba(14,94,87,0.55)", dash="4,4", label="YOU DECIDE · ALL RECORDED")

    d.path(f"M{nx[0] + nw},{you_c} H{cx[1] - 8} Q{cx[1]},{you_c} {cx[1]},{you_c + 8} V{mz_y}")
    d.path(f"M{nx[1] + nw},{mz_c} H{cx[2] - 8} Q{cx[2]},{mz_c} {cx[2]},{mz_c - 8} V{you_y + nh}")
    d.path(f"M{nx[2] + nw},{you_c} H{nx[3]}")
    d.path(f"M{nx[3] + nw},{you_c} H{cx[4] - 8} Q{cx[4]},{you_c} {cx[4]},{you_c + 8} V{mz_y}")

    d.node(nx[0], you_y, nw, nh, "step", "Scan and set up", ["you give scans,", "key and roster"])
    d.node(nx[1], mz_y, nw, nh, "step", "Read and score", ["reads marks", "without the key"])
    d.node(nx[2], you_y, nw, nh, "step", "Review doubts", ["see the scan,", "then decide"])
    d.node(nx[3], you_y, nw, nh, "step", "Release results", ["blocked while", "items are open"])
    d.node(nx[4], mz_y, nw, nh, "step", "Export reports",
           ["grades, item", "analysis and", "outcome tables"])
    d.write()


def review_release():
    d = Diagram(
        "review-release", 960, 780,
        "What happens to each mark, and when results can be released",
        "Flowchart: Mizan reads each mark without the answer key; a mark it is sure of is "
        "accepted, any other mark waits in Review until a person accepts the suggestion or "
        "changes it with a reason, and every decision is recorded; release stays blocked while "
        "serious review items are open, and when the exam is ready you release it.",
    )
    B, bx, bw = 480, 344, 272
    d.eyebrow(40, 40, "EACH MARK")
    d.layers["zones"].append(
        f'<line x1="40" y1="512" x2="920" y2="512" stroke="{HAIR}" stroke-width="1"/>')
    d.eyebrow(40, 540, "WHOLE EXAM")

    d.path(f"M{B},92 V116")
    d.path("M610,172 H690")
    d.path(f"M{B},228 V252")
    d.path(f"M{B},348 V372")
    d.path(f"M{B},488 V540")
    d.path("M790,208 V588 Q790,596 782,596 H610")
    d.path("M350,596 H256")
    d.path(f"M200,556 V308 Q200,300 208,300 H{bx}", dash="5,4")
    d.path(f"M{B},652 V676")
    d.path(f"M{bx + bw},720 H680")

    d.label_h(650, 172, "SURE")
    d.label_v(B, 240, "NOT SURE")
    d.label_h(303, 596, "BLOCKED")
    d.label_v(200, 430, "OPEN")
    d.label_v(B, 664, "READY")

    d.oval(B, 60, 300, 64, "Mizan reads a mark", "without seeing the key")
    d.diamond(B, 172, 130, 56, "Sure of the mark?")
    d.node(690, 136, 200, 72, "step", "Accepted", ["counted as read"])
    d.node(bx, 252, bw, 96, "store", "Waits in Review",
           ["faint, double, erased", "or unreadable marks"])
    d.node(bx, 372, bw, 116, "focal", "You decide",
           ["accept the suggestion,", "or change it with a reason;", "every decision recorded"])
    d.diamond(B, 596, 130, 56, "Release ready?")
    d.node(32, 556, 224, 80, "step", "Release blocked", ["serious review items", "still open"])
    d.node(bx, 676, bw, 88, "focal", "You release", ["press Release results…", "and confirm"])
    d.oval(780, 720, 200, 64, "Released", "exports written")
    d.write()


def data_local():
    d = Diagram(
        "data-local", 960, 360, "Where Mizan keeps your exam data",
        "Architecture: on your computer, Mizan writes each exam folder (a copy of every scanned "
        "page, the exports including the answer key, and a tamper-evident audit log of every "
        "decision) and keeps a private folder with the signing key and signed checkpoints that "
        "check the log; Mizan makes no network calls of its own and refuses cloud sync folders "
        "and network drives for exams (on Windows, only some of them so far); you share the "
        "grade upload and course report yourself.",
    )
    d.zone(16, 40, 584, 304, fill=SUNKEN, stroke="rgba(14,94,87,0.55)", dash="4,4",
           label="YOUR COMPUTER")
    d.zone(288, 76, 288, 252, fill=PAPER, stroke=MUTED, label="ONE FOLDER PER EXAM",
           label_fill=MUTED, label_bg=SUNKEN)

    d.path("M208,132 H288")
    d.path("M208,284 H304")
    d.path("M576,304 H712", marker=False, dash="5,4")
    d.path("M560,208 H712", color="link")
    d.layers["arrows"].append(
        f'<path d="M650,298 L662,310 M662,298 L650,310" stroke="{MUTED}" stroke-width="1.6" '
        f'stroke-linecap="round"/>')

    d.label_h(248, 132, "WRITES", bg=SUNKEN)
    d.label_h(248, 284, "CHECKS", bg=SUNKEN)
    d.label_h(656, 208, "YOU COPY", fill=LINK)
    d.label_h(656, 304, "REFUSED")

    d.node(40, 88, 168, 88, "step", "Mizan", ["no network calls", "of its own"])
    d.node(40, 240, 168, 88, "quiet", "Private key folder", ["signing key,", "checkpoints"])
    d.node(304, 100, 256, 64, "step", "Scans", ["a copy of every scan"], align="start")
    d.node(304, 176, 256, 64, "step", "Exports", ["results, reports, the key"], align="start")
    d.node(304, 252, 256, 64, "focal", "Audit log", ["every decision, sealed"], align="start")
    d.node(712, 164, 232, 88, "external", "Shared by you", ["grade upload,", "course report"])
    d.node(712, 268, 232, 72, "optional", "Cloud or network drive", ["refused for exams*"])
    d.write()


def exam_lifecycle():
    d = Diagram(
        "exam-lifecycle", 568, 540, "The life of an exam in Mizan",
        "State machine of the statuses on the Exams screen: Setup, then Scanning while pages "
        "are read; once graded the exam is In review while anything blocks release and Ready "
        "to release when nothing does; you release it, and a later change supersedes the "
        "release so it must be released again.",
        name=18, sub=16, label=16,
    )
    C = 284
    d.zone(16, 284, 536, 128, fill=SUNKEN, stroke=STRONG, label="GRADED, NOT RELEASED",
           label_fill=MUTED)
    d.layers["nodes"].append(f'<circle cx="{C}" cy="20" r="6" fill="{INK}"/>')

    d.path(f"M{C},26 V44")
    d.path(f"M{C},108 V164")
    d.path(f"M{C},228 V284")
    d.path("M216,340 H352")
    d.path("M352,360 H216", dash="5,4")
    d.path("M444,380 V460", color="accent")
    d.path("M352,492 H132 Q124,492 124,484 V380", dash="5,4")

    d.label_v(C, 136, "GRADE EXAM")
    d.label_v(C, 256, "RUN ENDS")
    d.label_h(284, 340, "CLEARED", bg=SUNKEN)
    d.label_h(284, 360, "NEW BLOCKER", bg=SUNKEN, below=True)
    d.label_v(444, 436, "YOU RELEASE", side="left", fill=ACCENT)
    d.label_h(244, 492, "LATER CHANGE")

    d.node(196, 44, 176, 64, "step", "Setup", ["no pages yet"], rx=8)
    d.node(196, 164, 176, 64, "step", "Scanning", ["pages being read"], rx=8)
    d.node(32, 316, 184, 64, "step", "In review", ["release blocked"], rx=8)
    d.node(352, 316, 184, 64, "step", "Ready to release", ["nothing blocks it"], rx=8)
    d.node(352, 460, 184, 64, "focal", "Released", ["your decision"], rx=8)
    d.write()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    how_it_works()
    review_release()
    data_local()
    exam_lifecycle()
    if ERRORS:
        print("\n".join(ERRORS))
        sys.exit(1)
    print("ok", sorted(p.name for p in OUT.glob("*.svg")))


if __name__ == "__main__":
    main()
