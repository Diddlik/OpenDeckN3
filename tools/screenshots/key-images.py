# Generates key images (data URLs) for the README demo profile.
import base64, io, json, math, sys
from PIL import Image, ImageDraw

N = 144
def grad(c1, c2):
    img = Image.new("RGB", (N, N))
    d = ImageDraw.Draw(img)
    for y in range(N):
        t = y / (N - 1)
        d.line([(0, y), (N, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(c1, c2)))
    return img

def url(img):
    b = io.BytesIO(); img.save(b, "PNG")
    return "data:image/png;base64," + base64.b64encode(b.getvalue()).decode()

W = (255, 255, 255)
def gamepad():
    img = grad((124, 58, 237), (49, 46, 129)); d = ImageDraw.Draw(img)
    d.rounded_rectangle([28, 50, 116, 98], 24, outline=W, width=7)
    d.line([(48, 74), (64, 74)], fill=W, width=7); d.line([(56, 66), (56, 82)], fill=W, width=7)
    d.ellipse([84, 62, 94, 72], fill=W); d.ellipse([96, 74, 106, 84], fill=W)
    return img
def briefcase():
    img = grad((14, 165, 233), (12, 74, 110)); d = ImageDraw.Draw(img)
    d.rounded_rectangle([30, 52, 114, 104], 10, outline=W, width=7)
    d.rounded_rectangle([56, 38, 88, 54], 6, outline=W, width=6)
    d.line([(30, 74), (114, 74)], fill=W, width=6)
    return img
def sun():
    img = grad((251, 191, 36), (194, 65, 12)); d = ImageDraw.Draw(img)
    d.ellipse([52, 52, 92, 92], outline=W, width=7)
    for i in range(8):
        a = i * math.pi / 4
        d.line([(72 + 30 * math.cos(a), 72 + 30 * math.sin(a)), (72 + 44 * math.cos(a), 72 + 44 * math.sin(a))], fill=W, width=7)
    return img
def broadcast():
    img = grad((244, 63, 94), (136, 19, 55)); d = ImageDraw.Draw(img)
    d.ellipse([62, 62, 82, 82], fill=W)
    for r in (26, 44):
        d.arc([72 - r, 72 - r, 72 + r, 72 + r], 200, 340, fill=W, width=7)
        d.arc([72 - r, 72 - r, 72 + r, 72 + r], 20, 160, fill=W, width=7)
    return img
json.dump({"gaming": url(gamepad()), "office": url(briefcase()), "sun": url(sun()), "live": url(broadcast())}, sys.stdout)
