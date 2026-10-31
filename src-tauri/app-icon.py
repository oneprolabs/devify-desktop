"""Generate the 1024x1024 source icon used by `npx tauri icon`.

Run from the repository root:
    python3 src-tauri/app-icon.py src-tauri/app-icon.png
    npx tauri icon src-tauri/app-icon.png

The mark is a chat bubble with a delivered message inside: chat records are
read on this machine, digests are delivered to a Devify inbox.
"""

import sys

from PIL import Image, ImageDraw

SS = 4          # supersample factor, keeps edges smooth after downscaling
SIZE = 1024
INSET = 100     # macOS icon grid: 824x824 content area inside a 1024 canvas
RADIUS = 185


class Canvas:
    """ImageDraw wrapper that takes 1024-space coordinates."""

    def __init__(self, draw):
        self.d = draw

    def rect(self, box, radius, fill):
        x0, y0, x1, y1 = [value * SS for value in box]
        self.d.rounded_rectangle((x0, y0, x1 - 1, y1 - 1), radius=radius * SS, fill=fill)

    def line(self, points, color, width):
        self.d.line([(x * SS, y * SS) for x, y in points], fill=color, width=int(width * SS), joint="curve")

    def dot(self, x, y, radius, fill):
        self.d.ellipse(
            ((x - radius) * SS, (y - radius) * SS, (x + radius) * SS, (y + radius) * SS),
            fill=fill,
        )


def diagonal_gradient(size, start, end):
    steps = 256
    gradient = Image.new("RGB", (steps, steps))
    pixels = gradient.load()
    for y in range(steps):
        for x in range(steps):
            t = (x + y) / (2 * (steps - 1))
            pixels[x, y] = tuple(round(start[i] + (end[i] - start[i]) * t) for i in range(3))
    return gradient.resize((size, size), Image.BICUBIC)


def squircle_mask(size, inset, radius):
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (inset, inset, size - inset - 1, size - inset - 1), radius=radius, fill=255
    )
    return mask


def sheen(size, inset, radius):
    """Soft top highlight so the tile reads as a lit surface, not flat paint."""
    fade = Image.new("L", (1, size))
    for y in range(size):
        t = max(0.0, 1 - y / (size * 0.62))
        fade.putpixel((0, y), int(30 * t * t))
    layer = Image.new("RGBA", (size, size), (255, 255, 255, 0))
    layer.putalpha(fade.resize((size, size)))
    lit = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    lit.paste(layer, (0, 0), squircle_mask(size, inset, radius))
    return lit


def main(out_path: str) -> None:
    size = SIZE * SS
    icon = Image.new("RGBA", (size, size), (0, 0, 0, 0))

    # Warm chat green -> deep teal, close to WeChat's palette without copying its logo.
    tile = diagonal_gradient(size, (18, 205, 122), (3, 96, 94)).convert("RGBA")
    icon.paste(tile, (0, 0), squircle_mask(size, INSET * SS, RADIUS * SS))
    icon.alpha_composite(sheen(size, INSET * SS, RADIUS * SS))

    draw = Canvas(ImageDraw.Draw(icon))
    white = (255, 255, 255, 255)
    green = (3, 110, 88, 255)

    # Speech bubble.
    draw.rect((190, 244, 834, 646), 152, white)
    # Filled envelope inside it: solid shapes survive 16-32px rendering best.
    draw.rect((352, 330, 672, 556), 34, green)
    draw.line([(388, 384), (512, 458), (636, 384)], white, 34)

    icon.resize((SIZE, SIZE), Image.LANCZOS).save(out_path, "PNG")
    print(f"wrote {out_path}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "src-tauri/app-icon.png")
