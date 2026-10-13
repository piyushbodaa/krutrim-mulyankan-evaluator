#!/usr/bin/env bash
set -euo pipefail
task_project="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
task_python="${KM_PYTHON:-$task_project/.venv/bin/python}"
if [[ ! -x "$task_python" ]]; then
  echo 'Set KM_PYTHON to an existing Python environment with project dependencies.' >&2
  exit 1
fi
cd -- "$task_project"
export PYTHONPATH="$task_project/src${PYTHONPATH:+:$PYTHONPATH}"
export PYTHONDONTWRITEBYTECODE=1
exec "$task_python" -c 'import sys; from pathlib import Path; from kritrim_mulyankan.local_ui import serve; serve(Path.cwd(), int(sys.argv[1]))' "${1:-8765}"
