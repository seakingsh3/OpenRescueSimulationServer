# speaker-esp32 – Raumlautsprecher für OpenRescueSim (XIAO ESP32-C3 + MAX98357A)

ESP32-C3 als WLAN-Lautsprecher für „Voice of Room": meldet sich wie der Raspberry-Pi-Client
(`speaker/ors-speaker.py`) am Server an, empfängt Durchsagen und spielt sie ab – aber über
unkomprimiertes RTP/L16-PCM (16 kHz, mono) statt Opus, weil der ESP32-C3 keine Hardware-FPU hat
und Opus-Decoding in Echtzeit auf dieser Hardware riskant wäre. Der Server erzeugt dafür einen
eigenen L16-Zweig (`src/voice.rs`).

**Status**: Firmware vollständig geschrieben, Server-seitiger L16-Zweig in `src/voice.rs`
implementiert und mit einem simulierten Lautsprecher + GStreamer-Mitschnitt gegen den echten
Server verifiziert (korrektes 16 kHz/Mono/16-Bit-PCM, Dauer passte exakt zur PTT-Haltezeit).
**Nicht verifiziert**: die Firmware selbst auf echter ESP32-C3-Hardware – kein Toolchain in dieser
Umgebung vorhanden, nur Code-Review.

## Hardware-Verdrahtung

MAX98357A an den XIAO ESP32-C3 (I2S, 3 Leitungen + Stromversorgung):

| MAX98357A | ESP32-C3 (Beispiel, siehe `I2S_*_PIN` im Sketch) |
|---|---|
| BCLK | GPIO 7 |
| LRCLK (WS) | GPIO 8 |
| DIN | GPIO 9 |
| GND | GND |
| VIN | 3V3 oder 5V (siehe Datenblatt deines Boards) |

Die genauen GPIO-Nummern oben im Sketch (`I2S_BCLK_PIN`, `I2S_LRCLK_PIN`, `I2S_DOUT_PIN`) an deine
tatsächliche Verdrahtung anpassen – die hier genannten sind nur ein Vorschlag, auf dem XIAO
ESP32-C3 frei wählbar unter den verfügbaren GPIOs.

## Flashen

1. Arduino IDE installieren, Boards-Verwalter-URL für ESP32 hinzufügen (wie bei `relay-onair`,
   siehe dessen README), Board **"XIAO_ESP32C3"** (oder generisch "ESP32C3 Dev Module") wählen.
2. Über den Bibliotheksverwalter installieren:
   - **"WebSockets" von Markus Sattler** (arduinoWebSockets)
   - **"ArduinoJson" von Benoit Blanchon**, Version 6.x (nicht 7.x – der Sketch nutzt die
     `StaticJsonDocument`-API aus Version 6)
3. `speaker_esp32.ino` öffnen, oben unter „Konfiguration" WLAN-SSID/-Passwort, `SERVER_HOST`/
   `SERVER_PORT`, die I2S-Pins und ggf. Name/Gruppe eintragen.
4. Hochladen, seriellen Monitor (115200 Baud) öffnen – zeigt WLAN-/WebSocket-Verbindung,
   Registrierung und laufende RTP-Paketzahl.
5. Im Server unter Optionen → Kameras & Räume → Raumlautsprecher sollte das Gerät nach dem Start
   erscheinen (Name "Simraum ESP32" bzw. der von dir gesetzte Name).
6. Test: PTT-Knopf im Browser gedrückt halten (Gruppe "default", bzw. die von dir gesetzte
   Gruppe) und sprechen – sollte mit ca. 150–300 ms Verzögerung aus dem Lautsprecher kommen.

## Bekannte Stolpersteine, auf die beim ersten Test zu achten ist

- **I2S-Treiber-API**: Der Sketch nutzt die klassische ESP-IDF-`driver/i2s.h`-API
  (`i2s_driver_install`, `i2s_write`). Neuere `arduino-esp32`-Core-Versionen (3.x) markieren das
  teils als "legacy" zugunsten einer neuen `ESP_I2S`-Klasse – sollte weiterhin funktionieren,
  ggf. gibt es beim Kompilieren eine Deprecation-Warnung (keine Fehlermeldung). Falls doch ein
  Fehler kommt: in den Core-Release-Notes nach dem aktuellen I2S-API-Stand schauen.
- **MAX98357A SD/Gain-Pin**: Der Sketch schreibt denselben Mono-Sample auf L und R (Stereo-Frame
  mit identischen Kanälen) – das funktioniert unabhängig davon, wie der SD-Pin des MAX98357A
  beschaltet ist (L, R oder L+R-Mittelwert).
- **ArduinoJson-Version**: Explizit 6.x installieren. Mit 7.x ändert sich die
  `StaticJsonDocument`-API, der Sketch kompiliert dann nicht ohne Anpassung.
- **RTP-Port-Kollision**: `RTP_PORT` (Standard 5006) darf sich nicht mit anderen Geräten in
  derselben Gruppe überschneiden, falls mehrere ESP32-Lautsprecher laufen – jedes Gerät braucht
  einen eigenen Port (wie beim Pi-Client).

## Protokoll-Referenz

Identisch zu `speaker/ors-speaker.py` (siehe dort und `Voice_of_Room_Konzept.md` Abschnitt 5),
mit dem einen Unterschied `codec:"l16"` statt `"opus"` bei der Registrierung. Steuerbefehle
(`set_volume`, `mute`, `test_tone`, `identify`, `reboot`, `config`) funktionieren identisch zum
Pi-Client – im Server/Frontend gibt es keinen Unterschied zwischen den Gerätetypen.
