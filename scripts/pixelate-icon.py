"""Turns a generated pixel-art picture into true pixel art.

An image model draws "pixels" that are soft at the edges. This reads one
colour per cell of the picture's grid, limits the palette, and writes each
cell back as a hard square, at the size macOS expects of an icon.

    python3 scripts/pixelate-icon.py <generated.png> assets/icon.png <grid>

<grid> is how many cells across the model drew the picture on. Measure it:
a wrong grid shows up as stray pixels along every edge.
"""

import sys

from PIL import Image

COLOURS = 24      # a small fixed palette, as on 8-bit hardware
CANVAS = 1024
ARTWORK = 824     # the part of the canvas Apple's icon grid gives to artwork

source = Image.open(sys.argv[1]).convert("RGBA")
grid = int(sys.argv[3])
cell = source.width / grid

small = Image.new("RGBA", (grid, grid))
for row in range(grid):
    for column in range(grid):
        # The middle half of the cell, away from its soft edges.
        box = (
            round((column + 0.25) * cell),
            round((row + 0.25) * cell),
            round((column + 0.75) * cell),
            round((row + 0.75) * cell),
        )
        red, green, blue, alpha = source.crop(box).resize((1, 1), Image.Resampling.BOX).getpixel((0, 0))
        small.putpixel((column, row), (red, green, blue, 255 if alpha > 128 else 0))

alpha = small.getchannel("A")
flat = small.convert("RGB").quantize(COLOURS, method=Image.Quantize.MAXCOVERAGE).convert("RGBA")
flat.putalpha(alpha)
sprite = flat.crop(alpha.getbbox())

# The largest whole number of screen pixels per cell that still fits.
scale = ARTWORK // max(sprite.size)
size = (sprite.width * scale, sprite.height * scale)
icon = Image.new("RGBA", (CANVAS, CANVAS))
icon.paste(sprite.resize(size, Image.Resampling.NEAREST), ((CANVAS - size[0]) // 2, (CANVAS - size[1]) // 2))
icon.save(sys.argv[2])
