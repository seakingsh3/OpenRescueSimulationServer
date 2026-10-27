#!/usr/bin/env bash
# Ende-zu-Ende-Tests: baut den Server und führt alle Tests aus.
#   tests/e2e/run.sh              alle schnellen Tests
#   tests/e2e/run.sh --slow       zusätzlich Speichertest (dauert ~3 min)
#   tests/e2e/run.sh prestart     nur Tests, deren Dateiname „prestart“ enthält
#   tests/e2e/run.sh --keep       Arbeitsordner (Serverlog, Aufnahmen) behalten
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --quiet
exec node tests/e2e/run.js "$@"
