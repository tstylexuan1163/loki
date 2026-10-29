"""Builds the app icon from branding/logo-white-src.webp.

The source is a rendered card on a white backdrop with a drop shadow. This
cuts the card out along its rounded rim (everything outside becomes
transparent), trims it to a square and centres it on a 1024 canvas with a thin
transparent margin, the way app icons are expected to be.

    python branding/make-icon.py branding/logo-white-src.webp branding/icon-1024.png
    pnpm tauri icon branding/icon-1024.png -o src-tauri/icons

Needs Pillow. The card geometry below was measured from the source image.
"""
import sys
from PIL import Image, ImageDraw

SRC, OUT = sys.argv[1], sys.argv[2]
# Outer edge of the card's rim in the 1254px source, and its corner radius.
LEFT, TOP, RIGHT, BOTTOM, RADIUS = 155, 139, 1083, 1061, 205
CANVAS, MARGIN = 1024, 0.04
SS = 4  # supersampling for a smooth mask edge

im = Image.open(SRC).convert("RGBA")
w, h = im.size
mask = Image.new("L", (w * SS, h * SS), 0)
ImageDraw.Draw(mask).rounded_rectangle(
    [LEFT * SS, TOP * SS, (RIGHT + 1) * SS - 1, (BOTTOM + 1) * SS - 1],
    radius=RADIUS * SS,
    fill=255,
)
mask = mask.resize((w, h), Image.LANCZOS)
im.putalpha(mask)

card = im.crop((LEFT, TOP, RIGHT + 1, BOTTOM + 1))
side = max(card.size)
square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
square.paste(card, ((side - card.width) // 2, (side - card.height) // 2), card)

body = round(CANVAS * (1 - 2 * MARGIN))
square = square.resize((body, body), Image.LANCZOS)
out = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
off = (CANVAS - body) // 2
out.paste(square, (off, off), square)
out.save(OUT)
print(f"card {card.size} -> {body}px body on {CANVAS}px canvas")
