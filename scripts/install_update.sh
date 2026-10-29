#!/usr/bin/env bash
# Installiert ein Update aus der Oberfläche – nur wenn es mit einem hinterlegten Schlüssel
# signiert ist. Läuft als root (eigene systemd-Unit, über sudo vom Dienstbenutzer gestartet).
#
#   install_update.sh /var/lib/openresquesim/updates/<paket>.deb
#
# Erwartet daneben <paket>.deb.sig (Ed25519-Signatur, erzeugt mit scripts/sign_update.sh).
# Vertrauenswürdige Schlüssel: /etc/openresquesim/update-keys/*.pem (nur root kann sie ablegen).
#
# Warum signiert: Ein .deb führt bei der Installation eigene Skripte als root aus. Ohne
# Signatur könnte jeder, der den Dienst übernimmt, über ein „Update“ root werden.
set -euo pipefail

UPDATES_DIR="${ORS_UPDATES_DIR:-/var/lib/openresquesim/updates}"
KEY_DIR="${ORS_UPDATE_KEY_DIR:-/etc/openresquesim/update-keys}"
PACKAGE=openresquesim-server

fail() { echo "FEHLER: $*" >&2; exit 1; }

[[ $EUID -eq 0 ]] || fail "muss als root laufen"
[[ $# -eq 1 ]] || fail "genau eine Datei erwartet"

src="$(realpath -e -- "$1" 2>/dev/null)" || fail "Datei nicht gefunden: $1"
dir="$(realpath -e -- "$UPDATES_DIR")"
[[ "$(dirname -- "$src")" == "$dir" ]] || fail "Datei liegt nicht im Update-Ordner $dir"
[[ "$src" == *.deb && -f "$src" && ! -L "$1" ]] || fail "keine .deb-Datei"
[[ -f "$src.sig" ]] || fail "Signatur fehlt ($src.sig). Paket mit scripts/sign_update.sh signieren und beide Dateien hochladen."

shopt -s nullglob
keys=("$KEY_DIR"/*.pem)
[[ ${#keys[@]} -gt 0 ]] || fail "kein Update-Schlüssel hinterlegt ($KEY_DIR). Öffentlichen Schlüssel einmal auf der Konsole ablegen, siehe Hilfe."

# Kopie in einen nur für root zugänglichen Ordner: Der Dienst kann die Datei im
# Update-Ordner nach der Prüfung nicht mehr austauschen.
work="$(mktemp -d /root/ors-update.XXXXXX 2>/dev/null || mktemp -d)"
chmod 700 "$work"
trap 'rm -rf -- "$work"' EXIT
cp -- "$src" "$work/update.deb"
cp -- "$src.sig" "$work/update.deb.sig"

echo "==> Signatur prüfen"
ok=""
for k in "${keys[@]}"; do
  if openssl pkeyutl -verify -pubin -inkey "$k" -rawin -in "$work/update.deb" -sigfile "$work/update.deb.sig" >/dev/null 2>&1; then
    ok="$k"; break
  fi
done
[[ -n "$ok" ]] || fail "Signatur ungültig – das Paket stammt nicht von einem hinterlegten Schlüssel oder wurde verändert."
echo "    gültig ($(basename -- "$ok"))"

name="$(dpkg-deb -f "$work/update.deb" Package)"
version="$(dpkg-deb -f "$work/update.deb" Version)"
[[ "$name" == "$PACKAGE" ]] || fail "falsches Paket: $name"
echo "==> Installiere $name $version"

# _apt darf den Ordner nicht lesen; apt installiert lokale Dateien dann ohne Sandbox (Hinweis harmlos)
DEBIAN_FRONTEND=noninteractive apt-get install -y --reinstall --allow-downgrades \
  -o Dpkg::Options::=--force-confold "$work/update.deb"
echo "==> Fertig: $name $version"
