#!/usr/bin/env bash
# Installiert den Raumlautsprecher-Client auf einem Raspberry Pi (Raspberry Pi OS / Debian).
# Aufruf:  sudo ./install_speaker.sh <server-host:port> [Name] [Gruppe] [ALSA-Gerät]
# Beispiel: sudo ./install_speaker.sh 192.168.1.10:8393 "Simraum 1" simraum default
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "Bitte mit sudo ausführen." >&2
  exit 1
fi

SERVER="${1:-}"
NAME="${2:-$(hostname)}"
GROUP="${3:-default}"
ALSA="${4:-default}"
SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST=/opt/ors-speaker
CONF=/etc/ors-speaker.conf

if [[ -z "$SERVER" && ! -f "$CONF" ]]; then
  echo "Server-Adresse fehlt. Aufruf: sudo $0 <host:port> [Name] [Gruppe] [ALSA-Gerät]" >&2
  exit 1
fi

echo "==> Pakete installieren"
apt-get update -qq
apt-get install -y --no-install-recommends \
  python3 python3-gi gir1.2-gstreamer-1.0 gir1.2-glib-2.0 \
  gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-alsa python3-websockets alsa-utils

echo "==> Dateien nach $DEST"
mkdir -p "$DEST"
install -m 0755 "$SRC_DIR/ors-speaker.py" "$DEST/ors-speaker.py"
install -m 0644 "$SRC_DIR/ors-speaker.conf.example" "$DEST/ors-speaker.conf.example"

if [[ ! -f "$CONF" ]]; then
  echo "==> Konfiguration $CONF anlegen"
  sed -e "s|^server = .*|server = ${SERVER}|" \
      -e "s|^name = .*|name = ${NAME}|" \
      -e "s|^group = .*|group = ${GROUP}|" \
      -e "s|^alsa_device = .*|alsa_device = ${ALSA}|" \
      "$SRC_DIR/ors-speaker.conf.example" > "$CONF"
else
  echo "==> $CONF existiert, bleibt unverändert"
  if [[ -n "$SERVER" ]]; then
    sed -i "s|^server = .*|server = ${SERVER}|" "$CONF"
    echo "    Server-Adresse aktualisiert auf ${SERVER}"
  fi
fi

echo "==> systemd-Dienst"
install -m 0644 "$SRC_DIR/ors-speaker.service" /etc/systemd/system/ors-speaker.service
systemctl daemon-reload
systemctl enable --now ors-speaker.service

echo
echo "Fertig. Status:   systemctl status ors-speaker"
echo "Live-Log:         journalctl -u ors-speaker -f"
echo "Audiogeräte:      aplay -l    (ALSA-Gerät in $CONF unter alsa_device eintragen)"
echo "Das Gerät erscheint in OpenRescueSim unter Einstellungen → Kameras & Räume → Raumlautsprecher."
