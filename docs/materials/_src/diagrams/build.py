"""Build the Mizan brochure/manual diagrams (diagram-design skill, Mizan skin).

Writes <slug>.svg (the bare <svg>, every id prefixed with the slug) and <slug>.html
(minimal light wrapper) into docs/materials/diagrams. Text widths are checked against
the vendored IBM Plex fonts; label masks are checked against node boxes.
"""

from __future__ import annotations

import pathlib
import sys
from xml.sax.saxutils import escape

from PIL import ImageFont

REPO = pathlib.Path(__file__).resolve().parents[4]
OUT = REPO / "docs/materials/diagrams"
FONTS = REPO / "docs/materials/assets/fonts"

F_NAME = ImageFont.truetype(str(FONTS / "IBMPlexSans-SemiBold.ttf"), 16)
F_MONO = ImageFont.truetype(str(FONTS / "IBMPlexMono-Regular.ttf"), 12)
F_MONO_M = ImageFont.truetype(str(FONTS / "IBMPlexMono-Medium.ttf"), 12)
F_SANS = ImageFont.truetype(str(FONTS / "IBMPlexSans-Regular.ttf"), 12)

# Mizan brand tokens
PAPER = "#FFFDF8"
PAPER2 = "#F0EADF"
INK = "#1C2430"
MUTED = "#55606E"
SOFT = "#8C8270"
RULE = "rgba(28,36,48,0.14)"
ACCENT = "#0E5E57"
ATINT = "#DCEBE6"
LINK = "#3D5A80"
WHITE = "#FFFFFF"

SANS = "'IBM Plex Sans', 'Segoe UI', sans-serif"
MONO = "'IBM Plex Mono', Consolas, monospace"

ERRORS: list[str] = []


def hexrgb(h: str) -> tuple[int, int, int]:
    h = h.lstrip("#")
    return int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)


def blend(top: str, alpha: float, base: str) -> str:
    t, b = hexrgb(top), hexrgb(base)
    c = [round(t[i] * alpha + b[i] * (1 - alpha)) for i in range(3)]
    return "#{:02X}{:02X}{:02X}".format(*c)


def mono_w(s: str, ls: float = 0.0, medium: bool = False) -> float:
    f = F_MONO_M if medium else F_MONO
    return f.getlength(s) + len(s) * ls * 12


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
    def __init__(self, slug: str, w: int, h: int, title: str, desc: str):
        self.slug, self.w, self.h, self.title, self.desc = slug, w, h, title, desc
        self.layers: dict[str, list[str]] = {
            k: [] for k in ("zones", "zonelabels", "arrows", "labels", "nodes", "top")
        }
        self.boxes: list[tuple[str, tuple]] = []  # node boxes (painted after labels)
        self.masks: list[tuple[str, tuple]] = []

    # ---- primitives -------------------------------------------------------
    def text(self, layer, x, y, s, *, size=12, weight=None, family=SANS, fill=INK,
             anchor="middle", ls=None):
        attrs = [f'x="{fmt(x)}"', f'y="{fmt(y)}"', f'fill="{fill}"', f'font-size="{size}"']
        if weight:
            attrs.append(f'font-weight="{weight}"')
        attrs.append(f'font-family="{family}"')
        if anchor != "start":
            attrs.append(f'text-anchor="{anchor}"')
        if ls:
            attrs.append(f'letter-spacing="{ls}em"')
        self.layers[layer].append(f'<text {" ".join(attrs)}>{escape(s)}</text>')

    def zone(self, x, y, w, h, *, fill, stroke, dash=None, label=None, label_fill=ACCENT,
             label_bg=PAPER, sw=1.2):
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.layers["zones"].append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="8" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="{sw}"{d}/>'
        )
        if label:
            tw = mono_w(label, 0.12, medium=True)
            mw = up4(tw + 16)
            self.layers["zonelabels"].append(
                f'<rect x="{x + 12}" y="{y - 8}" width="{mw}" height="16" rx="2" fill="{label_bg}"/>'
            )
            self.text("zonelabels", x + 20, y + 4, label, size=12, weight=500, family=MONO,
                      fill=label_fill, anchor="start", ls=0.12)

    def node(self, x, y, w, h, kind, name, subs=(), *, align="middle", rx=6, name_dy=None,
             sub_dy=None, bg=PAPER):
        styles = {
            "step": (WHITE, INK, None),
            "focal": (ATINT, ACCENT, None),
            "store": ("rgba(28,36,48,0.05)", MUTED, None),
            "external": ("rgba(28,36,48,0.03)", "rgba(28,36,48,0.30)", None),
            "optional": ("rgba(28,36,48,0.02)", "rgba(28,36,48,0.32)", "4,3"),
        }
        fill, stroke, dash = styles[kind]
        d = f' stroke-dasharray="{dash}"' if dash else ""
        L = self.layers["nodes"]
        L.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{bg}"/>')
        L.append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="1.2"{d}/>'
        )
        self.boxes.append((name, (x, y, w, h)))
        nw = F_NAME.getlength(name)
        if nw > w - 16:
            ERRORS.append(f"{self.slug}: name '{name}' {nw:.0f}px > {w - 16}")
        for s in subs:
            sw = mono_w(s)
            if sw > w - 12:
                ERRORS.append(f"{self.slug}: sub '{s}' {sw:.0f}px > {w - 12}")
        n_lines = 1 + len(subs)
        if name_dy is None:
            # centre the block: name 16px, subs 12px on a 16px pitch, 6px gap after the name
            block = 12 + (6 + 16 * len(subs) if subs else 0)
            top = y + (h - block) / 2
            name_dy = round(top + 12 - y)
        if sub_dy is None:
            sub_dy = name_dy + 21
        if align == "middle":
            tx, anchor = x + w / 2, "middle"
        else:
            tx, anchor = x + 16, "start"
        tx = int(tx) if float(tx).is_integer() else tx
        self.text("nodes", tx, y + name_dy, name, size=16, weight=600, family=SANS,
                  fill=INK, anchor=anchor)
        for i, s in enumerate(subs):
            self.text("nodes", tx, y + sub_dy + 16 * i, s, size=12, family=MONO, fill=MUTED,
                      anchor=anchor)
        return n_lines

    def oval(self, cx, cy, w, h, name, sub=None, kind="external"):
        x, y = cx - w // 2, cy - h // 2
        fill, stroke = {
            "external": ("rgba(28,36,48,0.03)", "rgba(28,36,48,0.30)"),
            "focal": (ATINT, ACCENT),
        }[kind]
        L = self.layers["nodes"]
        L.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{h // 2}" fill="{PAPER}"/>')
        L.append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{h // 2}" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="1.2"/>'
        )
        self.boxes.append((name, (x, y, w, h)))
        if F_NAME.getlength(name) > w - 32:
            ERRORS.append(f"{self.slug}: oval name '{name}' too wide")
        if sub:
            self.text("nodes", cx, cy - 2, name, size=16, weight=600, fill=INK)
            self.text("nodes", cx, cy + 18, sub, size=12, family=MONO, fill=MUTED)
            if mono_w(sub) > w - 40:
                ERRORS.append(f"{self.slug}: oval sub '{sub}' too wide")
        else:
            self.text("nodes", cx, cy + 6, name, size=16, weight=600, fill=INK)

    def diamond(self, cx, cy, hw, hh, lines):
        pts = f"{cx},{cy - hh} {cx + hw},{cy} {cx},{cy + hh} {cx - hw},{cy}"
        L = self.layers["nodes"]
        L.append(f'<polygon points="{pts}" fill="{PAPER}"/>')
        L.append(f'<polygon points="{pts}" fill="{WHITE}" stroke="{INK}" stroke-width="1.2"/>')
        self.boxes.append(("diamond " + lines[0], (cx - hw, cy - hh, 2 * hw, 2 * hh)))
        if len(lines) == 1:
            ys = [cy + 6]
        else:
            ys = [cy - 4, cy + 16]
        for s, yy in zip(lines, ys):
            tw = F_NAME.getlength(s)
            # half-width of the diamond at the text's top and bottom
            dy = max(abs(yy - 12 - cy), abs(yy + 4 - cy))
            avail = 2 * hw * (1 - dy / hh) - 16
            if tw > avail:
                ERRORS.append(f"{self.slug}: diamond text '{s}' {tw:.0f}px > {avail:.0f}")
            self.text("nodes", cx, yy, s, size=16, weight=600, fill=INK)

    def path(self, d, *, color="muted", dash=None, marker=True, sw=1.5):
        stroke = {"muted": MUTED, "accent": ACCENT, "link": LINK}[color]
        mk = {"muted": "arrow", "accent": "arrow-accent", "link": "arrow-link"}[color]
        m = f' marker-end="url(#{self.slug}-{mk})"' if marker else ""
        ds = f' stroke-dasharray="{dash}"' if dash else ""
        self.layers["arrows"].append(
            f'<path d="{d}" fill="none" stroke="{stroke}" stroke-width="{sw}"{ds}{m}/>'
        )

    def label_h(self, cx, line_y, s, *, fill=MUTED, bg=PAPER, below=False):
        """Arrow label centred on a horizontal segment, 8px gap above (or below)."""
        tw = mono_w(s, 0.06)
        mw = up4(tw + 12)
        mx = cx - mw / 2
        my = line_y + 8 if below else line_y - 8 - 16
        mx = round(mx)
        self.layers["labels"].append(
            f'<rect x="{mx}" y="{my}" width="{mw}" height="16" rx="2" fill="{bg}"/>'
        )
        self.text("labels", cx, my + 12, s, size=12, family=MONO, fill=fill, ls=0.06)
        self.masks.append((s, (mx, my, mw, 16)))

    def label_v(self, line_x, cy, s, *, side="right", fill=MUTED, bg=PAPER):
        """Arrow label beside a vertical segment, 8px gap."""
        tw = mono_w(s, 0.06)
        mw = up4(tw + 12)
        mx = line_x + 8 if side == "right" else line_x - 8 - mw
        my = cy - 8
        self.layers["labels"].append(
            f'<rect x="{mx}" y="{my}" width="{mw}" height="16" rx="2" fill="{bg}"/>'
        )
        self.text("labels", mx + mw / 2, my + 12, s, size=12, family=MONO, fill=fill, ls=0.06)
        self.masks.append((s, (mx, my, mw, 16)))

    # ---- output -----------------------------------------------------------
    def check(self):
        for lname, m in self.masks:
            for nname, b in self.boxes:
                if overlap(m, b):
                    ERRORS.append(f"{self.slug}: label '{lname}' mask overlaps node '{nname}'")
        for i, (an, a) in enumerate(self.boxes):
            for bn, b in self.boxes[i + 1:]:
                if overlap(a, b):
                    ERRORS.append(f"{self.slug}: node '{an}' overlaps node '{bn}'")
            x, y, w, h = a
            if x < 8 or y < 8 or x + w > self.w - 8 or y + h > self.h - 8:
                ERRORS.append(f"{self.slug}: node '{an}' too close to the edge")

    def svg(self) -> str:
        s = self.slug
        defs = (
            f'<marker id="{s}-arrow" markerWidth="8" markerHeight="6" refX="7" refY="3" '
            f'orient="auto"><polygon points="0 0, 8 3, 0 6" fill="{MUTED}"/></marker>'
            f'<marker id="{s}-arrow-accent" markerWidth="8" markerHeight="6" refX="7" refY="3" '
            f'orient="auto"><polygon points="0 0, 8 3, 0 6" fill="{ACCENT}"/></marker>'
            f'<marker id="{s}-arrow-link" markerWidth="8" markerHeight="6" refX="7" refY="3" '
            f'orient="auto"><polygon points="0 0, 8 3, 0 6" fill="{LINK}"/></marker>'
        )
        body = []
        for k in ("zones", "zonelabels", "arrows", "labels", "nodes", "top"):
            if self.layers[k]:
                body.append(f"  <!-- {k} -->")
                body.extend("  " + line for line in self.layers[k])
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" '
            f'width="100%" role="img" aria-labelledby="{s}-title {s}-desc">\n'
            f'  <title id="{s}-title">{escape(self.title)}</title>\n'
            f'  <desc id="{s}-desc">{escape(self.desc)}</desc>\n'
            f"  <defs>{defs}</defs>\n"
            f'  <rect width="{self.w}" height="{self.h}" fill="{PAPER}"/>\n'
            + "\n".join(body)
            + "\n</svg>\n"
        )


HTML = """<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{title}</title>
  <link href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans:wght@400;600&display=swap" rel="stylesheet">
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    :root {{
      --paper: #FFFDF8;
      --paper-2: #F0EADF;
      --ink: #1C2430;
      --muted: #55606E;
      --soft: #8C8270;
      --rule: rgba(28,36,48,0.14);
      --accent: #0E5E57;
      --font-sans: 'IBM Plex Sans', 'Segoe UI', sans-serif;
      --font-mono: 'IBM Plex Mono', Consolas, monospace;
    }}
    body {{
      font-family: var(--font-sans);
      background: var(--paper);
      color: var(--ink);
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 2.5rem 1rem;
    }}
    .frame {{ max-width: 960px; width: 100%; }}
    .eyebrow {{
      font-family: var(--font-mono);
      font-size: 0.72rem;
      font-weight: 500;
      letter-spacing: 0.16em;
      text-transform: uppercase;
      color: var(--muted);
      margin-bottom: 0.5rem;
    }}
    h1 {{
      font-family: var(--font-sans);
      font-weight: 600;
      font-size: clamp(1.4rem, 2vw + 0.8rem, 1.9rem);
      letter-spacing: -0.01em;
      line-height: 1.2;
      color: var(--ink);
      margin-bottom: 0.4rem;
    }}
    .sub {{ color: var(--muted); font-size: 0.95rem; margin-bottom: 1.5rem; max-width: 48rem; }}
    svg {{ width: 100%; height: auto; display: block; }}
  </style>
</head>
<body>
  <div class="frame">
    <p class="eyebrow">{eyebrow}</p>
    <h1>{h1}</h1>
    <p class="sub">{sub}</p>
    {svg}  </div>
</body>
</html>
"""


def write(d: Diagram, eyebrow: str, h1: str, sub: str):
    d.check()
    svg = d.svg()
    (OUT / f"{d.slug}.svg").write_text(svg, encoding="utf-8", newline="\n")
    html = HTML.format(title=escape(h1), eyebrow=escape(eyebrow), h1=escape(h1),
                       sub=escape(sub), svg=svg.replace("\n", "\n    ").rstrip() + "\n")
    (OUT / f"{d.slug}.html").write_text(html, encoding="utf-8", newline="\n")


# ===========================================================================
# 1. how-it-works (Process): two lanes, five steps; the human pair in a zone
# ===========================================================================
def how_it_works():
    d = Diagram(
        "how-it-works", 960, 360,
        "How Mizan grades an exam",
        "Process in five steps: you scan the sheets and confirm the setup, Mizan reads every "
        "mark without seeing the answer key and scores it, you decide each doubtful mark and "
        "release the results, and Mizan then writes the reports; every decision is recorded.",
    )
    slot, nw, nh = 172, 144, 88
    nx = [88 + j * slot for j in range(5)]          # 88, 260, 432, 604, 776
    cx = [x + nw // 2 for x in nx]                  # 160, 332, 504, 676, 848
    you_y, mz_y = 96, 236                           # node tops per lane
    you_c, mz_c = you_y + nh // 2, mz_y + nh // 2   # 140, 280

    # lanes: hairlines and labels
    Z = d.layers["zones"]
    for y in (48, 216, 344):
        Z.append(f'<line x1="16" y1="{y}" x2="944" y2="{y}" stroke="{RULE}" stroke-width="1"/>')
    Z.append(f'<line x1="76" y1="48" x2="76" y2="344" stroke="{RULE}" stroke-width="1"/>')
    d.text("zones", 44, you_c + 4, "YOU", size=12, weight=500, family=MONO, fill=MUTED, ls=0.12)
    d.text("zones", 44, mz_c + 4, "MIZAN", size=12, weight=500, family=MONO, fill=MUTED, ls=0.12)

    # step number chips
    for j, c in enumerate(cx):
        Z.append(f'<circle cx="{c}" cy="24" r="12" fill="rgba(28,36,48,0.07)"/>')
        d.text("zones", c, 28, str(j + 1), size=12, weight=500, family=MONO, fill=INK)

    # the human pair: review + release
    zx, zw = nx[2] - 16, (nx[3] + nw + 16) - (nx[2] - 16)
    d.zone(zx, 64, zw, 136, fill=blend(ACCENT, 0.05, PAPER), stroke="rgba(14,94,87,0.55)",
           dash="4,4", label="YOU DECIDE · EVERY DECISION RECORDED")

    # connectors (exit right, enter top/bottom; r=8 elbows)
    d.path(f"M{nx[0] + nw},{you_c} H{cx[1] - 8} Q{cx[1]},{you_c} {cx[1]},{you_c + 8} V{mz_y}")
    d.path(f"M{nx[1] + nw},{mz_c} H{cx[2] - 8} Q{cx[2]},{mz_c} {cx[2]},{mz_c - 8} V{you_y + nh}")
    d.path(f"M{nx[2] + nw},{you_c} H{nx[3]}")
    d.path(f"M{nx[3] + nw},{you_c} H{cx[4] - 8} Q{cx[4]},{you_c} {cx[4]},{you_c + 8} V{mz_y}")

    zone_bg = blend(ACCENT, 0.05, PAPER)
    d.node(nx[0], you_y, nw, nh, "step", "Scan and set up", ["scans, key, roster", "you check details"])
    d.node(nx[1], mz_y, nw, nh, "step", "Read and score", ["reads every mark", "without the key"])
    d.node(nx[2], you_y, nw, nh, "step", "Review doubts", ["shown as scanned", "accept or override"],
           bg=zone_bg)
    d.node(nx[3], you_y, nw, nh, "step", "Release results", ["blocked while", "anything is open"],
           bg=zone_bg)
    d.node(nx[4], mz_y, nw, nh, "step", "Export reports", ["grades, analysis,", "outcome tables"])
    write(d, "How it works · Process",
          "From a stack of scans to released results",
          "Mizan does the reading and the arithmetic. Every doubtful mark, and the release "
          "itself, is left to you.")


# ===========================================================================
# 2. review-release (Flowchart): what happens to each mark, then the release gate
# ===========================================================================
def review_release():
    d = Diagram(
        "review-release", 960, 856,
        "What happens to each mark, and when results can be released",
        "Flowchart: Mizan reads each mark without the answer key; a mark it is sure of is "
        "accepted, any other mark waits in Review until a person accepts the suggestion or "
        "changes it with a reason, and every decision is recorded; release stays blocked while "
        "ERROR or BLOCKER review items are open, and when the exam is ready you release it.",
    )
    B = 480            # main spine
    bw = 216
    bx = B - bw // 2   # 372

    # section eyebrows and divider
    d.text("zones", 40, 44, "EACH MARK", size=12, weight=500, family=MONO, fill=SOFT,
           anchor="start", ls=0.12)
    d.layers["zones"].append(
        f'<line x1="40" y1="496" x2="920" y2="496" stroke="{RULE}" stroke-width="1"/>')
    d.text("zones", 40, 524, "WHOLE EXAM", size=12, weight=500, family=MONO, fill=SOFT,
           anchor="start", ls=0.12)

    # connectors
    d.path(f"M{B},96 V128")                                    # start -> D1
    d.path(f"M600,184 H664")                                   # D1 -> Accepted (sure)
    d.path(f"M{B},240 V264")                                   # D1 -> Review (not sure)
    d.path(f"M{B},352 V376")                                   # Review -> You decide
    d.path(f"M{B},480 V536")                                   # You decide -> D2
    d.path(f"M760,224 V584 Q760,592 752,592 H600")             # Accepted -> D2
    d.path(f"M360,592 H272")                                   # D2 -> Blocked
    d.path(f"M176,548 V316 Q176,308 184,308 H{bx}", dash="5,4")  # Blocked -> Review (Open)
    d.path(f"M{B},648 V688")                                   # D2 -> You release
    d.path(f"M{bx + bw},732 H664")                             # You release -> Released

    d.label_h(632, 184, "SURE")
    d.label_v(B, 252, "NOT SURE")
    d.label_h(316, 592, "BLOCKED")
    d.label_v(176, 424, "OPEN")
    d.label_v(B, 668, "READY")

    d.oval(B, 64, 256, 64, "Mizan reads a mark", "without seeing the key")
    d.diamond(B, 184, 120, 56, ["Sure of the mark?"])
    d.node(664, 144, 192, 80, "step", "Accepted", ["counted as read"])
    d.node(bx, 264, bw, 88, "store", "Waits in Review", ["faint, double, erased", "or unreadable marks"])
    d.node(bx, 376, bw, 104, "focal", "You decide",
           ["accept the suggestion,", "or change it with a reason;", "every decision recorded"])
    d.diamond(B, 592, 120, 56, ["Release ready?"])
    d.node(80, 548, 192, 88, "step", "Release blocked", ["ERROR/BLOCKER review", "items open"])
    d.node(bx, 688, bw, 88, "focal", "You release", ["press Release results…", "and confirm"])
    d.oval(760, 732, 192, 64, "Released", "exports written")

    # legend strip
    L = d.layers["top"]
    L.append(f'<line x1="40" y1="808" x2="920" y2="808" stroke="{RULE}" stroke-width="1"/>')
    y = 832
    L.append(f'<rect x="40" y="{y - 10}" width="28" height="14" rx="7" fill="rgba(28,36,48,0.03)" '
             f'stroke="rgba(28,36,48,0.30)" stroke-width="1"/>')
    d.text("top", 76, y + 2, "Start or end", size=12, fill=MUTED, anchor="start")
    L.append(f'<rect x="216" y="{y - 10}" width="28" height="14" rx="3" fill="{WHITE}" '
             f'stroke="{INK}" stroke-width="1"/>')
    d.text("top", 252, y + 2, "Step", size=12, fill=MUTED, anchor="start")
    L.append(f'<rect x="336" y="{y - 10}" width="28" height="14" rx="3" fill="rgba(28,36,48,0.05)" '
             f'stroke="{MUTED}" stroke-width="1"/>')
    d.text("top", 372, y + 2, "Waits for a person", size=12, fill=MUTED, anchor="start")
    L.append(f'<polygon points="540,{y - 11} 556,{y - 3} 540,{y + 5} 524,{y - 3}" fill="{WHITE}" '
             f'stroke="{INK}" stroke-width="1"/>')
    d.text("top", 568, y + 2, "Check", size=12, fill=MUTED, anchor="start")
    L.append(f'<rect x="660" y="{y - 10}" width="28" height="14" rx="3" fill="{ATINT}" '
             f'stroke="{ACCENT}" stroke-width="1"/>')
    d.text("top", 696, y + 2, "A person decides", size=12, fill=MUTED, anchor="start")
    write(d, "Review and release · Flowchart",
          "What happens to each mark",
          "Mizan accepts only the marks it is sure of. Everything else waits for a person, and "
          "release stays blocked while serious review items are open.")


# ===========================================================================
# 3. data-local (Architecture with a trust boundary)
# ===========================================================================
def data_local():
    d = Diagram(
        "data-local", 960, 560,
        "Where Mizan keeps your exam data",
        "Architecture: on your computer, Mizan writes each exam folder (a copy of every scanned "
        "page, the exports including the answer key, and a tamper-evident audit log of every "
        "decision) and keeps a private key folder that checks the log; Mizan makes no network "
        "calls of its own, refuses cloud sync folders and network drives for exams, and "
        "exports leave only when you copy them.",
    )
    comp = blend(ACCENT, 0.04, PAPER)
    folder = blend(INK, 0.04, comp)

    # boundary + exam folder container
    d.zone(24, 40, 608, 408, fill=comp, stroke="rgba(14,94,87,0.55)", dash="4,4",
           label="YOUR COMPUTER")
    d.zone(288, 72, 312, 240, fill=folder, stroke=MUTED, label="EXAM FOLDER · ONE PER EXAM",
           label_fill=MUTED, label_bg=comp)

    # connectors
    d.path("M216,216 H288")                                         # Mizan -> exam folder
    d.path("M352,352 V296")                                         # private -> audit log
    d.path("M600,288 H704", marker=False, dash="5,4")               # refused
    d.path("M584,196 H704", color="link")                           # you copy
    X = d.layers["arrows"]
    X.append(f'<path d="M662,282 L674,294 M674,282 L662,294" stroke="{MUTED}" stroke-width="1.6" '
             f'stroke-linecap="round"/>')

    d.label_h(252, 216, "WRITES", bg=comp)
    d.label_v(352, 332, "CHECKS", bg=comp)
    d.label_h(652, 196, "YOU COPY", fill=LINK)

    # "REFUSED" sits beside the cross, above the dashed line
    d.label_h(652, 288, "REFUSED")

    d.node(56, 172, 160, 88, "step", "Mizan", ["no network calls", "of its own"])
    d.node(304, 96, 280, 56, "step", "Scans", ["a copy of every scanned page"], align="start",
           name_dy=24, sub_dy=44)
    d.node(304, 168, 280, 56, "step", "Exports", ["results, reports, the answer key"],
           align="start", name_dy=24, sub_dy=44)
    d.node(304, 240, 280, 56, "focal", "Audit log", ["every decision, tamper-evident"],
           align="start", name_dy=24, sub_dy=44)
    d.node(288, 352, 312, 64, "store", "Private key folder", ["signing key and log checkpoints"],
           align="start", name_dy=28, sub_dy=48, bg=comp)
    d.node(704, 252, 232, 72, "optional", "Cloud or network drive", ["refused for exams*"])
    d.node(704, 160, 232, 72, "external", "Shared by you", ["grade upload, course report"])

    # notes strip
    L = d.layers["top"]
    L.append(f'<line x1="24" y1="476" x2="936" y2="476" stroke="{RULE}" stroke-width="1"/>')
    d.text("top", 24, 500, "* On Windows, so far only network paths and Dropbox-style folders "
           "are refused.", size=12, fill=MUTED, anchor="start")
    d.text("top", 24, 520, "Mizan does not encrypt these folders: keep them on an encrypted "
           "disk, and back up both.", size=12, fill=MUTED, anchor="start")
    d.text("top", 24, 540, "\u201cNo network calls of its own\u201d comes from a review of "
           "the program\u2019s code, not from a traffic test.", size=12, fill=SOFT,
           anchor="start")
    write(d, "Your data · Architecture",
          "Everything stays on your computer",
          "Scans, decisions and exports live in the exam folder. They leave only when you "
          "copy them.")


# ===========================================================================
# 4. exam-lifecycle (State machine): the status names on the Exams screen
# ===========================================================================
def exam_lifecycle():
    d = Diagram(
        "exam-lifecycle", 960, 664,
        "The life of an exam in Mizan",
        "State machine of the statuses on the Exams screen: Setup, then Scanning while pages "
        "are read; once graded the exam is In review while anything blocks release and Ready "
        "to release when nothing does; you release it, and a later change supersedes the "
        "release so it must be released again.",
    )
    zone_fill = blend(INK, 0.03, PAPER)
    C = 480
    d.zone(224, 296, 512, 144, fill=zone_fill, stroke="rgba(28,36,48,0.30)",
           label="GRADED, NOT RELEASED", label_fill=MUTED)

    # start dot
    d.layers["nodes"].append(f'<circle cx="{C}" cy="28" r="6" fill="{INK}"/>')

    # connectors
    d.path(f"M{C},34 V56")
    d.path(f"M{C},120 V176")                                   # grade exam
    d.path(f"M{C},240 V296")                                   # run ends -> zone
    d.path("M424,356 H536")                                    # cleared
    d.path("M536,380 H424", dash="5,4")                        # new blocker
    d.path("M624,400 V488", color="accent")                    # you release
    d.path(f"M536,520 H{C + 8} Q{C},520 {C},512 V440", dash="5,4")  # later change

    d.label_v(C, 148, "GRADE EXAM")
    d.label_v(C, 268, "RUN ENDS")
    d.label_h(480, 356, "CLEARED", bg=zone_fill)
    d.label_h(480, 380, "NEW BLOCKER", bg=zone_fill, below=True)
    d.label_v(624, 460, "YOU RELEASE", fill=ACCENT)
    d.label_v(C, 476, "LATER CHANGE", side="left")

    d.node(400, 56, 160, 64, "step", "Setup", ["no pages yet"], rx=8)
    d.node(400, 176, 160, 64, "step", "Scanning", ["pages being read"], rx=8)
    d.node(248, 336, 176, 64, "step", "In review", ["release is blocked"], rx=8, bg=zone_fill)
    d.node(536, 336, 176, 64, "step", "Ready to release", ["nothing blocks it"], rx=8,
           bg=zone_fill)
    d.node(536, 488, 176, 64, "focal", "Released", ["your deliberate act"], rx=8)

    # legend + note
    L = d.layers["top"]
    L.append(f'<line x1="40" y1="584" x2="920" y2="584" stroke="{RULE}" stroke-width="1"/>')
    y = 608
    L.append(f'<circle cx="46" cy="{y - 4}" r="6" fill="{INK}"/>')
    d.text("top", 60, y, "Start", size=12, fill=MUTED, anchor="start")
    L.append(f'<line x1="128" y1="{y - 4}" x2="156" y2="{y - 4}" stroke="{MUTED}" stroke-width="1.5" '
             f'marker-end="url(#exam-lifecycle-arrow)"/>')
    d.text("top", 168, y, "Usual path", size=12, fill=MUTED, anchor="start")
    L.append(f'<line x1="264" y1="{y - 4}" x2="292" y2="{y - 4}" stroke="{MUTED}" stroke-width="1.5" '
             f'stroke-dasharray="5,4" marker-end="url(#exam-lifecycle-arrow)"/>')
    d.text("top", 304, y, "After a change", size=12, fill=MUTED, anchor="start")
    L.append(f'<rect x="424" y="{y - 11}" width="28" height="14" rx="4" fill="{ATINT}" '
             f'stroke="{ACCENT}" stroke-width="1"/>')
    d.text("top", 460, y, "Your act", size=12, fill=MUTED, anchor="start")
    d.text("top", 40, y + 32, "Adding scans grades a new batch: the exam shows Scanning again, "
           "and a released exam must be released again.", size=12, fill=MUTED, anchor="start")
    write(d, "Exam status · State machine",
          "The life of an exam",
          "The five statuses are the ones the Exams screen shows. Only you can move an exam "
          "to Released.")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    how_it_works()
    review_release()
    data_local()
    exam_lifecycle()
    if ERRORS:
        print("\n".join(ERRORS))
        sys.exit(1)
    print("ok")


if __name__ == "__main__":
    main()
