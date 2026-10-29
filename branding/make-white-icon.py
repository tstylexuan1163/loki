"""Builds the white app icon from the original dark artwork.

    python make-white-icon.py icon-dark-1024.png icon-white-1024.png
    pnpm tauri icon branding/icon-white-1024.png -o src-tauri/icons

Needs Pillow and numpy.
"""
import sys, numpy as np
from PIL import Image
src, out = sys.argv[1], sys.argv[2]
a = np.asarray(Image.open(src).convert("RGBA")).astype(np.float32) / 255.0
rgb, alpha = a[..., :3], a[..., 3:]
r, g, b = rgb[..., 0:1], rgb[..., 1:2], rgb[..., 2:3]
mx = rgb.max(-1, keepdims=True)

# Coverage: how far a pixel stands out from the near-black card. The card and
# the dark silhouette become paper; strokes keep their weight.
t = np.clip((mx - 0.10) / (0.42 - 0.10), 0, 1)
t = t * t * (3 - 2 * t)

# Two inks, as on the light theme: brand green for the prompt, eye and cape
# lines; gold for the horns and helmet. Anything neutral (the card's rim) is a
# soft grey.
GREEN = np.array([0.059, 0.616, 0.431], np.float32)  # #0f9d6e
GOLD = np.array([0.663, 0.482, 0.173], np.float32)   # #a97b2c
RIM = np.array([0.72, 0.74, 0.73], np.float32)
wg = np.clip((g - r) / np.maximum(mx * 0.25, 1e-4), 0, 1)
wy = np.clip((r - b) / np.maximum(mx * 0.35, 1e-4), 0, 1) * (1 - wg)
wn = np.clip(1 - wg - wy, 0, 1)
ink = wg * GREEN + wy * GOLD + wn * RIM

paper = np.array([0.992, 0.993, 0.99], np.float32)
res = np.concatenate([paper * (1 - t) + ink * t, alpha], -1)
Image.fromarray((np.clip(res, 0, 1) * 255).astype(np.uint8), "RGBA").save(out)
