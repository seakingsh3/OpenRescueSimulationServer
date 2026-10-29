#!/usr/bin/env bash
# Signiert Update-Pakete für die Installation aus der Oberfläche (auf dem Build-Rechner).
#
#   scripts/sign_update.sh --init          # einmalig: Schlüsselpaar erzeugen
#   scripts/sign_update.sh <paket>.deb     # Signatur <paket>.deb.sig erzeugen
#
# Der private Schlüssel bleibt auf diesem Rechner (~/.config/openresquesim/, nur für dich lesbar).
# Den öffentlichen Schlüssel legst du einmal auf jedem Server ab (Befehl siehe --init).
set -euo pipefail

KEY_DIR="${ORS_SIGN_DIR:-$HOME/.config/openresquesim}"
KEY="$KEY_DIR/update-signing.key"
PUB="$KEY_DIR/update-signing.pem"

if [[ "${1:-}" == "--init" ]]; then
  [[ -e "$KEY" ]] && { echo "Schlüssel existiert schon: $KEY" >&2; exit 2; }
  mkdir -p "$KEY_DIR"; chmod 700 "$KEY_DIR"
  (umask 077; openssl genpkey -algorithm ed25519 -out "$KEY")
  openssl pkey -in "$KEY" -pubout -out "$PUB"
  echo "Schlüsselpaar erzeugt:"
  echo "  privat:     $KEY   (sichern, nicht weitergeben)"
  echo "  öffentlich: $PUB"
  echo
  echo "Auf jedem Server einmal ablegen (Datei hinkopieren, dann):"
  echo "  sudo install -D -m 644 update-signing.pem /etc/openresquesim/update-keys/$(hostname -s).pem"
  exit 0
fi

deb="${1:-}"
[[ -f "$deb" && "$deb" == *.deb ]] || { echo "Aufruf: $0 --init | $0 <paket>.deb" >&2; exit 2; }
[[ -f "$KEY" ]] || { echo "Kein Schlüssel – zuerst: $0 --init" >&2; exit 2; }
openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$deb" -out "$deb.sig"
openssl pkeyutl -verify -pubin -inkey "$PUB" -rawin -in "$deb" -sigfile "$deb.sig" >/dev/null
echo "Signiert: $deb.sig"
echo "In der Oberfläche beide Dateien hochladen: $(basename -- "$deb") und $(basename -- "$deb").sig"
