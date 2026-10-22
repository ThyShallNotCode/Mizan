"""Copy two synthetic app screenshots into docs/materials/assets/screens and mask the course
name in the breadcrumb ("CS 211 Data Structures" -> "Sample course"). The rest of the
breadcrumb (separators, exam title, screen name, and the key pill on Results) is shifted left
so the spacing stays as the app draws it."""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

REPO = Path(__file__).resolve().parents[4]
SRC = REPO / "docs/design/screens/real"
DST = REPO / "docs/materials/assets/screens"
FONT = REPO / "docs/materials/assets/fonts/IBMPlexSans-Regular.ttf"
SCRATCH = Path(__file__).parent

INK = (85, 95, 109)
Y0, Y1 = 12, 44  # breadcrumb band (the top bar is 0..55)
OLD_X0, OLD_X1 = 97, 231  # ink extent of "CS 211 Data Structures"
CAP_TOP = 24  # first ink row of the capitals


def ink_width(font, text):
    l, t, r, b = font.getbbox(text)
    return r - l


def fit_font(target_px):
    best = None
    for tenth in range(110, 160):
        f = ImageFont.truetype(str(FONT), tenth / 10)
        w = ink_width(f, "CS 211 Data Structures")
        if best is None or abs(w - target_px) < abs(best[1] - target_px):
            best = (f, w)
    return best[0]


def right_extent(im, x_from, x_to, bg):
    """Last column in [x_from, x_to) whose band differs from the bar background."""
    last = x_from
    for x in range(x_from, x_to):
        if any(sum(abs(a - b) for a, b in zip(im.getpixel((x, y)), bg)) > 12 for y in range(Y0, Y1)):
            last = x
    return last


def mask(name, shift_to):
    im = Image.open(SRC / name).convert("RGB")
    bg = im.getpixel((90, 28))
    font = fit_font(OLD_X1 - OLD_X0 + 1)
    text = "Sample course"
    l, t, r, b = font.getbbox(text)
    cap_t = font.getbbox("S")[1]
    new_end = OLD_X0 + (r - l) - 1
    shift = OLD_X1 - new_end
    # Move the rest of the breadcrumb left by `shift`.
    tail = im.crop((236, Y0, shift_to + 1, Y1))
    draw = ImageDraw.Draw(im)
    draw.rectangle((OLD_X0 - 3, Y0, shift_to + 1, Y1 - 1), fill=bg)
    im.paste(tail, (236 - shift, Y0))
    draw.text((OLD_X0 - l, CAP_TOP - cap_t), text, font=font, fill=INK)
    out = DST / ("brochure-" + name.replace("-light", ""))
    im.save(out, optimize=True)
    im.crop((80, 6, 640, 50)).resize((1680, 132), Image.LANCZOS).save(SCRATCH / f"check-{name}")
    print(name, "font", font.size, "shift", shift, "->", out)


if __name__ == "__main__":
    DST.mkdir(parents=True, exist_ok=True)
    res = Image.open(SRC / "results-light.png").convert("RGB")
    bg = res.getpixel((90, 28))
    pill_end = right_extent(res, 384, 560, bg)
    print("results pill ends at", pill_end)
    mask("results-light.png", pill_end + 1)
    mask("review-light.png", 382)
