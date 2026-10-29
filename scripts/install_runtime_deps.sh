#!/usr/bin/env bash
set -euo pipefail

# Installiert die Laufzeit-Abhängigkeiten, die nicht in das fertige Paket gebündelt werden können.

if [[ "${EUID}" -ne 0 ]]; then
  echo "Bitte als root bzw. via sudo ausführen, damit Pakete installiert werden können."
  exit 1
fi

DEBIAN_DEPS=(
  libgstreamer1.0-0
  gstreamer1.0-plugins-base
  gstreamer1.0-plugins-good
  gstreamer1.0-plugins-bad
  gstreamer1.0-plugins-ugly
  gstreamer1.0-libav
  gstreamer1.0-nice
  gstreamer1.0-alsa
  gstreamer1.0-tools
  libglib2.0-0
  libssl3
  libsqlite3-0
)

if command -v apt-get >/dev/null 2>&1; then
  echo "Aktualisiere Paketquellen..."
  apt-get update

  echo "Installiere Laufzeitabhängigkeiten für Open Rescue Simulation Server..."
  apt-get install -y --no-install-recommends "${DEBIAN_DEPS[@]}"

  echo "Fertig. Starte anschließend die kompilierte Binary (z.B. target/release/openresquesim_server)."
else
  echo "Kein apt-get gefunden. Bitte installiere folgende Pakete manuell:"
  printf '  %s\n' "${DEBIAN_DEPS[@]}"
  exit 1
fi
