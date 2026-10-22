"""Brochure image fixes.

1. brochure-review.png: paint the status-bar colour over "Two-person rule: off" (a feature
   that is not built), x 1122-1221 / y 881-892 of the 1440x900 image.
2. cover-hero-print.jpg: a print-weight JPEG of the cover image at its full 1792x2400.
"""
from pathlib import Path

from PIL import Image, ImageDraw

REPO = Path(__file__).resolve().parents[2] / "assets"
HERE = Path(__file__).parent

rev = REPO / "screens/brochure-review.png"
im = Image.open(rev).convert("RGB")
bg = im.getpixel((1100, 886))
assert bg == (240, 234, 223), bg
ImageDraw.Draw(im).rectangle((1116, 876, 1228, 897), fill=bg)
im.save(rev, optimize=True)
im.crop((1040, 860, 1440, 900)).resize((1600, 160), Image.LANCZOS).save(HERE / "check-review-status.png")
print("masked", rev)

src = REPO / "img/cover-hero.png"
out = REPO / "img/cover-hero-print.jpg"
Image.open(src).convert("RGB").save(out, quality=88, optimize=True, progressive=True)
print("wrote", out, Image.open(out).size, out.stat().st_size)
