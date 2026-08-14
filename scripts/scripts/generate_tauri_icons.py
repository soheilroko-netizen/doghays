#!/usr/bin/env python3
"""Generate a full Tauri icon set (multi-size .ico + pngs) without ImageMagick.

Usage:
    uv venv .venv-ico
    uv pip install --python .venv-ico pillow
    .venv-ico/bin/python scripts/generate_tauri_icons.py src-tauri/icons --bg 141C2E --accent E94560
    rm -rf .venv-ico

Design: rounded-square bg + security shield with keyhole (proxy/VPN/security apps).
"""
import argparse, os
from PIL import Image, ImageDraw

SIZES = [16, 24, 32, 48, 64, 128, 256]

def hex2rgb(h):
    h = h.lstrip('#')
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))

def draw_icon(size, bg, accent):
    img = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    m = max(1, size // 32)
    d.rounded_rectangle([m, m, size - m, size - m], radius=size // 5, fill=bg)
    cx = size / 2.0
    top = size * 0.19
    sw = size * 0.375
    bottom = size * 0.765
    shield = [
        (cx - sw / 2, top), (cx + sw / 2, top),
        (cx + sw / 2, top + size * 0.27),
        (cx, bottom), (cx - sw / 2, top + size * 0.27),
    ]
    d.polygon(shield, fill=accent, outline=accent)
    ky = top + size * 0.235
    kr = size * 0.05
    d.ellipse([cx - kr, ky - kr, cx + kr, ky + kr], fill=bg)
    d.rounded_rectangle([cx - size * 0.027, ky + size * 0.015, cx + size * 0.027, ky + size * 0.135], radius=size * 0.016, fill=bg)
    return img

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('outdir')
    ap.add_argument('--bg', default='1A1A2E')
    ap.add_argument('--accent', default='E94560')
    args = ap.parse_args()
    os.makedirs(args.outdir, exist_ok=True)
    bg = hex2rgb(args.bg)
    accent = hex2rgb(args.accent)
    base = draw_icon(256, bg, accent)
    base.save(os.path.join(args.outdir, 'icon.ico'), sizes=[(s, s) for s in SIZES])
    base.save(os.path.join(args.outdir, 'icon.png'), size=(256, 256))
    base.resize((128, 128), Image.LANCZOS).save(os.path.join(args.outdir, '128x128.png'))
    base.resize((256, 256), Image.LANCZOS).save(os.path.join(args.outdir, '128x128@2x.png'))
    base.resize((32, 32), Image.LANCZOS).save(os.path.join(args.outdir, '32x32.png'))
    print('wrote:', sorted(os.listdir(args.outdir)))

if __name__ == '__main__':
    main()
