# Nutzerinfo – Wichtige Änderungen in der Nutzung

Stand: 2026-02-18

## Was hat sich geändert?

- Viele Funktionen sind jetzt an Login gebunden (Sicherheit):
  - Simulation starten/stoppen
  - Marker setzen/anzeigen
  - Sessions, Debrief-Daten, Räume, Kamera-/Systemfunktionen
- Viewer- und öffentliche Inhalte bleiben erreichbar:
  - Zuschauer-Viewer
  - Trivia/Nerdfacts/Ticker
  - QR-Code-Funktionen für Zuschauerlinks
- Bei fehlendem Login erscheint oben in der Session-Statusleiste ein roter Hinweis mit **Einloggen**-Button.


## Neuer Nutzeraccount per Token (Self-Register)
### Für Nutzer (mit Token registrieren)
1. Öffne **Optionen → Einstellungen** und dort den Reiter **Benutzer & Zugang**.
2. Im Bereich **Anmeldung per Token**:
   - Token eintragen
   - Benutzername wählen
   - Passwort setzen
3. Auf **Registrieren** klicken.
4. Danach normal einloggen.

## Kurz gesagt

- Mehr Sicherheit durch Login-Pflicht für interne Daten.
- Öffentliche Viewer-Funktionen bleiben für Zuschauer nutzbar.
- Einfachere Bedienung durch bessere Hinweise und klareren QR-Ablauf.

## Session-Dateien: Zugriff & Aufbewahrung

### Wer sieht welche Sessions?
- **Nicht eingeloggt:** keine internen Session-Listen/Marker/VOD-Daten.
- **Eingeloggte Nutzer (user):**
  - aktive Session(s)
  - beendete Sessions der letzten **24 Stunden**
- **Admins:**
  - alle Sessions (auch älter als 24h)

### Zugriff auf Session-Dateien (Snapshot/VOD)
- Snapshot/VOD folgen derselben Sichtbarkeit wie die Session selbst.
- Bedeutet: ältere Sessions sind für normale Nutzer ausgeblendet, für Admins weiterhin erreichbar.

### Was passiert beim Löschen einer Session?
- Beim Session-Löschen werden nicht nur DB-Einträge entfernt, sondern auch die zugehörigen Dateien:
  - Session-Ordner unter `recordings`
  - zugehörige Aufnahme-Dateien
  - zugehörige VOD-Dateien
- Danach ist die Session inklusive Marker/Dateien nicht mehr verfügbar.

## Marker per Taste setzen (auch mit eigener Tastatur)

Jedem Marker-Typ kann eine Taste zugewiesen werden. Ein Tastendruck setzt dann sofort einen Marker dieses Typs, ohne Klick.

### Taste zuweisen
1. In **Simulieren** im Marker-Bereich auf ⚙️ (Marker-Typen verwalten) klicken.
2. Beim gewünschten Typ (z.B. **Papier**) auf **⌨️ Taste zuweisen** klicken.
3. Die Taste (oder Tastenkombination) drücken. **Esc** bricht ab, **Entf** entfernt eine Taste.
4. Alternativ: **1–9 automatisch belegen** verteilt die Ziffern auf alle Typen ohne Taste.

### Hinweise für eine eigene Makro-Tastatur
- Empfohlen sind die Tasten **F13 bis F24**: Sie kollidieren nicht mit Browser-Kürzeln und wirken auch, wenn gerade ein Eingabefeld (z.B. Freitext) aktiv ist.
- Alle anderen Tasten wirken nur, wenn kein Eingabefeld fokussiert ist.
- Erkannt wird die physische Taste (Scancode), nicht das Zeichen. Eine Makro-Tastatur muss also einfach den passenden Tastencode senden, das Tastaturlayout ist egal.
- Die Zuordnung ist serverseitig gespeichert und gilt in jedem Browser. Persönliche Marker-Typen (nach Login) haben eigene Tasten.
- Bei laufender Session erscheint nach dem Tastendruck ein Toast „Marker … gesetzt“. Ohne aktive Session kommt der bekannte Hinweis „Keine Session“.

## Teilnehmer-Notizblätter (ersetzt den Ausbilder-Viewer)

Teilnehmer können während der Simulation auf dem eigenen Handy oder Tablet ein Notizblatt im AMLS-Formular-Layout ausfüllen.

### Für Ausbilder
1. Menü **Teilnehmer** öffnen, Namen eingeben und **Anlegen** klicken.
2. Den **QR-Code** zeigen oder den **Link kopieren** und weitergeben. Der Link funktioniert ohne Login.
3. In der Liste siehst du, ob ein Blatt mit einer Session verknüpft ist. Über **Lösen** trennst du die Verknüpfung, über **Löschen** wird das Blatt samt Link ungültig.
4. Verknüpfte Einträge erscheinen live in der Marker-Liste unter dem Nutzer **Teilnehmer**, z.B. „Max: A Atemweg: frei“.

### Für Teilnehmer
- Einfach ausfüllen. Gespeichert wird automatisch, sobald ein Feld verlassen wird.
- Startet der Ausbilder eine Simulation, erscheint ein Popup: **Ja, verknüpfen** schickt ab jetzt jede Notiz als Marker an den Ausbilder. **Nein** lässt das Blatt privat. Die Frage kommt pro Simulation nur einmal.
- **QR-Code zum Download** zeigt einen Code, der zur Druck-Ansicht des ausgefüllten Blattes führt. Dort kann es gedruckt, als PDF oder als HTML-Datei gespeichert werden.
- **Zurücksetzen** löscht alle Eingaben nach Rückfrage. Bereits gesetzte Marker bleiben erhalten.

## Optionen neu sortiert
- Der **Systemstatus** steht jetzt an erster Stelle: im Menü unter Optionen, in der Leiste oben auf jeder Optionen-Seite und über die Dashboard-Karte „System & Einstellungen“.
- Die Seite **Einstellungen** ist in vier Reiter gegliedert: **Kameras & Räume**, **Anzeige & Marker**, **Aufnahme & Ausgabe** (Admin) und **Benutzer & Zugang**. Der zuletzt gewählte Reiter wird gemerkt.
- Alle Funktionen sind unverändert vorhanden, nur anders gruppiert.

## Durchsagen in den Raum (Voice of Room)

- Im Raum stehen kleine Lautsprechergeräte (Raspberry Pi), die sich selbst am Server anmelden. Unter **Einstellungen → Kameras & Räume → Raumlautsprecher** siehst du sie mit Online-Status, kannst Name, Gruppe und Lautstärke setzen, einen Testton abspielen oder das Gerät piepen lassen.
- Zum Sprechen den blauen **PTT**-Knopf am rechten Rand drücken, Gruppe wählen und die große Taste **gedrückt halten** (oder auf „Umschalten“ stellen und tippen). Die Statuszeile zeigt, wie viele Lautsprecher gerade empfangen.
- „Auch an Viewer senden“ lässt zusätzlich die Zuschauer-Viewer mithören.
- Das Mikrofon funktioniert im Browser nur über HTTPS oder localhost (siehe Installscript, Abschnitt HTTPS).

## Mehrkanal-Audio-Interfaces (z.B. Scarlett 18i20)
- Der Server öffnet Interfaces jetzt mit allen Kanälen und mischt selbst. Bisher kamen bei solchen Geräten nur Eingang 1 und 2 an.
- In **Einstellungen → Kameras & Räume → Räume → Audio** hat jeder Audio-Eingang ein Feld **Kanäle**: leer = alle Kanäle zusammen (Mono), `3` = nur Eingang 3, `1,2` = Stereo aus Eingang 1 links und 2 rechts, `1-8` = Eingänge 1 bis 8 gemischt. Daneben steht, wie viele Kanäle das Gerät hat.

## HTTPS für Durchsagen
- Der Server bietet HTTPS auf Port 8443, sobald der Admin `scripts/setup_https.sh` ausgeführt hat.
- Sprech-Geräte (iPad, PC) einmalig vorbereiten: Wurzelzertifikat von `http://<server>:8393/static/ca/ors-rootCA.crt` installieren und vertrauen, dann `https://<server>.local:8443/ui` nutzen.
- Alle anderen Geräte bleiben bei den bekannten HTTP-Adressen.

## Marker an der Seite anheften
- Bei laufender Übung kannst du die Liste **Gesetzte Marker** neben die Steuerung anheften, statt sie im Marker-Bereich einzubetten: Schalter **📌 An der Seite** neben „Gesetzte Marker“.
- Die Liste erscheint dann rechts, bleibt beim Scrollen sichtbar und zeigt genau dieselben Marker wie gewohnt, inklusive Löschen.
- **✕ Lösen** in der Seitenleiste bettet die Liste wieder ein. Die Wahl wird im Browser gemerkt.
- Auf schmalen Bildschirmen (Tablet-Hochformat) erscheint die Seitenleiste als Block weiter unten statt daneben, da dort kein Platz für zwei Spalten ist.
