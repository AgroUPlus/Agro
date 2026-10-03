#!/usr/bin/env python3
"""Renders the promo images used by docs/index.html and the READMEs.

Every image is a dark stage with a tinted oval and a few device frames holding real screenshots:
phones for Wanda, a desktop window for Wander. Run from the repository root:

    python3 scripts/promo/render.py [--wander-shots DIR]

Needs Pillow. The Wander screenshots are not stored in this repository; pass the folder holding
wander-home.png and wander-focus.png (the ones in Wander's README).
"""
import argparse
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

SCALE = 2  # draw at twice the size, then downsample, for clean anti-aliased edges
W, H = 1600, 1000
STAGE = (19, 18, 24)
SHOTS = Path("docs/assets/shots")
OUT = Path("docs/assets/mockups")

# Outer oval colour, inner highlight colour.
TINTS = {
    "violet": ((130, 95, 215), (200, 182, 248)),
    "magenta": ((178, 72, 160), (236, 134, 200)),
    "blue": ((52, 110, 200), (110, 170, 230)),
    "teal": ((38, 140, 120), (110, 205, 175)),
}


def s(v):
    return int(round(v * SCALE))


def stage(tint):
    outer, inner = TINTS[tint]
    img = Image.new("RGB", (s(W), s(H)), STAGE)
    d = ImageDraw.Draw(img)
    d.ellipse([s(210), s(58), s(1510), s(962)], fill=outer)
    d.ellipse([s(430), s(250), s(1250), s(960)], fill=inner)
    return img.convert("RGBA")


def rounded_mask(size, radius):
    m = Image.new("L", size, 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, size[0] - 1, size[1] - 1], radius, fill=255)
    return m


def phone(shot, width):
    """A phone frame around `shot`, `width` px wide at output scale."""
    screen_w = s(width - 24)
    src = Image.open(SHOTS / shot).convert("RGB")
    screen_h = int(screen_w * src.height / src.width)
    screen = src.resize((screen_w, screen_h), Image.LANCZOS)
    bezel = s(12)
    fw, fh = screen_w + 2 * bezel, screen_h + 2 * bezel
    frame = Image.new("RGBA", (fw, fh), (0, 0, 0, 0))
    d = ImageDraw.Draw(frame)
    d.rounded_rectangle([0, 0, fw - 1, fh - 1], s(64), fill=(28, 27, 34), outline=(70, 66, 82), width=s(3))
    frame.paste(screen, (bezel, bezel), rounded_mask(screen.size, s(52)))
    cx = fw // 2
    d.ellipse([cx - s(9), bezel + s(18), cx + s(9), bezel + s(36)], fill=(8, 8, 10))
    d.rounded_rectangle([cx - s(70), fh - bezel - s(16), cx + s(70), fh - bezel - s(10)], s(3),
                        fill=(220, 220, 228, 200))
    return frame


def window(shot, width):
    """A desktop window with traffic lights around `shot`, `width` px wide at output scale."""
    inner_w = s(width)
    src = Image.open(shot).convert("RGB")
    inner_h = int(inner_w * src.height / src.width)
    content = src.resize((inner_w, inner_h), Image.LANCZOS)
    bar = s(34)
    frame = Image.new("RGBA", (inner_w, inner_h + bar), (0, 0, 0, 0))
    d = ImageDraw.Draw(frame)
    d.rounded_rectangle([0, 0, inner_w - 1, inner_h + bar - 1], s(16), fill=(24, 23, 30),
                        outline=(70, 66, 82), width=s(2))
    frame.paste(content, (0, bar))
    frame.putalpha(Image.composite(frame.getchannel("A"), Image.new("L", frame.size, 0),
                                   rounded_mask(frame.size, s(16))))
    for i, c in enumerate([(255, 95, 87), (254, 188, 46), (40, 200, 64)]):
        x = s(20 + i * 22)
        d.ellipse([x, s(11), x + s(12), s(23)], fill=c)
    return frame


def place(canvas, device, center, angle=0):
    if angle:
        device = device.rotate(angle, Image.BICUBIC, expand=True)
    alpha = device.getchannel("A")
    shadow = Image.new("RGBA", device.size, (0, 0, 0, 0))
    shadow.putalpha(alpha.point(lambda a: a * 0.7))
    pad = s(60)
    blurred = Image.new("RGBA", (device.width + 2 * pad, device.height + 2 * pad), (0, 0, 0, 0))
    blurred.paste(shadow, (pad, pad))
    blurred = blurred.filter(ImageFilter.GaussianBlur(s(22)))
    x, y = s(center[0]) - device.width // 2, s(center[1]) - device.height // 2
    canvas.alpha_composite(blurred, (x - pad, y - pad + s(18)))
    canvas.alpha_composite(device, (x, y))


def save(canvas, name):
    out = canvas.convert("RGB").resize((W, H), Image.LANCZOS)
    out.save(OUT / name, quality=88, optimize=True, progressive=True)
    print(OUT / name)


def render(wander):
    # Agro: one account across Wander and Wanda (handoff, merged stats).
    c = stage("violet")
    place(c, window(wander / "wander-home.png", 1120), (720, 350), 0)
    place(c, phone("wanda-stats.jpg", 330), (260, 650), 12)
    place(c, phone("wanda-player.jpg", 360), (1300, 610), 0)
    save(c, "promo-agro-devices.jpg")

    # Agro: friends, jam rooms and playlists edited together.
    c = stage("magenta")
    place(c, phone("wanda-friends.jpg", 380), (390, 560), 22)
    place(c, phone("wanda-playlist-share.jpg", 380), (1210, 560), -22)
    place(c, phone("wanda-jam.jpg", 400), (800, 515), 0)
    save(c, "promo-agro-social.jpg")

    # Wanda: Navidrome, local files, YouTube Music and Deezer as one library.
    c = stage("blue")
    place(c, phone("wanda-mix.jpg", 380), (390, 560), 22)
    place(c, phone("wanda-search.jpg", 380), (1210, 560), -22)
    place(c, phone("wanda-home.jpg", 400), (800, 515), 0)
    save(c, "promo-wanda-sources.jpg")

    # Wanda: recognition, lyrics and playlist import, all on the phone.
    c = stage("teal")
    place(c, phone("wanda-import.jpg", 380), (390, 560), 22)
    place(c, phone("wanda-lyrics.jpg", 380), (1210, 560), -22)
    place(c, phone("wanda-recognize.jpg", 400), (800, 515), 0)
    save(c, "promo-wanda-discovery.jpg")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--wander-shots", type=Path, required=True)
    render(p.parse_args().wander_shots)
