"""Small statistics helpers for aggregating results with confidence intervals."""

from __future__ import annotations

import math
from statistics import mean, stdev

Z95 = 1.959964


def wilson_interval(successes: int, n: int, z: float = Z95) -> tuple[float, float] | None:
    """95% Wilson score interval for a success rate; well behaved at 0/n and n/n."""
    if n <= 0:
        return None
    p = successes / n
    denom = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denom
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denom
    return max(0.0, centre - half), min(1.0, centre + half)


def mean_ci(values: list[float], z: float = Z95) -> tuple[float, float, float] | None:
    """(mean, low, high) with a normal-approximation 95% interval; zero width for n=1."""
    if not values:
        return None
    m = mean(values)
    half = z * stdev(values) / math.sqrt(len(values)) if len(values) > 1 else 0.0
    return m, m - half, m + half
