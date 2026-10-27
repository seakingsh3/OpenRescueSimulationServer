#!/usr/bin/env bash
# Ersatz für scripts/setup_https.sh in den Tests (kein root, kein mkcert): gibt die
# gleichen Meldungen aus und legt ein selbstsigniertes Wurzelzertifikat im Datenordner ab.
set -euo pipefail
echo "==> Pakete (mkcert, NSS-Tools, Avahi für <hostname>.local)"
echo "    alles vorhanden"
sleep 1
echo "==> Serverzertifikat für: ${*:-test.local}"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=E2E Test CA" \
  -keyout "$ORS_DATA_DIR/rootCA-key.pem" -out "$ORS_DATA_DIR/rootCA.pem" >/dev/null 2>&1
sleep 1
echo "Fertig. HTTPS-Adresse für Ausbilder-Geräte:  https://${1:-test.local}:8443/ui"
