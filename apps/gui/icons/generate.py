"""Draws the PC Agent icon (Iris's face on a violet tile) at every size Tauri needs.

Run from this folder with Pillow installed: `python generate.py`. Each size is drawn at 4x and
downscaled, so small icons stay smooth. The shapes follow the mascot in ui/js/mascot.js.
"""
from PIL import Image, ImageDraw

SCALE = 4


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def draw(size):
    s = size * SCALE
    u = s / 1024  # design units: a 1024 x 1024 tile
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # Tile: vertical violet gradient, rounded like a Windows 11 app icon.
    tile = Image.new("RGBA", (s, s))
    top, bottom = (132, 118, 255), (86, 70, 222)
    for y in range(s):
        ImageDraw.Draw(tile).line([(0, y), (s, y)], fill=lerp(top, bottom, y / s) + (255,))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, s - 1, s - 1], radius=224 * u, fill=255)
    img.paste(tile, (0, 0), mask)

    d = ImageDraw.Draw(img)

    def box(x0, y0, x1, y1):
        return [x0 * u, y0 * u, x1 * u, y1 * u]

    # Antenna and its warm tip.
    d.line(box(512, 318, 512, 206), fill=(255, 255, 255, 255), width=round(30 * u))
    d.ellipse(box(462, 140, 562, 240), fill=(255, 138, 101, 255))
    # Side bolts.
    d.rounded_rectangle(box(160, 470, 222, 650), radius=31 * u, fill=(214, 208, 255, 255))
    d.rounded_rectangle(box(802, 470, 864, 650), radius=31 * u, fill=(214, 208, 255, 255))
    # Head shell.
    d.rounded_rectangle(box(196, 300, 828, 820), radius=240 * u, fill=(250, 249, 255, 255))
    # Visor.
    d.rounded_rectangle(box(282, 400, 742, 700), radius=150 * u, fill=(30, 26, 77, 255))
    # Eyes.
    d.rounded_rectangle(box(388, 474, 468, 616), radius=40 * u, fill=(124, 243, 230, 255))
    d.rounded_rectangle(box(556, 474, 636, 616), radius=40 * u, fill=(124, 243, 230, 255))
    # Smile under the visor.
    d.arc(box(452, 690, 572, 780), start=20, end=160, fill=(86, 70, 222, 255), width=round(22 * u))

    return img.resize((size, size), Image.LANCZOS)


def main():
    for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png"), (512, "icon.png")]:
        draw(size).save(name)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [draw(n) for n in sizes]
    frames[-1].save("icon.ico", sizes=[(n, n) for n in sizes], append_images=frames[:-1])


if __name__ == "__main__":
    main()
