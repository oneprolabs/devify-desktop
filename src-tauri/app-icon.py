"""Prepare the AImyChats brand mark for Tauri icon generation.

The source artwork is the public AImyChats mark from https://aimychats.com/zh/.
Keep the source in ``src/assets/app-icon.png`` and run this script before
``npx tauri icon src-tauri/app-icon.png`` when the icon set needs rebuilding.
"""

import sys
from pathlib import Path

from PIL import Image

SIZE = 1024
SOURCE = Path(__file__).resolve().parents[1] / "src" / "assets" / "app-icon.png"


def main(out_path: str) -> None:
    source = Image.open(SOURCE).convert("RGBA")
    source = source.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    canvas.alpha_composite(source)
    canvas.save(out_path, "PNG")
    print(f"wrote {out_path}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else str(Path(__file__).with_name("app-icon.png")))
