"""The bundled generator.

The host language's random module is deliberately not used: a port has to
reproduce these exact draws, so the algorithm is spelled out here. It is a
64-bit linear congruential generator with the constants below, and every draw
takes the high bits, which are the ones that mix.
"""

MODULUS = 1 << 64
MULTIPLIER = 6364136223846793005
INCREMENT = 1442695040888963407
SHIFT = 33


class Sequence:
    __slots__ = ("state",)

    def __init__(self, seed):
        self.state = seed % MODULUS

    def next(self):
        self.state = (self.state * MULTIPLIER + INCREMENT) % MODULUS
        return self.state >> SHIFT

    def below(self, ceiling):
        if ceiling < 1:
            raise ValueError("ceiling must be positive")
        return self.next() % ceiling

    def shuffled(self, items):
        """Fisher-Yates from the end, the order a port must reproduce."""
        result = list(items)
        for index in range(len(result) - 1, 0, -1):
            swap = self.below(index + 1)
            result[index], result[swap] = result[swap], result[index]
        return result
