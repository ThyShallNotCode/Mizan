"""Copy the synthetic app screenshots the manual uses into docs/materials/assets/screens,
masking what must not appear (legacy template ID and scheme name, invented operator and student
names and numbers, the printed header of sheet thumbnails), and cropping details."""

import pathlib
from PIL import Image, ImageDraw

SRC = pathlib.Path(__file__).resolve().parents[4] / "docs/design/screens/real"
DST = pathlib.Path(__file__).resolve().parents[2] / "assets/screens"
DST.mkdir(parents=True, exist_ok=True)

BAR = (227, 218, 203)  # hairline #E3DACB: a neutral redaction bar


def bar(d, box, fill=BAR, r=3):
    d.rounded_rectangle(box, radius=r, fill=fill)


def operator(im, d):
    """The invented operator name and initials in the top bar."""
    bar(d, (1098, 21, 1222, 36))
    fill = im.getpixel((1236, 20))  # avatar fill, inside the ring
    d.rectangle((1239, 21, 1258, 36), fill=fill)


def thumbs(im, d, region):
    """Blank the printed header band (emblem and title) of every sheet thumbnail."""
    x0, y0, x1, y1 = region
    px = im.load()

    def grey(p):
        return 228 <= p[0] <= 248 and p[0] == p[1] == p[2]

    def white(p):
        return p[0] >= 253 and p[1] >= 253 and p[2] >= 253

    done = 0
    for y in range(y0 + 1, y1):
        x = x0
        while x < x1:
            if grey(px[x, y]) and white(px[x, y - 1]):
                s = x
                while x < x1 and grey(px[x, y]) and white(px[x, y - 1]):
                    x += 1
                if x - s >= 50:
                    g = px[s + 2, y]
                    d.rectangle((s - 1, y, x, y + 10), fill=g)
                    done += 1
            x += 1
    return done


def save(im, name, crop=None):
    if crop:
        im = im.crop(crop)
    im.save(DST / name, optimize=True)
    print(name, im.size)


def load(name):
    im = Image.open(SRC / name).convert("RGB")
    return im, ImageDraw.Draw(im)


# Chapter 3: the window tour.
im, d = load("exams-light-1280x800.png")
operator(im, d)
save(im, "exams-tour.png")

# Chapter 4: the New exam card asking for a key.
im, d = load("exams-new-ask-light-1280x800.png")
operator(im, d)
bar(d, (756, 432, 827, 447))  # legacy template ID, "Sheet" row
bar(d, (756, 677, 875, 693))  # student-number scheme name, "Student ID" row
save(im, "new-exam-ask.png")
# Chapter 4: the "What is graded" panel of the same card, large enough to read in print
# (the template ID mask at y 432 falls inside; the scheme-name row is below the crop).
save(im, "new-exam-graded.png", crop=(640, 102, 1252, 534))

# Chapter 4: Advanced options (detail).
im, d = load("exams-new-ready-light-1280x800.png")
bar(d, (110, 489, 185, 504), fill=(240, 236, 228))  # legacy template ID in the field hint
bar(d, (756, 213, 827, 228))  # legacy template ID, "Sheet" row (outside the crop, masked anyway)
operator(im, d)
save(im, "new-exam-advanced.png", crop=(96, 426, 640, 690))

# Chapter 4: the Scan screen with the completion notice.
im, d = load("scan-notice-light-1280x800.png")
print("thumbnail headers masked:", thumbs(im, d, (100, 440, 940, 690)))
save(im, "scan-notice.png")
save(im, "scan-notice-bar.png", crop=(84, 692, 1280, 770))

# Chapter 5: the key-sheet summary in Setup (detail).
im, d = load("setup-keysheet-light-1280x800.png")
save(im, "setup-keysheet.png", crop=(72, 56, 1280, 602))  # ends on a row boundary

# Chapter 6: the Add scans bar (detail, above the thumbnails).
im, d = load("scan-add-light-1280x800.png")
save(im, "scan-add.png", crop=(72, 0, 1280, 140))

# Chapter 7: Review.
im, d = load("review-light-1280x800.png")
save(im, "review-mark.png")

im, d = load("review-override-light-1280x800.png")
save(im, "review-override.png", crop=(941, 66, 1280, 560))

# Chapter 8: Results.
im, d = load("results-light-1280x800.png")
save(im, "results.png")

# Chapter 9: the Export bar (detail).
im, d = load("results-export-light-1280x800.png")
save(im, "results-export.png", crop=(72, 0, 1280, 140))

# Chapter 11: CLO attainment (detail).
im, d = load("outcomes-light-1280x800.png")
save(im, "outcomes.png", crop=(72, 0, 1280, 380))

# Chapter 12: Settings (detail, name masked).
im, d = load("settings-light-1280x800.png")
bar(d, (118, 327, 238, 343))
save(im, "settings.png", crop=(72, 56, 1280, 516))  # Review card whole; keymap cut on a row boundary
