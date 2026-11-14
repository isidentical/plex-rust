"""Turns a generated pixel-art picture into true pixel art.

An image model draws "pixels" that are soft at the edges. This reads one
colour per cell of the picture's grid, limits the palette, and writes each
cell back as a hard square, at the size macOS expects of an icon.

    python3 scripts/pixelate-icon.py <generated.png> assets/icon.png
"""

import sys

from PIL import Image

GRID = 32         # the generated picture is drawn on 32 by 32 cells
COLOURS = 24      # a small fixed palette, as on 8-bit hardware
CELL_SIZE = 27    # the 30 cells the tile spans make 810, inside Apple's 824
CANVAS = 1024

source = Image.open(sys.argv[1]).convert("RGBA")
cell = source.width // GRID

small = Image.new("RGBA", (GRID, GRID))
for row in range(GRID):
    for column in range(GRID):
        # The middle half of the cell, away from its soft edges.
        box = (
            column * cell + cell // 4,
            row * cell + cell // 4,
            column * cell + 3 * cell // 4,
            row * cell + 3 * cell // 4,
        )
        red, green, blue, alpha = source.crop(box).resize((1, 1), Image.Resampling.BOX).getpixel((0, 0))
        small.putpixel((column, row), (red, green, blue, 255 if alpha > 128 else 0))

alpha = small.getchannel("A")
flat = small.convert("RGB").quantize(COLOURS, method=Image.Quantize.MAXCOVERAGE).convert("RGBA")
flat.putalpha(alpha)
tile = flat.crop(alpha.getbbox())

size = (tile.width * CELL_SIZE, tile.height * CELL_SIZE)
icon = Image.new("RGBA", (CANVAS, CANVAS))
icon.paste(tile.resize(size, Image.Resampling.NEAREST), ((CANVAS - size[0]) // 2, (CANVAS - size[1]) // 2))
icon.save(sys.argv[2])
