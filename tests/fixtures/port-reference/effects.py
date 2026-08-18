"""The effects themselves.

An effect answers one question: at this frame, which characters of the source
text are visible, and in what colour. Order of resolution is fixed and every
random choice comes from the bundled generator, so two implementations of the
same effect agree character for character.
"""

from . import easing, palette
from .random import Sequence


class Cell:
    __slots__ = ("character", "colour")

    def __init__(self, character, colour):
        self.character = character
        self.colour = colour


def _blank(width):
    return [Cell(" ", None) for _ in range(width)]


def _visible_count(width, progress, curve_name):
    eased = easing.curve(curve_name)(progress)
    return min(width, int(eased * width + 1e-9))


def typewriter(text, frame, frames, options):
    width = len(text)
    colours = palette.gradient(options["colours"], max(width, 1))
    revealed = _visible_count(width, frame / max(frames - 1, 1), options["easing"])
    row = _blank(width)
    for index in range(revealed):
        row[index] = Cell(text[index], colours[index])
    return row


def wipe(text, frame, frames, options):
    width = len(text)
    colours = palette.gradient(options["colours"], max(width, 1))
    progress = frame / max(frames - 1, 1)
    edge = _visible_count(width, progress, options["easing"])
    row = _blank(width)
    for index in range(width):
        if index < edge:
            row[index] = Cell(text[index], colours[index])
        elif index == edge and edge < width:
            row[index] = Cell(options["leader"], colours[index])
    return row


def scatter(text, frame, frames, options):
    """Characters land in an order the seed decides, never in reading order."""
    width = len(text)
    colours = palette.gradient(options["colours"], max(width, 1))
    order = Sequence(options["seed"]).shuffled(list(range(width)))
    landed = _visible_count(width, frame / max(frames - 1, 1), options["easing"])
    row = _blank(width)
    for index in order[:landed]:
        row[index] = Cell(text[index], colours[index])
    return row


def rain(text, frame, frames, options):
    """Each column falls on its own schedule, drawn once from the seed."""
    width = len(text)
    colours = palette.gradient(options["colours"], max(width, 1))
    sequence = Sequence(options["seed"])
    delays = [sequence.below(max(frames // 2, 1)) for _ in range(width)]
    row = _blank(width)
    for index in range(width):
        if frame >= delays[index]:
            row[index] = Cell(text[index], colours[index])
    return row


def pulse(text, frame, frames, options):
    """Every character is present throughout; only the colour moves."""
    width = len(text)
    progress = easing.curve(options["easing"])(frame / max(frames - 1, 1))
    start = palette.parse_hex(options["colours"][0])
    end = palette.parse_hex(options["colours"][-1])
    colour = palette.blend(start, end, progress)
    return [Cell(character, colour) for character in text]


EFFECTS = {
    "typewriter": typewriter,
    "wipe": wipe,
    "scatter": scatter,
    "rain": rain,
    "pulse": pulse,
}

DEFAULTS = {
    "easing": "linear",
    "colours": ["#ffffff"],
    "seed": 1,
    "leader": "|",
}


def resolve(name):
    if name not in EFFECTS:
        raise KeyError(f"unknown effect: {name}")
    return EFFECTS[name]


def options_for(declared):
    options = dict(DEFAULTS)
    options.update(declared or {})
    if not options["colours"]:
        raise ValueError("an effect needs at least one colour")
    if len(options["leader"]) != 1:
        raise ValueError("the leader must be a single character")
    return options
