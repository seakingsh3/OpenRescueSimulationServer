#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/first_start.sh --admin-password <pw> [options]

Options:
  --admin-password, -p   Admin-Passwort setzen
  --marker-type          Marker-Typ hinzufügen: "Name" "#rrggbb" oder "Name:#rrggbb"
  --no-default-markers   Keine Standard-Marker anlegen
  --data-dir             ORS_DATA_DIR setzen
  --db-path              ORS_DB_PATH setzen
  --bin                  Pfad zur openresquesim-server Binary
  --help                 Hilfe anzeigen

Beispiele:
  scripts/first_start.sh -p 'Sicher123!'
  scripts/first_start.sh -p 'Sicher123!' --marker-type "Alarm" "#ff0000"
  scripts/first_start.sh -p 'Sicher123!' --data-dir /var/lib/openresquesim
USAGE
}

ADMIN_PASSWORD=""
NO_DEFAULT=0
DATA_DIR=""
DB_PATH=""
BIN_PATH=""
SETUP_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --admin-password|-p)
      ADMIN_PASSWORD="${2:-}"
      shift 2
      ;;
    --marker-type)
      if [[ -z "${2:-}" ]]; then
        echo "Fehler: --marker-type braucht Werte" >&2
        exit 1
      fi
      if [[ "$2" == *":"* || "$2" == *","* ]]; then
        SETUP_ARGS+=(--marker-type "$2")
        shift 2
      else
        if [[ -z "${3:-}" ]]; then
          echo "Fehler: --marker-type braucht Name und Farbe" >&2
          exit 1
        fi
        SETUP_ARGS+=(--marker-type "$2" "$3")
        shift 3
      fi
      ;;
    --no-default-markers)
      NO_DEFAULT=1
      shift 1
      ;;
    --data-dir)
      DATA_DIR="${2:-}"
      shift 2
      ;;
    --db-path)
      DB_PATH="${2:-}"
      shift 2
      ;;
    --bin)
      BIN_PATH="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unbekanntes Argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$ADMIN_PASSWORD" && -z "${ORS_DEFAULT_ADMIN_PASSWORD:-}" ]]; then
  echo "Fehler: Admin-Passwort fehlt. Nutze --admin-password oder ORS_DEFAULT_ADMIN_PASSWORD." >&2
  exit 1
fi

if [[ "$NO_DEFAULT" -eq 1 ]]; then
  SETUP_ARGS+=(--no-default-markers)
fi

if [[ -z "$BIN_PATH" ]]; then
  if command -v openresquesim-server >/dev/null 2>&1; then
    BIN_PATH="$(command -v openresquesim-server)"
  elif [[ -x "./target/release/openresquesim_server" ]]; then
    BIN_PATH="./target/release/openresquesim_server"
  elif [[ -x "./target/debug/openresquesim_server" ]]; then
    BIN_PATH="./target/debug/openresquesim_server"
  elif command -v cargo >/dev/null 2>&1; then
    BIN_PATH="cargo run --bin openresquesim_server --"
  else
    echo "Fehler: openresquesim-server Binary nicht gefunden." >&2
    exit 1
  fi
fi

ENV_VARS=()
if [[ -n "$DATA_DIR" ]]; then
  ENV_VARS+=(ORS_DATA_DIR="$DATA_DIR")
fi
if [[ -n "$DB_PATH" ]]; then
  ENV_VARS+=(ORS_DB_PATH="$DB_PATH")
fi

if [[ -n "$ADMIN_PASSWORD" ]]; then
  SETUP_ARGS+=(--admin-password "$ADMIN_PASSWORD")
fi

if [[ "$BIN_PATH" == "cargo run --bin openresquesim_server --" ]]; then
  env "${ENV_VARS[@]}" cargo run --bin openresquesim_server -- setup "${SETUP_ARGS[@]}"
else
  env "${ENV_VARS[@]}" "$BIN_PATH" setup "${SETUP_ARGS[@]}"
fi
