#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
THRESHOLD="8000000000000"

if [[ $# -gt 0 && "${1}" != --* ]]; then
	THRESHOLD="${1}"
	shift
fi

cd "${ROOT_DIR}"
cmd=(cargo run --bin oracle_price_threshold_demo -- --threshold "${THRESHOLD}")

# Interactive mode is enabled by default for transaction inspection.
# Set ORACLE_DEMO_INTERACTIVE=0 to disable prompts.
if [[ "${ORACLE_DEMO_INTERACTIVE:-1}" != "0" ]]; then
	cmd+=(--interactive)
fi

cmd+=("$@")
exec "${cmd[@]}"