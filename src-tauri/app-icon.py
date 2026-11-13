"""Build the AIMyChats app icon: brand mark on a rounded macOS tile.

The source artwork lives in ``src/assets/app-icon-source.png`` (the brand mark
on a white background). This script keeps the artwork untouched and only adds
the platform chrome macOS expects:

* 1024x1024 canvas, icon body 824x824 inset by 100px (Apple grid);
* rounded corners, radius ~0.2247 x body (Apple's 185px at this size);
* transparent outside the tile, so the icon reads as rounded everywhere.

Run before ``npx tauri icon src-tauri/app-icon.png`` when the icon set needs
rebuilding. The generated image is also used at runtime by the app, so copy it
over ``src/assets/app-icon.png`` to keep both identical.
"""

import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

SIZE = 1024
BODY = 824
INSET = (SIZE - BODY) // 2
RADIUS = round(BODY * 0.2247)
SUPERSAMPLE = 4
ARTWORK_WIDTH = round(BODY * 0.84)
BACKGROUND = (255, 255, 255, 255)

SOURCE = Path(__file__).resolve().parents[1] / "src" / "assets" / "app-icon-source.png"


def rounded_mask(size: int, inset: int, radius: int) -> Image.Image:
    s = size * SUPERSAMPLE
    mask = Image.new("L", (s, s), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle(
        (inset * SUPERSAMPLE, inset * SUPERSAMPLE, s - inset * SUPERSAMPLE, s - inset * SUPERSAMPLE),
        radius=radius * SUPERSAMPLE,
        fill=255,
    )
    return mask.resize((size, size), Image.Resampling.LANCZOS)


def trim_margin(image: Image.Image) -> Image.Image:
    """Crop the uniform background margin without touching the artwork."""
    rgb = np.asarray(image.convert("RGB")).astype(np.int16)
    ink = rgb.min(axis=2) < 235
    ys, xs = np.where(ink)
    box = (int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1)
    return image.crop(box)


def build(source: Image.Image) -> Image.Image:
    mark = trim_margin(source.convert("RGBA"))
    width = ARTWORK_WIDTH
    height = max(1, round(mark.height * width / mark.width))
    mark = mark.resize((width, height), Image.Resampling.LANCZOS)

    tile = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    body = Image.new("RGBA", (BODY, BODY), BACKGROUND)
    body.alpha_composite(mark, ((BODY - width) // 2, (BODY - height) // 2))
    tile.paste(body, (INSET, INSET))

    mask = rounded_mask(SIZE, INSET, RADIUS)
    alpha = (
        np.asarray(tile.getchannel("A")).astype(np.uint16)
        * np.asarray(mask).astype(np.uint16)
        // 255
    ).astype("uint8")
    tile.putalpha(Image.fromarray(alpha, "L"))
    return tile


def main(out_path: str) -> None:
    build(Image.open(SOURCE)).save(out_path, "PNG")
    print(f"wrote {out_path}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else str(Path(__file__).with_name("app-icon.png")))
