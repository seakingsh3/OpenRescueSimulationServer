Unten ist eine reproduzierbare Installation auf Debian 13 (Trixie), inklusive systemd-Unit und den Systemänderungen, die wir im Verlauf als erforderlich identifiziert haben (GStreamer WebRTC + Recording/VOD via ffmpeg, Headless-Betrieb, Audio via ALSA).

---

## 1) Pakete installieren

```bash
sudo apt update
sudo apt install -y \
  ffmpeg \
  gstreamer1.0-tools \
  gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly \
  gstreamer1.0-libav \
  gstreamer1.0-alsa \
  gstreamer1.0-nice \
  alsa-utils
```

Hinweise:

* `gstreamer1.0-nice` ist nötig für WebRTC (libnice/ICE), sonst “libnice elements are not available”.
* `ffmpeg` ist nötig für VOD/Concat, sonst “failed to spawn ffmpeg … No such file”.

---

## 2) Root-Betrieb + Verzeichnisse

Der Dienst soll **als root** laufen. Die Verzeichnisse sollten trotzdem klar getrennt bleiben:

```bash
sudo install -d -o root -g root /var/lib/openresquesim
sudo install -d -o root -g root /var/cache/openresquesim
sudo install -d -o root -g root /var/log/openresquesim
```

---

## 3) systemd Unit: `/etc/systemd/system/openresquesim.service`

```ini
[Unit]
Description=OpenResqueSim Server
After=network.target sound.target
Wants=network.target

[Service]
Type=simple
User=root
Group=root

# Audio device access (ALSA)
PrivateDevices=false

# systemd erstellt diese Verzeichnisse automatisch:
StateDirectory=openresquesim
CacheDirectory=openresquesim
LogsDirectory=openresquesim
RuntimeDirectory=openresquesim

# XDG + HOME sauber auf StateDirectory legen (wichtig für Headless & Registry/Cache)
Environment=HOME=/var/lib/openresquesim
Environment=XDG_DATA_HOME=/var/lib/openresquesim
Environment=XDG_STATE_HOME=/var/lib/openresquesim
Environment=XDG_CACHE_HOME=/var/cache/openresquesim
Environment=XDG_RUNTIME_DIR=/run/openresquesim

# ffmpeg im PATH sicher verfügbar
Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# Optional: weniger "Authorization required..." Noise
Environment=NO_AT_BRIDGE=1

WorkingDirectory=/var/lib/openresquesim
ExecStart=/usr/bin/openresquesim-server

Restart=always
RestartSec=5

# Security Hardening (vorsichtig, da root)
NoNewPrivileges=false
PrivateTmp=true
ProtectHome=true
ProtectSystem=full

# Schreibe nur in diese Pfade
ReadWritePaths=/var/lib/openresquesim /var/cache/openresquesim /run/openresquesim /var/log/openresquesim

StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

Aktivieren/Starten:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now openresquesim
```

Logs live:

```bash
journalctl -u openresquesim -f -o cat
```

---

## 4) Audio-Reproduktionscheck (ALSA + GStreamer)

### ALSA Capture-Liste:

```bash
arecord -l
```

### Testaufnahme als Service-User:

```bash
sudo -u openresquesim arecord -D plughw:1,0 -f S16_LE -r 48000 -c 2 -d 1 /tmp/test.wav
ls -lh /tmp/test.wav
```

### GStreamer als Service-User:

```bash
sudo -u openresquesim gst-launch-1.0 -v \
  alsasrc device=plughw:1,0 \
  ! audio/x-raw,format=S16LE,rate=48000,channels=2 \
  ! fakesink
```

(Ersetze `plughw:1,0` bei Bedarf durch dein Device; gültige Namen findest du mit `arecord -L`.)

---

## 5) Systemänderungen für WebRTC/Recording/VOD (Kurzliste)

Damit es reproduzierbar läuft, müssen diese Punkte erfüllt sein:

1. **WebRTC / ICE**

   * Paket: `gstreamer1.0-nice`
   * Symptom ohne: “libnice elements are not available”

2. **Audio in headless systemd**

   * Pulseaudio ist im Systemdienst typischerweise nicht da → statt Pulse: **ALSA**
   * Root hat Zugriff auf `/dev/snd/*` (kein zusätzliches Group‑Setup nötig)
   * `PrivateDevices=false` bleibt nötig, damit ALSA‑Devices sichtbar sind

3. **VOD / ffmpeg**

   * Paket: `ffmpeg`
   * `PATH` im Service setzen, damit `ffmpeg` gefunden wird

4. **GStreamer CLI / Diagnose**

   * Paket: `gstreamer1.0-tools` für `gst-launch-1.0`

---

## 6) UDP-Freigaben (WebRTC)

WebRTC nutzt **UDP** für Medien. Stelle sicher, dass UDP auf dem Server erlaubt ist
und die Weiterleitung im LAN nicht blockiert.

### nftables (Beispiel, accept UDP)

```bash
sudo nft add rule inet filter input udp accept
sudo nft add rule inet filter input ct state established,related accept
```

### UFW (falls verwendet)

```bash
sudo ufw allow 3478/udp
sudo ufw allow 1024:65535/udp
```

> Hinweis: Der genaue UDP‑Portbereich hängt von deiner ICE/NAT‑Konfiguration ab.
> Für reine LAN‑Tests ist „UDP komplett erlauben“ am einfachsten.

---

## 7) Optional: Diagnostik-Schalter (nur bei Bedarf)

Wenn du wieder tiefer debuggen musst, temporär in der Unit ergänzen:

```ini
Environment=GST_DEBUG=webrtc*:6,nice*:6,alsa*:6,splitmux*:6,matroska*:6,*:3
Environment=GST_DEBUG_NO_COLOR=1
Environment=G_MESSAGES_DEBUG=all
```

Dann:

```bash
sudo systemctl daemon-reload
sudo systemctl restart openresquesim
journalctl -u openresquesim -f -o cat
```

---

## 8) HTTPS im LAN (für PTT auf iPad & Co.)

Das Mikrofon gibt der Browser nur über **HTTPS** oder `http://localhost` frei. Der Server bringt einen eigenen
HTTPS-Listener mit, ein Reverse-Proxy ist nicht mehr nötig. Zuschauer-Viewer, Pager, Notizblätter und
Raumlautsprecher laufen weiter über HTTP auf Port 8393; nur die Geräte, von denen aus gesprochen wird, nutzen HTTPS.

### Einrichten (einmalig auf dem Server)

```bash
sudo ./scripts/setup_https.sh              # Zertifikat für <hostname>.local + alle IPs
sudo ./scripts/setup_https.sh ors.local 192.168.1.20   # oder eigene Namen/IPs
```

Das Skript installiert `mkcert` und `avahi-daemon`, erzeugt eine lokale Zertifizierungsstelle unter
`/etc/openresquesim/mkcert`, stellt ein Serverzertifikat (825 Tage) nach `/etc/openresquesim/tls/` aus, schreibt
`/etc/openresquesim/tls.env` mit

```
ORS_TLS_CERT=/etc/openresquesim/tls/server.pem
ORS_TLS_KEY=/etc/openresquesim/tls/server-key.pem
ORS_TLS_BIND=0.0.0.0:8443
```

und hängt diese Datei als systemd-Drop-in an die Unit. Danach lauscht der Server zusätzlich auf
`https://<hostname>.local:8443`. Ohne diese Variablen bleibt HTTPS aus.

### Wurzelzertifikat auf die Geräte

Das Wurzelzertifikat liegt unter `/etc/openresquesim/tls/rootCA.pem` und ist über die App unter
`http://<server>:8393/static/ca/ors-rootCA.crt` herunterladbar.

- **iPad/iPhone**: Link in Safari öffnen → „Profil geladen“ → Einstellungen → Profil installieren → danach
  Allgemein → Info → Zertifikatsvertrauen → Schalter für das Zertifikat einschalten. Anschließend
  `https://<hostname>.local:8443/ui` als Lesezeichen anlegen.
- **Windows**: Datei doppelklicken → Zertifikat installieren → Lokaler Computer → Vertrauenswürdige Stammzertifizierungsstellen.
- **Linux/macOS**: `rootCA.pem` in den System-Truststore aufnehmen (macOS: Schlüsselbund, „Immer vertrauen“).

Nach 825 Tagen das Skript erneut ausführen; das Wurzelzertifikat auf den Geräten bleibt gültig.
Alternative bei vorhandenem Internet: echte Domain mit Let's Encrypt, dann entfällt die Verteilung des Wurzelzertifikats.
