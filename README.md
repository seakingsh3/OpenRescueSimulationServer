# Open Rescue Simulation Server

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

## Installation

Voraussetzung: ein Rechner mit **Debian 13 (Trixie), amd64**, im selben Netz wie Kameras und Geräte. Für die
Installation braucht er einmal Internet (für die Pakete aus Debian, z.B. GStreamer und FFmpeg), danach nicht mehr.

1. Das aktuelle Paket `openresquesim-server_<version>_amd64.deb` von der
   **[Releases-Seite](../../releases/latest)** herunterladen.
2. Installieren – mit `apt`, nicht mit `dpkg -i`, damit alle benötigten Pakete mitkommen:
   ```bash
   sudo apt install ./openresquesim-server_*_amd64.deb
   ```
3. Die Installation stellt drei Fragen:

   | Frage | Standard | Hinweis |
   |---|---|---|
   | Port der Weboberfläche | 8393 | Ohne Grund nicht ändern, die Geräte erwarten ihn. |
   | HTTPS jetzt einrichten? | ja | Nötig für Durchsagen aus dem Browser. Namen/IPs für das Zertifikat: leer lassen = automatisch. |
   | Firewall öffnen? | ja | Nur wenn die Firewall `ufw` aktiv ist. |

4. Am Ende zeigt die Installation die Adresse, z.B. `http://192.168.1.20:8393/`. Im Browser öffnen: Ein Assistent
   legt das Admin-Konto an. Fertig.

Der Server läuft danach als Dienst `openresquesim` und startet mit dem Rechner. Bedienung, Einstellungen und die
ausführliche Hilfe gibt es in der Oberfläche (Menü **Hilfe**).

### Danach

- **iPads/PCs der Ausbilder** (für Durchsagen): einmal das Wurzelzertifikat installieren. Download und QR-Code
  unter **Optionen → Systemstatus → 🔒 HTTPS**, Anleitung in der Hilfe.
- **Einstellungen der Installation ändern** (Port, HTTPS, Firewall): `sudo dpkg-reconfigure openresquesim-server`
- **Ohne Fragen installieren** (Standardwerte): `sudo DEBIAN_FRONTEND=noninteractive apt install ./openresquesim-server_*_amd64.deb`

### Update

Neues Paket von der Releases-Seite laden und genauso installieren:
```bash
sudo apt install ./openresquesim-server_*_amd64.deb
```
Einstellungen, Zertifikat, Datenbank und Aufnahmen bleiben, der Dienst startet von selbst neu.

**Aus der Oberfläche** (Optionen → Einstellungen → Update) geht es auch, aber nur mit signierten Paketen: `.deb`
und `.deb.sig` aus dem Release zusammen hochladen. Dafür einmal den Projektschlüssel `update-signing.pem` aus dem
Release auf dem Server hinterlegen – nur, wenn du den Paketen dieses Projekts vertraust:
```bash
sudo install -D -m 644 update-signing.pem /etc/openresquesim/update-keys/openrescuesim.pem
```

### Entfernen

```bash
sudo apt purge openresquesim-server
```
Dienst, Einstellungen und Zertifikate werden entfernt. Datenbank und Aufnahmen unter `/var/lib/openresquesim`
bleiben erhalten.

### Selbst bauen

`cargo deb` erzeugt dasselbe Paket unter `target/debian/` (Rust stable und die GStreamer-Entwicklungspakete
nötig). Einrichtung ohne Paket: [Installscript.md](Installscript.md).

## HTTPS

Browser geben das Mikrofon (Push-to-Talk) nur über HTTPS frei. Der Server lauscht dafür zusätzlich auf Port 8443.
Die Installation richtet das auf Wunsch gleich ein; erneuern oder später einrichten als Admin unter
**Optionen → Systemstatus → 🔒 HTTPS**. Es entsteht eine lokale Zertifizierungsstelle (mkcert, ohne Internet).
Ihr Wurzelzertifikat steht unter `http://<server>:8393/ca/ors-rootCA.crt` zum Installieren auf iPads und PCs bereit,
auf der Systemseite auch als QR-Code.

Viewer, Pager, Raumlautsprecher und Notizblätter brauchen kein HTTPS.

## Konfiguration

Alles Wichtige wird in der Oberfläche eingestellt. Für den Betrieb gibt es zusätzlich Umgebungsvariablen
(beim Paket in `/etc/openresquesim/server.env`, danach `sudo systemctl restart openresquesim`):

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

Raumlautsprecher (Raspberry Pi, ESP32) und die „On-Air“-Lampe am Raumeingang liegen in einem eigenen Repository:
**open-rescue-simulation-devices** (auf GitHub neben diesem Projekt).

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

## Mitmachen

Fehler und Vorschläge gern als Issue. Die Oberfläche und die Doku sind auf Deutsch.
