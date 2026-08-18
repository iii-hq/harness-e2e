"""Easing curves. Every function maps progress in [0, 1] to output in [0, 1].

Values are rounded to six decimal places so a port in another language reaches
the same numbers without depending on the host's float formatting.
"""

import math

PRECISION = 6


def _clamp(progress):
    return 0.0 if progress < 0.0 else 1.0 if progress > 1.0 else progress


def linear(progress):
    return round(_clamp(progress), PRECISION)


def in_quad(progress):
    progress = _clamp(progress)
    return round(progress * progress, PRECISION)


def out_quad(progress):
    progress = _clamp(progress)
    return round(1.0 - (1.0 - progress) * (1.0 - progress), PRECISION)


def in_out_cubic(progress):
    progress = _clamp(progress)
    if progress < 0.5:
        value = 4.0 * progress * progress * progress
    else:
        shifted = -2.0 * progress + 2.0
        value = 1.0 - (shifted * shifted * shifted) / 2.0
    return round(value, PRECISION)


def out_bounce(progress):
    progress = _clamp(progress)
    divisor = 7.5625
    threshold = 2.75
    if progress < 1.0 / threshold:
        value = divisor * progress * progress
    elif progress < 2.0 / threshold:
        progress -= 1.5 / threshold
        value = divisor * progress * progress + 0.75
    elif progress < 2.5 / threshold:
        progress -= 2.25 / threshold
        value = divisor * progress * progress + 0.9375
    else:
        progress -= 2.625 / threshold
        value = divisor * progress * progress + 0.984375
    return round(value, PRECISION)


def in_sine(progress):
    progress = _clamp(progress)
    return round(1.0 - math.cos((progress * math.pi) / 2.0), PRECISION)


CURVES = {
    "linear": linear,
    "in_quad": in_quad,
    "out_quad": out_quad,
    "in_out_cubic": in_out_cubic,
    "out_bounce": out_bounce,
    "in_sine": in_sine,
}


def curve(name):
    if name not in CURVES:
        raise KeyError(f"unknown easing curve: {name}")
    return CURVES[name]
