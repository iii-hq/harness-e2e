"""Colour handling: hex parsing, gradients, and the ANSI escapes a frame carries.

The gradient walks the sRGB cube linearly and rounds half away from zero, which
is stated here because a port that rounds half to even drifts by one on exact
midpoints.
"""

RESET = "\x1b[0m"


def parse_hex(value):
    text = value.lstrip("#")
    if len(text) != 6:
        raise ValueError(f"colour must be six hex digits: {value}")
    try:
        return tuple(int(text[index : index + 2], 16) for index in (0, 2, 4))
    except ValueError as error:
        raise ValueError(f"colour is not hexadecimal: {value}") from error


def to_hex(colour):
    return "#" + "".join(f"{channel:02x}" for channel in colour)


def _round_half_up(value):
    return int(value + 0.5) if value >= 0 else -int(-value + 0.5)


def blend(start, end, progress):
    progress = 0.0 if progress < 0.0 else 1.0 if progress > 1.0 else progress
    return tuple(
        max(0, min(255, _round_half_up(begin + (finish - begin) * progress)))
        for begin, finish in zip(start, end)
    )


def gradient(stops, steps):
    if steps < 1:
        raise ValueError("a gradient needs at least one step")
    colours = [parse_hex(stop) for stop in stops]
    if len(colours) == 1:
        return [colours[0]] * steps
    if steps == 1:
        return [colours[0]]
    spans = len(colours) - 1
    result = []
    for step in range(steps):
        position = (step / (steps - 1)) * spans
        index = min(int(position), spans - 1)
        result.append(blend(colours[index], colours[index + 1], position - index))
    return result


def foreground(colour):
    red, green, blue = colour
    return f"\x1b[38;2;{red};{green};{blue}m"


def paint(character, colour):
    if colour is None:
        return character
    return f"{foreground(colour)}{character}{RESET}"
