"""Draws Nook's icon and installer artwork.

The mark is the site's brand pill (a dark glass pill with an accent dot)
on a violet squircle tile, so it reads on light and dark taskbars.
Everything is drawn in code at 4x and downsampled, so there is no source
file to keep in sync. Run from the repo root after changing it:

    python installer/make_assets.py

Writes assets/icon/nook.ico, assets/icon/nook-<size>.png and the
installer's wizard images in installer/art/.
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ICON_DIR = ROOT / "assets" / "icon"
ART_DIR = ROOT / "installer" / "art"

ACCENT_TOP = (130, 104, 255)
ACCENT_BOTTOM = (80, 48, 222)
PILL_TOP = (34, 34, 42)
PILL_BOTTOM = (6, 6, 9)
DOT = (196, 184, 255)
SS = 4  # supersampling factor


def squircle_mask(size: int, inset: float, n: float = 5.0) -> Image.Image:
    """Superellipse |x|^n + |y|^n <= 1, the Windows 11 tile shape."""
    big = size * SS
    mask = Image.new("L", (big, big), 0)
    px = mask.load()
    r = big / 2 - inset * SS
    c = big / 2
    for y in range(big):
        dy = abs((y + 0.5 - c) / r)
        if dy >= 1:
            continue
        # Solve for the half width of this row once, then fill the span.
        half = r * (1 - dy**n) ** (1 / n)
        x0 = max(0, int(c - half))
        x1 = min(big, int(c + half) + 1)
        for x in range(x0, x1):
            px[x, y] = 255
    return mask


def vertical_gradient(w: int, h: int, top, bottom) -> Image.Image:
    grad = Image.new("RGB", (1, h))
    for y in range(h):
        t = y / max(1, h - 1)
        grad.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    return grad.resize((w, h))


def pill_layer(big: int, box, dot: bool) -> Image.Image:
    """The brand pill: dark gradient, a hairline highlight on top, accent dot."""
    x0, y0, x1, y1 = box
    h = y1 - y0
    layer = Image.new("RGBA", (big, big), (0, 0, 0, 0))

    # Soft drop shadow under the pill.
    shadow = Image.new("L", (big, big), 0)
    ImageDraw.Draw(shadow).rounded_rectangle(
        (x0, y0 + h * 0.18, x1, y1 + h * 0.18), radius=h / 2, fill=110
    )
    shadow = shadow.filter(ImageFilter.GaussianBlur(h * 0.22))
    layer.paste((20, 6, 60, 255), (0, 0), shadow)

    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=h / 2, fill=255)
    grad = Image.new("RGB", (big, big))
    grad.paste(vertical_gradient(big, int(h) + 1, PILL_TOP, PILL_BOTTOM), (0, int(y0)))
    layer.paste(grad, (0, 0), mask)

    # Inner top highlight: the pill mask minus itself shifted down.
    edge = max(1.0, h * 0.06)
    shifted = Image.new("L", (big, big), 0)
    ImageDraw.Draw(shifted).rounded_rectangle(
        (x0, y0 + edge, x1, y1 + edge), radius=h / 2, fill=255
    )
    rim = ImageChops.subtract(mask, shifted).point(lambda v: v * 0.30)
    layer.paste((255, 255, 255, 255), (0, 0), rim)

    if dot:
        d = h * 0.40
        cx = x1 - h / 2
        cy = (y0 + y1) / 2
        glow = Image.new("L", (big, big), 0)
        ImageDraw.Draw(glow).ellipse((cx - d, cy - d, cx + d, cy + d), fill=90)
        glow = glow.filter(ImageFilter.GaussianBlur(d * 0.5))
        layer.paste(ACCENT_TOP + (255,), (0, 0), ImageChops.multiply(glow, mask))
        ImageDraw.Draw(layer).ellipse(
            (cx - d / 2, cy - d / 2, cx + d / 2, cy + d / 2), fill=DOT + (255,)
        )
    return layer


def icon(size: int) -> Image.Image:
    big = size * SS
    # Small sizes get a fuller tile and a bigger pill so they stay legible.
    inset = 0 if size <= 24 else size * 0.035
    tile_mask = squircle_mask(size, inset)

    tile = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    tile.paste(vertical_gradient(big, big, ACCENT_TOP, ACCENT_BOTTOM), (0, 0), tile_mask)

    if size >= 32:
        # Glassy sheen over the top half of the tile.
        sheen = Image.new("L", (big, big), 0)
        ImageDraw.Draw(sheen).ellipse((-big * 0.3, -big * 0.75, big * 1.3, big * 0.42), fill=34)
        sheen = sheen.filter(ImageFilter.GaussianBlur(big * 0.04))
        tile.paste((255, 255, 255, 255), (0, 0), ImageChops.multiply(sheen, tile_mask))
        # Hairline rim so the tile holds its edge on violet wallpapers.
        inner = squircle_mask(size, inset + 0.6 * (size / 64))
        rim = ImageChops.subtract(tile_mask, inner).point(lambda v: v * 0.22)
        tile.paste((255, 255, 255, 255), (0, 0), rim)

    # The bento grid the pill opens into: two small tiles over a wide one.
    small = size <= 24
    gx0, gx1 = (0.20, 0.80) if small else (0.22, 0.78)
    gy0, gy1 = (0.47, 0.84) if small else (0.47, 0.80)
    gap = 0.08 if small else 0.055
    cw = (gx1 - gx0 - gap) / 2
    chh = (gy1 - gy0 - gap) / 2
    cells = Image.new("L", (big, big), 0)
    draw = ImageDraw.Draw(cells)
    radius = big * (0.03 if small else 0.045)
    # The first tile is lit, like the one under the cursor.
    for x0, y0, x1, y1, alpha in (
        (gx0, gy0, gx0 + cw, gy0 + chh, 150),
        (gx1 - cw, gy0, gx1, gy0 + chh, 78),
        (gx0, gy1 - chh, gx1, gy1, 78),
    ):
        draw.rounded_rectangle(
            (big * x0, big * y0, big * x1, big * y1), radius=radius, fill=alpha
        )
    tile.paste((255, 255, 255, 255), (0, 0), cells)

    pw = 0.56 if small else 0.48
    ph = pw * (15 / 36)  # the site's brand pill is 36x15
    cy = 0.27 if small else 0.28
    box = (
        big * (0.5 - pw / 2),
        big * (cy - ph / 2),
        big * (0.5 + pw / 2),
        big * (cy + ph / 2),
    )
    tile.alpha_composite(pill_layer(big, box, dot=size >= 20))
    # Keep everything inside the tile.
    tile.putalpha(ImageChops.multiply(tile.getchannel("A"), tile_mask))
    return tile.resize((size, size), Image.LANCZOS)


def wizard_large(w: int, h: int) -> Image.Image:
    """Left panel of the welcome and finish pages."""
    img = vertical_gradient(w, h, (30, 22, 74), (9, 7, 22)).convert("RGBA")
    glow = Image.new("L", (w, h), 0)
    ImageDraw.Draw(glow).ellipse((-w * 0.5, h * 0.18, w * 1.5, h * 0.62), fill=120)
    glow = glow.filter(ImageFilter.GaussianBlur(w * 0.22))
    img.paste(ACCENT_BOTTOM + (255,), (0, 0), glow)
    s = int(w * 0.58)
    mark = icon(256).resize((s, s), Image.LANCZOS)
    my = int(h * 0.38 - s / 2)
    img.alpha_composite(mark, ((w - s) // 2, my))

    draw = ImageDraw.Draw(img)
    try:
        font = ImageFont.truetype("segoeuib.ttf", round(w * 0.17))
    except OSError:
        font = ImageFont.load_default(round(w * 0.17))
    draw.text((w / 2, my + s + w * 0.10), "Nook", font=font, fill=(255, 255, 255), anchor="mt")
    return img.convert("RGB")


def main() -> None:
    ICON_DIR.mkdir(parents=True, exist_ok=True)
    ART_DIR.mkdir(parents=True, exist_ok=True)

    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = {s: icon(s) for s in sizes}
    for s in (16, 32, 48, 256):
        frames[s].save(ICON_DIR / f"nook-{s}.png")
    icon(512).save(ICON_DIR / "nook-512.png")
    # Pillow writes each size from its own frame when they are appended.
    frames[256].save(
        ICON_DIR / "nook.ico",
        sizes=[(s, s) for s in sizes],
        append_images=[frames[s] for s in sizes if s != 256],
    )

    # Inno Setup picks the bitmap closest to the screen's DPI.
    for scale, suffix in ((1.0, "100"), (1.25, "125"), (1.5, "150"), (2.0, "200")):
        wizard_large(round(164 * scale), round(314 * scale)).save(ART_DIR / f"wizard-{suffix}.png")
        # Transparent, so it sits on the light and the dark wizard alike.
        icon(round(55 * scale)).save(ART_DIR / f"wizard-small-{suffix}.png")
    print(f"wrote {ICON_DIR} and {ART_DIR}")


if __name__ == "__main__":
    main()
