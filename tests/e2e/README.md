# Ende-zu-Ende-Tests

Die Tests starten einen eigenen Open Rescue Simulation Server mit frischer Datenbank in einem
Temp-Ordner (Port 18493), steuern Headless-Chrome über das DevTools-Protokoll und prüfen
das Verhalten wie ein Nutzer: Bild, Ton, Aufnahme, Oberfläche. Der laufende Produktiv-
oder Entwicklungsserver bleibt unberührt.

```
tests/e2e/run.sh            # alle schnellen Tests (~4 min)
tests/e2e/run.sh --slow     # zusätzlich Speichertest über 10 Sessions
tests/e2e/run.sh voice      # nur Dateien, deren Name „voice“ enthält
tests/e2e/run.sh --keep     # Arbeitsordner mit Serverlog behalten
```

Benötigt: `node` (≥ 18), `google-chrome`, `ffprobe`; einzelne Tests zusätzlich `ffmpeg`
(Verteiler), `python3` (simulierte Lautsprecher), `sqlite3` (Anmeldung). Fehlt ein
Werkzeug, wird der betroffene Test übersprungen und so gemeldet.

| Datei | Prüft |
|---|---|
| `01_auth` | Login, öffentliche/geschützte API, zufälliger Token-Schlüssel bei Updates |
| `02_live_view` | Liveansicht beim Start, Aufnahme, Zuschauer kommt/geht, schneller Stopp |
| `03_distributor` | Verteiler unerreichbar / erreichbar (Bild + Ton) / Ausfall mittendrin |
| `04_prestart` | Vor-Start: alle Abbruchwege, Freigabe, Neustart, Oberfläche |
| `05_voice_room` | Lautsprecher-Gruppen, Lautsprecher je Raum, PTT-Dialog, Neustart-Knopf, Raum-Editor |
| `06_paging_viewer` | Durchsage an Viewer (parallel zum Video, Neuladen während Durchsage) und an Pager |
| `07_help` | Hilfeseite: Expertenmodus, Inhaltsverzeichnis, Handy-Breite |
| `08_memory` | (`--slow`) Speicher und Threads über 10 Sessions mit Zuschauer |
| `09_volume` | Stream-Lautstärke/Stumm wirkt auf Zuschauer, auch später beitretende (Server mit Testton) |
| `10_sessions_delete` | Sessions einzeln und alle über den Dialog löschen |
| `11_access` | Zugriffsschutz: Live-Video, Durchsage, Logs nur mit Login bzw. Ticket; Streamschlüssel nicht im Log |
| `12_viewer_lists` | Viewer-Listen in Einstellungen, Viewer-Dialog und Start-Dialog; online/offline nach Trennen |
| `13_pairing` | Kopplung von Viewer/Pager per PIN und QR-Link, Entkoppeln, Schutz gegen Raten |
| `14_speaker_approval` | Freigabe der Raumlautsprecher, Schlüssel, fremdes Gerät mit gleicher ID, alte Firmware |
| `15_https` | HTTPS aus der Oberfläche (mit Ersatzskript), Rechteprüfung, Wurzelzertifikat-Download und QR-Code |

Umgebungsvariablen: `ORS_E2E_PORT`, `ORS_E2E_BIN` (anderes Binary, z.B. Release),
`ORS_E2E_PASSWORD`, `CHROME_BIN`.

Neuer Test: Datei `tests/NN_name.test.js` mit `module.exports = { name, needs?, slow?, run(t) }`.
`t` bietet `api` (eingeloggt), `browser()`, `check(label, ok, detail)`, `restartServer()`,
`logMark()`/`logSince()`, `dataDir`, `workDir` und die Helfer aus `lib.js`.
