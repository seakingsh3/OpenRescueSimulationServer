# relay-onair – On-Air-Lampe für OpenRescueSim

ESP32-Relaisboard als WLAN-Client: fragt `GET /api/sim/status` am OpenRescueSim-Server
alle 3 Sekunden ab und schaltet ein Relais (z.B. eine „On-Air“-Lampe am Raumeingang)
ein, sobald eine Simulation läuft, und wieder aus, sobald sie beendet ist.

Der Status-Endpoint ist bewusst ohne Login erreichbar (siehe `is_public_api_path` in
`src/main.rs`) – damit braucht das Gerät kein Token-Handling, nur eine einfache
HTTP-Abfrage.

## Flashen

1. Arduino IDE installieren, darin über den Boards-Verwalter das Paket **"esp32" von
   Espressif Systems** hinzufügen (Datei → Voreinstellungen → zusätzliche
   Boardverwalter-URL: `https://raw.githubusercontent.com/espressif/arduino-esp32/gh-pages/package_esp32_index.json`).
2. `relay_onair.ino` öffnen, oben unter „Konfiguration“ WLAN-SSID/-Passwort,
   Server-Host:Port sowie `RELAY_PIN` (siehe Silkscreen auf dem Board) eintragen.
3. Passendes Board auswählen (z.B. „ESP32 Dev Module“, je nach konkretem
   Relaisboard), hochladen.
4. Seriellen Monitor (115200 Baud) öffnen – zeigt WLAN-Verbindung, jede Abfrage und
   Relais-Schaltvorgänge.

## Verhalten bei Störungen

- WLAN-Verbindung bricht ab → Gerät versucht automatisch neu zu verbinden
  (Status-LED blinkt währenddessen, falls `STATUS_LED_PIN` gesetzt ist).
- Server nicht erreichbar / Zeitüberschreitung → nach `MAX_CONSECUTIVE_FAILURES`
  (Standard: 3, also ~9s) fehlgeschlagenen Abfragen in Folge schaltet die Lampe
  sicherheitshalber **aus**, statt im letzten bekannten Zustand zu bleiben.

## Keine zusätzlichen Bibliotheken nötig

Nur `WiFi.h`/`HTTPClient.h` aus dem ESP32-Arduino-Core. Die JSON-Antwort wird per
einfacher Teilstring-Suche nach `"active":true`/`"active":false` ausgewertet (kein
ArduinoJson o.ä. nötig) – für das eine benötigte Feld ausreichend und robust genug,
solange sich der Feldname nicht ändert.
