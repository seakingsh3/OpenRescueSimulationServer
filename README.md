# OpenRescueSim Server

Server für das Simulationstraining im Rettungsdienst und in anderen Einsatzbereichen: Er nimmt die Kameras und
Mikrofone im Übungsraum auf, zeigt das Bild live auf Zuschauer-Bildschirmen, lässt Ausbilder während der Übung
Marker setzen und Durchsagen in den Raum sprechen, und stellt danach alles fürs Debriefing bereit.

Läuft auf einem Linux-Rechner im lokalen Netz, ohne Cloud und ohne Internetzugang. Bedient wird er komplett im
Browser (Desktop, iPad, Handy).

Lizenz: [GPLv3](LICENSE). Fremdbestandteile: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Funktionen

- **Simulation steuern**: Raum wählen, Vor-Start (Bild und Ton prüfen ohne Aufnahme), Start/Stopp, Laufzeit, Hotkeys.
- **Aufnahme**: mehrere Kameras (RTSP, z.B. IP-Kameras) und Audio-Interfaces werden zu einem Bild zusammengesetzt
  und als Video gespeichert.
- **Marker**: während der Übung per Klick oder Taste setzen (eigene Marker-Typen, mit Standbild), im Debriefing
  direkt anspringen.
- **Debriefing**: Aufnahmen mit Zeitleiste und Markern abspielen, Sessions verwalten und löschen.
- **Live-Ansicht**: Zuschauer-Bildschirme bekommen das Bild per WebRTC mit geringer Latenz. Neue Geräte werden
  einmalig per PIN oder QR-Code gekoppelt.
- **Externer Verteiler (optional)**: Bild und Ton zusätzlich per RTMP an einen Streaming-Server
  (z.B. MediaMTX, nginx-rtmp, Owncast) für viele Zuschauer.
- **Durchsagen („Voice of Room“)**: Push-to-Talk aus dem Browser an Raumlautsprecher (Raspberry Pi oder ESP32),
  an Pager und an Zuschauer-Bildschirme, nach Raum bzw. Gruppe auswählbar. Raumlautsprecher müssen vom Admin
  freigegeben werden.
- **Kamerasteuerung**: schwenkbare Axis-Kameras (PTZ) über eine eigene, touch-taugliche Seite steuern.
- **Teilnehmer**: Notizblätter per geheimem Link ohne Login.
- **Verwaltung**: Benutzer und Rollen (Admin/Nutzer), Registrierung per Einladungs-Token, Systemstatus mit
  Live-Logs, HTTPS-Einrichtung per Knopf, Hilfe direkt in der Oberfläche (`/help/`).

## Schnellstart

### Debian-Paket (empfohlen)

```bash
cargo install cargo-deb      # einmalig, auf dem Build-Rechner
cargo deb
sudo apt install ./target/debian/openresquesim-server_*.deb
```

Das Paket bringt die Abhängigkeiten (GStreamer, FFmpeg, mkcert …) und die Hilfsskripte mit. Den Dienst
(systemd-Unit) richtest du nach [Installscript.md](Installscript.md) ein. Danach `http://<server>:8393` öffnen. Beim ersten Aufruf führt eine Ersteinrichtung durch Admin-Konto und
Grundeinstellungen.

### Aus dem Quellcode

```bash
sudo ./scripts/install_runtime_deps.sh   # GStreamer, FFmpeg, OpenSSL, SQLite
cargo build --release
./target/release/openresquesim_server
```

Voraussetzungen zum Bauen: Rust (stable) sowie die GStreamer-Entwicklungspakete
(`libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, `libgstreamer-plugins-bad1.0-dev`).
Die Oberfläche nutzt Tailwind CSS mit daisyUI. Das fertige Stylesheet liegt im Repository
(`src/web/frontend/static/dist/style.css`). Nach Änderungen an den Vorlagen neu bauen:
`cd src/web/frontend && npm install && npm run build:css`.

Ausführliche Einrichtung als Dienst (systemd, Audio-Rechte, Firewall für WebRTC): [Installscript.md](Installscript.md).

## HTTPS

Browser geben das Mikrofon (Push-to-Talk) nur über HTTPS frei. Der Server lauscht dafür zusätzlich auf Port 8443.
Einrichten lässt sich das als Admin unter **Optionen → Systemstatus → 🔒 HTTPS** oder auf dem Server mit
`sudo ./scripts/setup_https.sh [namen/ips …]`. Das Skript erzeugt eine lokale Zertifizierungsstelle (mkcert).
Ihr Wurzelzertifikat steht danach unter `http://<server>:8393/ca/ors-rootCA.crt` zum Installieren auf iPads und
PCs bereit, auf der Systemseite auch als QR-Code.

Viewer, Pager, Raumlautsprecher und Notizblätter brauchen kein HTTPS.

## Konfiguration

Alles Wichtige wird in der Oberfläche eingestellt. Für den Betrieb gibt es zusätzlich Umgebungsvariablen:

| Variable | Bedeutung | Standard |
|---|---|---|
| `ORS_BIND` | Adresse und Port für HTTP | `0.0.0.0:8393` |
| `ORS_TLS_CERT`, `ORS_TLS_KEY` | Zertifikat und Schlüssel (PEM); aktiviert HTTPS | – |
| `ORS_TLS_BIND` | Adresse und Port für HTTPS | `0.0.0.0:8443` |
| `ORS_DATA_DIR` | Datenverzeichnis (Datenbank, Aufnahmen) | `/var/lib/openresquesim` |
| `ORS_DB_PATH` | eigener Pfad zur SQLite-Datenbank | im Datenverzeichnis |
| `ORS_LOG_DIR`, `ORS_LOG_FILE` | Ort der Logdateien | automatisch |
| `ORS_AUTH_SECRET` | Schlüssel für Login-Tokens (sonst einmalig erzeugt und gespeichert) | – |
| `ORS_STUN_SERVER` | STUN-Server für WebRTC, `none` = aus | `stun://stun.l.google.com:19302` |
| `ORS_ICE_TCP` | WebRTC auch über TCP | an |

Ist das Datenverzeichnis nicht anlegbar, nutzt der Server benutzerlokale Pfade. Datenbank-Migrationen laufen beim
Start automatisch.

## Zusatzgeräte

| Ordner | Gerät |
|---|---|
| [`speaker/`](speaker/README.md) | Raumlautsprecher auf einem Raspberry Pi (Python, Opus über RTP) |
| [`speaker-esp32/`](speaker-esp32/README.md) | Raumlautsprecher auf einem ESP32-C3 mit I²S-Verstärker (Entwurf) |
| [`relay-onair/`](relay-onair/README.md) | „On-Air“-Lampe am Raumeingang, schaltet bei laufender Übung (ESP32-Relais) |
| `ors_viewer/` | eigenständiger Zuschauer-Client (Rust, FFmpeg) |

## Tests

Ende-zu-Ende-Tests starten einen eigenen Server mit frischer Datenbank und steuern Headless-Chrome:

```bash
tests/e2e/run.sh            # alle schnellen Tests
tests/e2e/run.sh --slow     # zusätzlich Speichertest
```

Voraussetzungen und Details: [tests/e2e/README.md](tests/e2e/README.md).

## Aufbau des Quellcodes

| Pfad | Inhalt |
|---|---|
| `src/main.rs` | Start, Routen, HTTP/HTTPS |
| `src/pipeline.rs`, `src/sim.rs` | GStreamer-Pipeline, Aufnahme, Ablauf einer Simulation |
| `src/ws.rs` | WebRTC für die Live-Ansicht |
| `src/voice.rs` | Durchsagen und Raumlautsprecher |
| `src/routes.rs`, `src/db.rs` | API und Datenbank (SQLite) |
| `src/pairing.rs`, `src/stream_auth.rs` | Gerätekopplung, Absicherung der Live-Verbindungen |
| `src/web/frontend/` | Oberfläche (Tera-Vorlagen, JavaScript, Hilfe) |
| `migrations/` | Datenbank-Migrationen |
| `scripts/` | Installation, HTTPS-Einrichtung |
| [`API_Befehle.md`](API_Befehle.md) | Übersicht der API |
| [`Updates_Snapshot.md`](Updates_Snapshot.md) | Änderungsprotokoll |

## Mitmachen

Fehler und Vorschläge gern als Issue. Die Oberfläche und die Doku sind auf Deutsch.
