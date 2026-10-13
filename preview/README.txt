STATIC PILOT PREVIEW (UNVALIDATED)
==================================
Public static preview of the rebuilt Krutrim Mulyankan / Chakra Engine dashboard.

Includes:
- Rebuilt selection / progress / inspect UI (live execution disabled)
- REAL CLI harness aggregates from the 13 Oct 2026 IST india-pilot compare
  · Claude Code 2.1.296 / claude-opus-5-5: 23/24, median 4.1s end-to-end CLI latency
  · Muse Code 1.4.7 / muse-spark-1.3: 22/24, median 26.7s end-to-end CLI latency
- Per-task PASS/FAIL + latency only
- Highly visible UNVALIDATED / PILOT labels — this is not a ranking

Privacy:
- Private pilot prompts, model answers, raw.jsonl, credentials and local paths are omitted
- Synthetic smoke fixtures remain for software-check demos only

Execution:
- This Vercel site cannot run models, CLIs, imports or writes
- Live evaluation is local-only: clone the GitHub repo and run `km ui` (localhost gate)

Notes:
- Latency = observed end-to-end CLI run latency, not intrinsic model speed
- Wonder Max was unavailable; Muse Spark must not be relabeled as Wonder Max
- Separate product site krutrimmulyankan.in is unrelated and unchanged
