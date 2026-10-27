#!/usr/bin/env bash
# Richtet HTTPS für OpenRescueSim im lokalen Netz ein (lokale Zertifizierungsstelle mit mkcert).
#
#   sudo ./scripts/setup_https.sh [Hostname/IP ...]
#
# Ohne Argumente werden <hostname>.local, <hostname> und alle IPv4-Adressen des Rechners eingetragen.
# Ergebnis:
#   /etc/openresquesim/tls/server.pem, server-key.pem   – Serverzertifikat (825 Tage)
#   /etc/openresquesim/tls/rootCA.pem                    – Wurzelzertifikat zum Verteilen auf iPads/PCs
#   <static>/ca/ors-rootCA.crt                           – Wurzelzertifikat zum Download über die App
#   /etc/openresquesim/tls.env + systemd-Drop-in         – ORS_TLS_CERT/ORS_TLS_KEY/ORS_TLS_BIND
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "Bitte mit sudo ausführen." >&2
  exit 1
fi

UNIT="${ORS_SYSTEMD_UNIT:-openresquesim.service}"
TLS_DIR=/etc/openresquesim/tls
TLS_BIND="${ORS_TLS_BIND:-0.0.0.0:8443}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Statisches Verzeichnis der Web-Oberfläche finden (Paket oder Quellbaum)
STATIC_DIR=""
for d in "${ORS_STATIC_DIR:-}" /usr/share/openresquesim/web/frontend/static "$SCRIPT_DIR/../src/web/frontend/static"; do
  if [[ -n "$d" && -d "$d" ]]; then STATIC_DIR="$(cd "$d" && pwd)"; break; fi
done

# Namen/IPs für das Zertifikat
if [[ $# -gt 0 ]]; then
  NAMES=("$@")
else
  HN="$(hostname -s 2>/dev/null || hostname)"
  NAMES=("${HN}.local" "${HN}")
  while read -r ip; do [[ -n "$ip" ]] && NAMES+=("$ip"); done < <(hostname -I 2>/dev/null | tr ' ' '\n' | grep -E '^[0-9]+\.' || true)
fi

echo "==> Pakete (mkcert, NSS-Tools, Avahi für <hostname>.local)"
# Nur nachinstallieren, was fehlt – im Übungsnetz gibt es oft kein Internet. Das
# Debian-Paket bringt mkcert und libnss3-tools als Abhängigkeit mit.
missing=()
command -v mkcert >/dev/null 2>&1 || missing+=(mkcert)
command -v certutil >/dev/null 2>&1 || missing+=(libnss3-tools)
command -v avahi-daemon >/dev/null 2>&1 || missing+=(avahi-daemon)
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "    installiere: ${missing[*]}"
  if ! { apt-get update -qq && apt-get install -y --no-install-recommends "${missing[@]}" >/dev/null; }; then
    if ! command -v mkcert >/dev/null 2>&1; then
      echo "FEHLER: mkcert fehlt und konnte nicht installiert werden (kein Internet?)." >&2
      echo "        Paket mkcert einmal mit Internet installieren, dann erneut ausführen." >&2
      exit 1
    fi
    echo "    Hinweis: ${missing[*]} nicht installierbar – fahre ohne fort."
  fi
else
  echo "    alles vorhanden"
fi
systemctl enable --now avahi-daemon >/dev/null 2>&1 || true

echo "==> Lokale Zertifizierungsstelle"
export CAROOT=/etc/openresquesim/mkcert
mkdir -p "$CAROOT" "$TLS_DIR"
mkcert -install >/dev/null 2>&1 || mkcert -install
chmod 700 "$CAROOT"

echo "==> Serverzertifikat für: ${NAMES[*]}"
( cd "$TLS_DIR" && mkcert -cert-file server.pem -key-file server-key.pem "${NAMES[@]}" )
cp "$CAROOT/rootCA.pem" "$TLS_DIR/rootCA.pem"
chmod 640 "$TLS_DIR/server-key.pem"
chmod 644 "$TLS_DIR/server.pem" "$TLS_DIR/rootCA.pem"
# Dienstbenutzer darf den Key lesen (falls die Unit nicht als root läuft)
SVC_USER="$(systemctl show -p User --value "$UNIT" 2>/dev/null || true)"
if [[ -n "$SVC_USER" && "$SVC_USER" != "root" ]]; then
  chgrp "$SVC_USER" "$TLS_DIR/server-key.pem" 2>/dev/null || true
fi

if [[ -n "$STATIC_DIR" ]]; then
  echo "==> Wurzelzertifikat zum Download: $STATIC_DIR/ca/ors-rootCA.crt"
  mkdir -p "$STATIC_DIR/ca"
  cp "$CAROOT/rootCA.pem" "$STATIC_DIR/ca/ors-rootCA.crt"
  chmod 644 "$STATIC_DIR/ca/ors-rootCA.crt"
else
  echo "==> Kein statisches Web-Verzeichnis gefunden, Wurzelzertifikat liegt unter $TLS_DIR/rootCA.pem"
fi

echo "==> Umgebung für den Dienst"
cat > /etc/openresquesim/tls.env <<EOF
ORS_TLS_CERT=$TLS_DIR/server.pem
ORS_TLS_KEY=$TLS_DIR/server-key.pem
ORS_TLS_BIND=$TLS_BIND
EOF
mkdir -p "/etc/systemd/system/${UNIT}.d"
cat > "/etc/systemd/system/${UNIT}.d/tls.conf" <<EOF
[Service]
EnvironmentFile=-/etc/openresquesim/tls.env
EOF
systemctl daemon-reload
if systemctl is-active --quiet "$UNIT"; then
  systemctl restart "$UNIT"
  echo "==> $UNIT neu gestartet"
else
  echo "==> $UNIT läuft nicht; Umgebung greift beim nächsten Start. Manuell: set -a; . /etc/openresquesim/tls.env; ./openresquesim_server"
fi

PORT="${TLS_BIND##*:}"
echo
echo "Fertig. HTTPS-Adresse für Ausbilder-Geräte:  https://${NAMES[0]}:${PORT}/ui"
echo "Wurzelzertifikat auf iPad/PC installieren:"
if [[ -n "$STATIC_DIR" ]]; then
  echo "  Download:  http://${NAMES[0]}:8393/static/ca/ors-rootCA.crt   (oder per AirDrop: $TLS_DIR/rootCA.pem)"
else
  echo "  Datei:     $TLS_DIR/rootCA.pem (per AirDrop/Mail auf das Gerät)"
fi
echo "  iPad:      Einstellungen → Profil geladen → Installieren, danach Allgemein → Info → Zertifikatsvertrauen einschalten"
echo "  Windows:   Doppelklick → Zertifikat installieren → Lokaler Computer → Vertrauenswürdige Stammzertifizierungsstellen"
echo "  Linux/Mac: mkcert -install auf dem Gerät mit kopiertem CAROOT, oder rootCA.pem in den System-Truststore"
echo "Gültigkeit des Serverzertifikats: 825 Tage. Danach dieses Skript erneut ausführen (Wurzelzertifikat bleibt gültig)."
