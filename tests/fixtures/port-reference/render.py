"""Rendering a script into frames, and the CLI around it.

A script is JSON:

    {"width": 40, "frames": 12, "lines": [
        {"text": "hello", "effect": "wipe", "options": {"easing": "out_quad"}}
    ]}

Output is frames separated by a form feed, each frame being its lines joined by
newlines and padded to the declared width. Nothing else reaches stdout, so two
implementations can be compared byte for byte.
"""

import json
import sys

from . import effects, palette

FRAME_SEPARATOR = "\x0c"
VERSION = "1.0.0"


def render_line(line, frame, frames):
    text = line["text"]
    options = effects.options_for(line.get("options"))
    cells = effects.resolve(line["effect"])(text, frame, frames, options)
    return "".join(palette.paint(cell.character, cell.colour) for cell in cells)


def pad(rendered, text, width):
    if len(text) >= width:
        return rendered
    return rendered + " " * (width - len(text))


def render(script):
    width = script["width"]
    frames = script["frames"]
    if frames < 1:
        raise ValueError("a script needs at least one frame")
    if width < 1:
        raise ValueError("a script needs a positive width")
    rendered_frames = []
    for frame in range(frames):
        rows = [
            pad(render_line(line, frame, frames), line["text"], width)
            for line in script["lines"]
        ]
        rendered_frames.append("\n".join(rows))
    return FRAME_SEPARATOR.join(rendered_frames)


def main(argv):
    if len(argv) == 1 and argv[0] == "--version":
        sys.stdout.write(f"{VERSION}\n")
        return 0
    if len(argv) != 2 or argv[0] != "render":
        sys.stderr.write("usage: render <script.json> | --version\n")
        return 2
    with open(argv[1], "r", encoding="utf-8") as handle:
        script = json.load(handle)
    sys.stdout.write(render(script))
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
