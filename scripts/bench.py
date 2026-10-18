"""Size and startup budget for the Rust km binary. Usage: bench.py [path-to-km]

Prints binary size and median --help startup, then fails (exit 1) if the
binary exceeds MAX_BYTES. Startup is reported, not gated: CI clocks are noisy.
"""

from __future__ import annotations

import statistics
import subprocess
import sys
import time
from pathlib import Path

MAX_BYTES = 9 * 1024 * 1024


def main() -> int:
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "km-rs/target/release/km")
    size = binary.stat().st_size
    samples = []
    for _ in range(9):
        start = time.perf_counter()
        subprocess.run([str(binary), "--help"], capture_output=True, check=True)
        samples.append((time.perf_counter() - start) * 1000)
    print(
        f"{binary}: {size / 1e6:.2f} MB (budget {MAX_BYTES / 1e6:.0f} MB), "
        f"startup median {statistics.median(samples):.1f} ms"
    )
    if size > MAX_BYTES:
        print(f"OVER BUDGET by {(size - MAX_BYTES) / 1e6:.2f} MB", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
