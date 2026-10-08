"""Renders the OpenDeckN3 logo (from ui/index.html) to assets/ and crates/n3-desktop/icons/.

Usage: python3 tools/make-icon.py   (needs Pillow)
"""
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 1024
S = SIZE / 30  # the logo is drawn on a 30×30 grid


def box(x, y, w, h):
    return [x * S, y * S, (x + w) * S, (y + h) * S]


def circle(cx, cy, r):
    return [(cx - r) * S, (cy - r) * S, (cx + r) * S, (cy + r) * S]


img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle(box(1, 1, 28, 28), radius=8 * S, fill="#171A1F", outline="#2E333B", width=round(S))
keys = [(5, 7, "#35E0D0"), (11, 7, "#ECEEF1"), (17, 7, "#ECEEF1"), (5, 13, "#ECEEF1"), (11, 13, "#ECEEF1"), (17, 13, "#ECEEF1")]
for x, y, color in keys:
    d.rounded_rectangle(box(x, y, 4.5, 4.5), radius=1.2 * S, fill=color)
for cx in (7.2, 13.2, 19.2):
    d.ellipse(circle(cx, 22, 1.6), fill="#5B6370")
d.ellipse(circle(24.5, 10, 2.4), outline="#35E0D0", width=round(1.4 * S))
d.ellipse(circle(24.5, 20.5, 1.6), outline="#A0A7B2", width=round(1.2 * S))

ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
root = Path(__file__).resolve().parent.parent

assets = root / "assets"
assets.mkdir(exist_ok=True)
img.resize((256, 256), Image.LANCZOS).save(assets / "icon.png")
img.save(assets / "icon.ico", sizes=ICO_SIZES)

# Icons for the desktop app (Tauri bundle + window/tray icon).
icons = root / "crates" / "n3-desktop" / "icons"
icons.mkdir(parents=True, exist_ok=True)
for name, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)]:
    img.resize((size, size), Image.LANCZOS).save(icons / name)
img.save(icons / "icon.ico", sizes=ICO_SIZES)
print("written:", assets, icons)
