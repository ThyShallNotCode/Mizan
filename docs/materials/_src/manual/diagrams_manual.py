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
           ["faint, unclear double", "or erased marks"])
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


def compare_screen():
    """A plan of the comparison screen on Results: its regions and labels, never a figure.

    Every count, booklet, student number and answer is a blank bar, so the drawing carries
    no comparison figure, real or invented. All text is 15 units or more, as in the other
    960-unit diagrams, so it prints at about 8pt.
    """
    W, H = 960, 402
    d = Diagram(
        "compare-screen", W, H, "The comparison with Remark, as a plan",
        "Plan of the comparison screen: a Restricted banner with Back to results; a row of six "
        "tiles, Booklets paired, Questions equal, To adjudicate, Totals equal, Identity and Key "
        "sheets; on the left the list Differences to adjudicate with Queue for review and the "
        "columns Q, Sheet as scanned, Readings and Status; on the right the cards Identity, "
        "totals and key sheets, and Workbooks and report with Report folder and Save report. "
        "Every count and answer is left blank.",
    )
    T = 15  # the smallest text size
    f_tile = font("IBMPlexMono-Medium.ttf", T)
    f_title = font("IBMPlexSans-SemiBold.ttf", 17)
    f_btn = font("IBMPlexSans-Medium.ttf", T)
    f_head = font("IBMPlexSans-Medium.ttf", T)
    f_body = font("IBMPlexSans-Regular.ttf", T)
    f_eye = font("IBMPlexMono-Medium.ttf", T)
    N = d.layers["nodes"]

    def fits(f, s, room, what):
        w = f.getlength(s)
        if w > room:
            ERRORS.append(f"compare-screen: {what} '{s}' {w:.0f}px > {room:.0f}")

    def rect(x, y, w, h, *, fill=PAPER, stroke=HAIR, rx=6, layer=N):
        st = f' stroke="{stroke}" stroke-width="1.2"' if stroke else ""
        layer.append(f'<rect x="{fmt(x)}" y="{fmt(y)}" width="{fmt(w)}" height="{fmt(h)}" '
                     f'rx="{rx}" fill="{fill}"{st}/>')

    def bar(x, y, w, h=8, fill=HAIR):
        rect(x, y, w, h, fill=fill, stroke=None, rx=h / 2)

    def button(x, y, w, s, *, primary=False):
        fits(f_btn, s, w - 16, "button")
        rect(x, y, w, 30, fill=ACCENT if primary else PAPER,
             stroke=ACCENT if primary else STRONG, rx=5)
        d.text("nodes", x + w / 2, y + 20, s, size=T, weight=500,
               fill=PAPER if primary else INK)

    def title(x, y, s, room):
        fits(f_title, s, room, "title")
        d.text("nodes", x, y, s, size=17, weight=600, anchor="start")

    # Banner: the screen's Restricted notice and the way back.
    rect(16, 14, 928, 40, fill=SUNKEN, stroke=STRONG)
    d.text("nodes", 32, 40, "Restricted", size=16, weight=600, anchor="start")
    bar(128, 30, 360)
    button(780, 19, 148, "Back to results")

    # The six tiles, with blank values and captions.
    tiles = ["BOOKLETS PAIRED", "QUESTIONS EQUAL", "TO ADJUDICATE", "TOTALS EQUAL",
             "IDENTITY", "KEY SHEETS"]
    tw, ty, th = 150, 64, 66
    gap = (928 - len(tiles) * tw) / (len(tiles) - 1)
    for i, s in enumerate(tiles):
        x = 16 + i * (tw + gap)
        rect(x, ty, tw, th)
        fits(f_tile, s, tw - 14, "tile")
        d.text("nodes", x + 8, ty + 23, s, size=T, weight=500, family=MONO, fill=MUTED,
               anchor="start")
        bar(x + 8, ty + 33, 40, 12)
        bar(x + 8, ty + 50, 100, 6)

    # Left: the differences, one row per question.
    lx, ly, lw = 16, 142, 608
    hy, rh, nrows = ly + 46, 42, 4
    rect(lx, ly, lw, hy + 26 + nrows * rh + 6 - ly)
    title(lx + 16, ly + 28, "Differences to adjudicate", 380)
    button(lx + lw - 16 - 152, ly + 10, 152, "Queue for review")
    N.append(f'<rect x="{lx + 1}" y="{hy}" width="{lw - 2}" height="26" fill="{SUNKEN}"/>')
    cols = [(lx + 16, "Q"), (lx + 56, "Sheet as scanned"), (lx + 226, "Readings"),
            (lx + 336, "Status · booklet · student · findings")]
    for j, (x, s) in enumerate(cols):
        room = (cols[j + 1][0] if j + 1 < len(cols) else lx + lw - 12) - x - 6
        fits(f_head, s, room, "column")
        d.text("nodes", x, hy + 18, s, size=T, weight=500, fill=MUTED, anchor="start")
    rows = [
        ((1,), (3,), "differs · ours auto"),
        ((2, 4), (2,), "differs · ours changed"),
        ((0,), (0,), "differs · ours pending review"),
        ((3,), (3,), "same reading · ours pending review"),
    ]
    for k, (marked, faint, status) in enumerate(rows):
        ry = hy + 26 + k * rh
        if k:
            N.append(f'<line x1="{lx + 12}" y1="{ry}" x2="{lx + lw - 12}" y2="{ry}" '
                     f'stroke="{HAIR}" stroke-width="1"/>')
        bar(lx + 16, ry + 17, 24)
        bar(lx + 56, ry + 17, 14)
        for b in range(5):
            cx, cy = lx + 88 + b * 24, ry + 21
            fill = STRONG if b in marked else (SUNKEN if b in faint else PAPER)
            N.append(f'<circle cx="{cx}" cy="{cy}" r="8" fill="{fill}" stroke="{MUTED}" '
                     f'stroke-width="1.1"/>')
        for t, y in (("Remark", ry + 18), ("Ours", ry + 36)):
            fits(f_body, t, 54, "reading")
            d.text("nodes", lx + 226, y, t, size=T, fill=MUTED, anchor="start")
        bar(lx + 286, ry + 9, 12, 10)
        bar(lx + 286, ry + 27, 12, 10)
        fits(f_body, status, lx + lw - 12 - (lx + 336), "status")
        d.text("nodes", lx + 336, ry + 19, status, size=T, fill=INK, anchor="start")
        bar(lx + 336, ry + 27, 150, 6)

    # Right: identity, totals and key sheets; then the workbooks and the report.
    rx_, rw = 636, 308
    rect(rx_, 142, rw, 130)
    title(rx_ + 16, 168, "Identity, totals and key sheets", rw - 32)
    for k, s in enumerate(["IDENTITY DIFFERENCES", "TOTALS", "KEY SHEETS GRADED AS STUDENTS"]):
        y = 196 + k * 28
        fits(f_eye, s, rw - 32, "section")
        d.text("nodes", rx_ + 16, y, s, size=T, weight=500, family=MONO, fill=MUTED,
               anchor="start")
        bar(rx_ + 16, y + 6, 200 - k * 30, 5)
    rect(rx_, 282, rw, 106)
    title(rx_ + 16, 308, "Workbooks and report", rw - 32)
    fits(f_body, "Remark's key differs", 140, "line")
    d.text("nodes", rx_ + 16, 332, "Remark's key differs", size=T, fill=INK, anchor="start")
    bar(rx_ + 164, 325, 90, 6)
    fits(f_body, "Report folder", 156 - 20, "field")
    rect(rx_ + 16, 348, 156, 30, fill=PAPER, stroke=STRONG, rx=4)
    d.text("nodes", rx_ + 26, 368, "Report folder", size=T, fill=MUTED, anchor="start")
    button(rx_ + rw - 16 - 116, 348, 116, "Save report", primary=True)

    # Pins for the legend under the figure, each on clear ground: 1 between the banner's bar
    # and Back to results, 2 in the first tile's empty lower-right corner, 3 to 5 beside the
    # card titles.
    pins = [(634, 34), (16 + tw - 18, ty + 44), (262, ly + 22), (922, 162), (858, 302)]
    for n, (cx, cy) in enumerate(pins, 1):
        N.append(f'<circle cx="{cx}" cy="{cy}" r="12" fill="{ACCENT}" stroke="{PAPER}" '
                 f'stroke-width="2"/>')
        d.text("nodes", cx, cy + 5, str(n), size=T, weight=500, family=MONO, fill=PAPER)

    d.boxes = []      # a plan, not a flow: the node-overlap check does not apply
    d.write()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    how_it_works()
    review_release()
    data_local()
    exam_lifecycle()
    compare_screen()
    if ERRORS:
        print("\n".join(ERRORS))
        sys.exit(1)
    print("ok", sorted(p.name for p in OUT.glob("*.svg")))


if __name__ == "__main__":
    main()
