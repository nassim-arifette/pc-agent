"""Draws the PC Agent icon (Nox's head, as in ui/js/brand.js) at every size Tauri needs.

Run from this folder with Pillow installed: `python generate.py`. Each size is drawn at 4x and
downscaled, so small icons stay smooth. Coordinates use the 24-unit grid of the in-app mark.
"""
from PIL import Image, ImageDraw

SCALE = 4
ACCENT_TOP, ACCENT_BOTTOM = (76, 108, 232), (47, 73, 181)


def quad(p0, p1, p2, n=24):
    return [((1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0],
             (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1])
            for t in (i / n for i in range(1, n + 1))]


# The same outline as HEAD in brand.js: ear tufts, a soft brow and a rounded chin.
HEAD = ([(3, 6.5), (6.5, 2.5), (9.5, 5.2)] + quad((9.5, 5.2), (12, 4.4), (14.5, 5.2))
        + [(17.5, 2.5), (21, 6.5), (21, 16)] + quad((21, 16), (21, 22), (12, 22))
        + quad((12, 22), (3, 22), (3, 16)))


def draw(size):
    s = size * SCALE
    u = s / 24
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    pts = [(x * u, y * u) for x, y in HEAD]

    # Head: a vertical accent gradient clipped to the outline.
    fill = Image.new("RGBA", (s, s))
    g = ImageDraw.Draw(fill)
    for y in range(s):
        t = y / s
        g.line([(0, y), (s, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(ACCENT_TOP, ACCENT_BOTTOM)) + (255,))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).polygon(pts, fill=255)
    img.paste(fill, (0, 0), mask)

    d = ImageDraw.Draw(img)

    def circle(cx, cy, r, colour):
        d.ellipse([(cx - r) * u, (cy - r) * u, (cx + r) * u, (cy + r) * u], fill=colour)

    for cx in (8.1, 15.9):
        circle(cx, 11.6, 3.7, (255, 255, 255, 255))
        circle(cx + 0.4, 11.9, 2.0, (118, 185, 0, 255))
        circle(cx + 0.5, 12.0, 0.95, (20, 24, 46, 255))
    d.polygon([(10.9 * u, 15.2 * u), (13.1 * u, 15.2 * u), (12 * u, 17.4 * u)], fill=(245, 165, 36, 255))
    return img.resize((size, size), Image.LANCZOS)


def main():
    for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png"), (512, "icon.png")]:
        draw(size).save(name)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [draw(n) for n in sizes]
    frames[-1].save("icon.ico", sizes=[(n, n) for n in sizes], append_images=frames[:-1])


if __name__ == "__main__":
    main()
