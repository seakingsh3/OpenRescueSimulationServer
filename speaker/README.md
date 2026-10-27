# ors-speaker – Raumlautsprecher für „Voice of Room“

Empfängerseite der Durchsagefunktion: Ein Raspberry Pi im Übungsraum spielt ab, was der Ausbilder per
Push-to-Talk im Browser spricht. Das Gerät meldet sich selbst am OpenRescueSim-Server an; feste IP-Adressen
sind nicht nötig.

## Hardware

| Teil | Empfehlung |
|---|---|
| Rechner | Raspberry Pi Zero 2 W (oder jeder andere Pi ab 3) |
| Strom + Netz | PoE-HAT mit Ethernet-Buchse für den Zero (z. B. Waveshare „PoE HAT (Zero)“). Alternativ WLAN + USB-Netzteil. |
| Ton | I²S-Verstärker-HAT (MAX98357A, HiFiBerry MiniAmp) mit 3–5-W-Lautsprecher, oder USB-Soundkarte + Aktivlautsprecher |
| Optional | LED an einem GPIO (BCM-Nummer in `led_gpio`) als Verbindungsanzeige |

Der Pi Zero hat keinen Klinkenausgang, deshalb ist ein I²S-DAC oder eine USB-Soundkarte Pflicht.
Für I²S-HATs den Overlay in `/boot/firmware/config.txt` aktivieren (z. B. `dtoverlay=hifiberry-dac` oder
`dtoverlay=max98357a`), danach `aplay -l` prüfen.

## Installation

Auf dem Pi (Raspberry Pi OS Lite, Bookworm oder neuer):

```bash
git clone <repo> && cd openresquesim_server/speaker
sudo ./install_speaker.sh 192.168.1.10:8393 "Simraum 1" simraum default
```

Parameter: Server `host:port`, Anzeigename, Gruppe, ALSA-Gerät. Das Skript installiert die Pakete, kopiert den
Client nach `/opt/ors-speaker`, legt `/etc/ors-speaker.conf` an und startet den Dienst `ors-speaker`.

Danach erscheint das Gerät in OpenRescueSim unter **Einstellungen → Kameras & Räume → Raumlautsprecher**.
Dort lassen sich Name, Gruppe und Lautstärke pflegen, ein Testton abspielen und das Gerät identifizieren.

## Betrieb

```bash
systemctl status ors-speaker      # Zustand
journalctl -u ors-speaker -f      # Live-Log
sudo systemctl restart ors-speaker
```

- **Gruppen**: Der Ausbilder wählt im PTT-Fenster die Gruppe. Alle Online-Lautsprecher dieser Gruppe empfangen.
- **Lautstärke** und **Stumm** werden vom Server gesetzt und lokal in `/var/lib/ors-speaker/state.json` gemerkt.
- **Testton** und **Identifizieren** (Piepfolge, LED blinkt) kommen aus der Web-Oberfläche.
- **Neustart** aus der Oberfläche ist erlaubt, solange `allow_reboot = yes` steht.

## Technik

- Registrierung und Steuerung: WebSocket `ws://server:8393/ws/voice/speaker` (JSON).
- Audio: RTP/Opus, 48 kHz mono, 48 kbit/s, Payload-Typ 111, UDP-Port 5004 (änderbar). Der Server schickt nur an
  Geräte, die gerade online und in der gewählten Gruppe sind.
- Pipeline: `udpsrc → rtpjitterbuffer(60 ms) → rtpopusdepay → opusdec → audiomixer → volume → alsasink`.
  Ein stummer Testton-Generator am Mixer hält den Ausgang dauerhaft aktiv, damit die erste Silbe nicht verschluckt wird.
- Erwartete Verzögerung Mikrofon → Lautsprecher: 150 bis 250 ms im LAN.

## Fehlersuche

| Symptom | Prüfen |
|---|---|
| Gerät erscheint nicht in der Liste | `journalctl -u ors-speaker -f`: Server-Adresse in `/etc/ors-speaker.conf`, Netzwerk, Port 8393 erreichbar |
| Online, aber kein Ton | `aplay -l` und `alsa_device`; mit `speaker-test -c1 -t sine` testen; Testton aus der Oberfläche |
| „empfängt“ fehlt während der Durchsage | Firewall auf dem Pi (UDP 5004 eingehend), Gruppe stimmt mit PTT-Auswahl überein |
| Knacken/Aussetzer | `jitter_ms` erhöhen (z. B. 120), WLAN durch PoE/Ethernet ersetzen |
