# API Befehle (Übersicht)

Stand: 2026-10-22 (abgeglichen mit den Routen in `src/main.rs` und `is_public_api_path`)

Hinweis:
- Auth per Bearer-Token: `Authorization: Bearer <token>`.
- `Admin` bedeutet zusätzliche Rollenprüfung im Handler.
- WebSockets können keinen Authorization-Header setzen: Live-Verbindungen erwarten `?token=<JWT>` (eingeloggte Oberfläche) bzw. `?ticket=<…>` (Viewer/Pager ohne Login, siehe unten).

## Zugriffsklassen

### Public (ohne Login)
- `POST /auth/login` — Login, liefert Token.
- `POST /auth/register` — Registrierung mit Invite-Token.
- `GET /api/setup/status` — Setup-Status.
- `POST /api/setup/complete` — Setup abschließen.
- `GET /api/qr` — QR-Code SVG.
- `GET /api/ui/content` — Viewer Overlay Daten (Ticker/Trivia).
- `GET /api/ticker` — Ticker lesen.
- `GET /api/trivia` — Trivia-Liste lesen.
- `GET /api/trivia/file/:name` — Trivia-Datei ausliefern.
- `GET /api/sessions/:id/snapshot` — Session-Previewbild.
- `GET /api/markers/:id/snapshot` — Marker-Vorschaubild.
- `GET /api/host/ip` — Server-IP (für Zuschauer-QR-Links).
- `GET /api/sim/status` — läuft eine Übung: `{active, session_id, name, start_time, room_id, room_name, prestart_session_id}`. `prestart_session_id` ist gesetzt, solange ein Vor-Start läuft (noch nicht freigegeben, `active` dann `false`). Bewusst ohne Login für die On-Air-Lampe und ähnliche Geräte.
- `GET /api/viewers/:id/ws` — Viewer Steuer-WebSocket-Handshake.
- `POST /api/viewers/:id/report` — Viewer Fehlerreport senden.
- `GET /api/tn/:token` — Notizblatt-Daten + aktive Session (Teilnehmer, per Token).
- `PUT /api/tn/:token/data` — Formular speichern `{data:{feld:wert}, changes:[{field,label,value}]}`; bei aktiver Verknüpfung entsteht je Änderung ein Marker (Typ „Teilnehmer“, Nutzer „Teilnehmer“).
- `POST /api/tn/:token/link` (`{session_id}`), `POST /api/tn/:token/unlink`, `POST /api/tn/:token/reset`

### Protected (Login erforderlich)
- `GET /auth/me`
- `GET/POST /api/audio/inputs` — Audio-Eingänge `{id, device, label, gain, channels?}`; `channels` = 1-basierte Hardware-Kanäle (leer = alle gemischt, genau zwei = Stereo). `GET /api/host/audio_inputs` liefert je ALSA-Karte `channels` (native Kanalzahl).
- `GET /api/audio/state`
- `GET /api/audio/sources/list`
- `POST /api/audio/volume/set` (`{volumePercent: 0..400}`), `POST /api/audio/mute/toggle` — Stream-Lautstärke: wirkt auf alle WebRTC-Zuschauer (auch später beitretende) und auf die Aufnahme.
- `GET /api/camera/brands`
- `GET/POST /api/cameras`, `PATCH/DELETE /api/cameras/:id` — Anlegen/Ändern/Löschen nur Admin. Felder u.a. `brand` (`generic`|`axis`), `ptz_capable`, `vapix_port`; `vapix_username`/`vapix_password` werden nur geschrieben, nie ausgeliefert.
- `POST /api/cameras/:id/ptz` — Schwenken/Neigen/Zoomen für alle eingeloggten Nutzer: `{pan, tilt, zoom}` je −100..100, `0,0,0` = Stopp. Nur für `ptz_capable` (400), Hersteller ohne PTZ-Unterstützung 501, Kamera nicht erreichbar 502. Axis per VAPIX `continuouspantiltmove`/`continuouszoommove`, HTTP-Basic-Auth.
- `GET/POST /api/cameras/:id/snapshot`
- `GET /api/camera_preview/:id`
- `POST /api/camera/add_simple`
- `GET /api/health`
- `POST /api/options/vlc`
- `GET /api/layouts`
- `POST /api/rebuild`
- `POST /api/set_layout`
- `POST /api/set_source`
- `POST /api/settings/layout`
- `GET /api/ui/nutzerinfo` — lädt `Updates_Snapshot.md` für das Updates-Modal auf der Systemseite.
- `GET /api/voice/status` — läuft eine Durchsage, Lautsprecher online.
- `GET /api/voice/speakers` — registrierte Raumlautsprecher (DB + Live-Status), `GET /api/voice/groups`.
- `PATCH /api/voice/speakers/:id` (`{name?, group?, volume?, muted?}`), `POST /api/voice/speakers/:id/cmd` (`{cmd: test_tone|identify|mute|set_volume|reboot|ping, value?}`; `reboot` = Knopf ⟳ in der Lautsprecherliste), `DELETE /api/voice/speakers/:id`.
- `GET /api/rooms`, `POST /api/rooms`, `GET/PATCH/DELETE /api/rooms/:id` — Felder `name`, `layout`, `audio_device`, `sources` (JSON-String, Kamera-IDs je Slot), `viewer_id`, `speakers` (JSON-String, Lautsprecher-IDs, z.B. `"[3,7]"`). Ändert ein PATCH `speakers`, während gerade eine Durchsage an diesen Raum läuft, greift das sofort.
- `POST /api/sim/prestart` — Vor-Start (Pipeline ohne Aufnahme, nur Ausbilder-Viewer). Ein noch offener Vor-Start wird ersetzt.
- `POST /api/sim/commit` — Vor-Start freigeben (Session aktiv, Aufnahme + Stream für alle).
- `POST /api/sim/start` — direkter Start; bricht einen offenen Vor-Start ab.
- `POST /api/sim/stop` — beendet die aktive Session. Läuft keine, aber ein Vor-Start, wird dieser abgebrochen (`{session_id, length_ms: 0, vod_file: null}`), sonst 404. Schaltet die Aufnahme danach ab (`record.enabled = false`).
- `POST /api/sim/stop_all` — beendet alle Sessions und einen offenen Vor-Start: `{ok, stopped_sessions, cancelled_prestart}`.
- `POST /api/sim/delete_all` — alle Sessions inkl. Aufnahmen löschen (wie `DELETE /api/sessions`).
- `POST /api/sim/switch_room` — Raum (Layout + Kameras + Viewer) während laufender Session live wechseln, Body `{"room_id": n}`; Pipeline läuft weiter, setzt Marker „Raumwechsel“ und sendet WS-Event `room_switched`.
- `GET/POST /api/participants` — Teilnehmer-Notizblätter auflisten / anlegen (`{name}`), liefert Token-Pfad `/tn/<token>`.
- `DELETE /api/participants/:id`, `POST /api/participants/:id/unlink`
- `POST /api/sim/debrief/start`
- `GET/POST /api/sim/debrief/position`
- `GET /api/sim/last_finished`
- `GET/DELETE /api/sessions`
- `DELETE /api/sessions/:id`
- `GET /api/sessions/:id/markers`
- `GET /api/sessions/:id/markers/export.json`
- `GET /api/sessions/:id/vod`
- `POST /api/markers`
- `DELETE/PATCH /api/markers/:id`
- `GET /api/marker_types`
- `POST /api/marker_types` — Body `{name, color, sort_index?, hotkey?}`; `hotkey` ist ein normalisiertes Tastenkürzel (z.B. `F13`, `Numpad1`, `Ctrl+Shift+Digit1`), bei Doppelbelegung 409.
- `PATCH/DELETE /api/marker_types/:id` — PATCH akzeptiert zusätzlich `hotkey` (weggelassen = unverändert, `""` = entfernen, sonst setzen; Doppelbelegung → 409). `GET /api/marker_types` liefert `hotkey` je Typ mit.
- `GET/POST /api/marker_notes`
- `GET /api/viewers`
- `POST /api/viewers`
- `PATCH/DELETE /api/viewers/:id`
- `POST /api/viewers/:id/cmd`
- `POST /api/viewers/:id/report_ack`
- `GET /api/users/:id/stats`
- `GET /api/users/:id/achievements`
- `DELETE /api/trivia/:id`

### Admin (Login + Rolle `admin`)
- `GET/POST /api/admin/settings` — u.a. `output_rtmp` (externer Verteiler, leer = aus), `audio_device`, `record_*`, `log_retention_days`, `log_mail_to`. (TCP-Port und Live-MP4 wurden 2026-10-21 entfernt.)
- `GET /api/ui/log_mail_to` — Empfängeradresse für Log-Mails.
- `POST /api/admin/invite_tokens`
- `GET /api/admin/users/stats`
- `GET/POST /api/users`
- `DELETE /api/users/:id`
- `GET /api/admin/update/list`
- `POST /api/admin/update/upload`
- `POST /api/admin/update/install`
- `GET /api/admin/logs/files`
- `GET /api/admin/logs/files/:name`
- `POST /api/admin/logs/cleanup`
- `GET /api/host/stats` — Systemwerte; `rtmp` = Zustand des externen Verteilers `{enabled, url (Streamkey maskiert), state: disabled|connecting|streaming|error, last_error, updated_at}`.
- `GET /api/host/audio_inputs`
- `GET /api/host/logs`
- `POST /api/host/reboot`
- `POST /api/host/poweroff`
- `POST /api/host/service/restart`

## WebSockets
- `GET /ws` — UI-Broadcast (öffentlich, nur Statusereignisse). Ereignisse u.a. `pre_start`, `pre_start_cancelled`, `sim_start_wait`, `sim_start_ready`, `sim_started`, `sim_stopped`, `room_switched`, `distributor_failed`, `record_failed`.
- `GET /ws/webrtc?token=…|ticket=…` — Live-Video (WebRTC-Signaling). Nur eingeloggt oder mit Video-Ticket; das Ticket legt der Server dem Viewer-Befehl `show_live` bei, es gilt nur für die laufende Session bzw. den laufenden Vor-Start. Sonst 401.
- `GET /ws/logs?token=…` — Live-Logs, nur Admin (sonst 401).
- `GET /ws/voice/tx?group=…&viewers=1&token=…` — nur eingeloggt (sonst 401). PTT-Sender (Browser-Mikrofon, WebRTC Offer/Answer/ICE, Status-Meldungen). `group` = Gruppenname oder `room:<id>` (Lautsprecher des Raums). Alt: `/ws/paging`.
- `GET /ws/voice/viewer?token=…|ticket=…` — nur eingeloggt oder mit Durchsage-Ticket (liegt `paging_start` bei, gilt nur für die laufende Durchsage). Viewer hören die Durchsage (WebRTC Audio, eigene Verbindung parallel zum Video). Viewer, die sich während einer Durchsage verbinden, bekommen sofort `paging_start`. Alt: `/ws/paging/viewer`.
- `GET /ws/voice/speaker` — öffentlich (ESP-Geräte ohne Login; ein Lautsprecher kann nur empfangen). Raumlautsprecher-Registrierung (`register`, `heartbeat`, `ack`; Server: `registered`, `set_volume`, `mute`, `test_tone`, `identify`, `reboot`, `stream`, `config`).

## Oberfläche (Login über die Seite selbst)
- `GET /ui` (Dashboard), `/ui/simulate`, `/ui/debrief`, `/ui/options`, `/ui/options/system`, `/ui/options/stats`, `/ui/options/achievements`, `/setup`
- `GET /ui/viewer` — Liveansicht für Ausbilder (WebRTC, ohne Viewer-Registrierung)
- `GET /ui/viewer_qr` — Zuschauer-QR-Assistent
- `GET /ui/camera-control` — PTZ-Steuerung (Tablet)

## Viewer/Trainer/Pager Seiten (öffentlich)
- `GET /viewer/:id`
- `GET /tn/:token` — Teilnehmer-Notizblatt (Formular, Bearbeiten)
- `GET /tn/:token/download` — Notizblatt in Druck-/Download-Ansicht (Ziel des QR-Codes)
- `GET /ui/participants` — Ausbilder-Seite zur Verwaltung der Notizblätter (Login)
- `GET /help/` — Statisches Hilfedokument (öffentlich)
- `GET /pager/:id`
