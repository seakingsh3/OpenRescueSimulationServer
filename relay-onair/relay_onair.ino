/*
 * relay-onair – „On-Air“-Lampe für OpenRescueSim
 *
 * Läuft auf einem ESP32-Relaisboard. Verbindet sich als WLAN-Client mit dem
 * Simulations-WLAN, fragt den Server regelmäßig per HTTP-GET, ob gerade eine
 * Übung läuft (GET /api/sim/status, kein Login nötig), und schaltet darüber
 * ein Relais (On-Air-Lampe o.ä.) ein bzw. aus.
 *
 * Board: Arduino-Core "ESP32" (Boards-Verwalter: "esp32" von Espressif).
 * Zusätzliche Bibliotheken: keine – nur WiFi.h und HTTPClient.h aus dem
 * ESP32-Core. Die JSON-Antwort wird bewusst NICHT mit einer JSON-Bibliothek
 * geparst, sondern per einfacher Teilstring-Suche nach `"active":true` bzw.
 * `"active":false` ausgewertet – das reicht für dieses eine Feld und spart
 * eine Zusatzabhängigkeit (ArduinoJson). Bei Bedarf später leicht ersetzbar.
 */

#include <WiFi.h>
#include <HTTPClient.h>

// ---------------------------------------------------------------------------
// Konfiguration – bitte an dein Board/Netz anpassen
// ---------------------------------------------------------------------------
#define WIFI_SSID        "DEIN-WLAN-SSID"
#define WIFI_PASSWORD    "DEIN-WLAN-PASSWORT"

// Server-Host:Port ohne "http://" (z.B. "192.168.1.10:8393")
#define SERVER_HOST      "192.168.1.10:8393"

// GPIO, an dem das Relais hängt (Silkscreen auf dem Board prüfen).
#define RELAY_PIN        26
// Viele Billig-Relaisboards schalten "active low" (LOW = Relais an).
// Falls die Lampe genau andersrum reagiert, hier auf false stellen.
#define RELAY_ACTIVE_LOW true

// Eingebaute Status-LED (optional, -1 = deaktiviert). Blinkt beim Verbinden,
// leuchtet dauerhaft sobald WLAN+Server erreichbar sind.
#define STATUS_LED_PIN   2

// Abfrageintervall in Millisekunden.
#define POLL_INTERVAL_MS 3000
// HTTP-Timeout pro Abfrage.
#define HTTP_TIMEOUT_MS  2000
// Nach so vielen FEHLGESCHLAGENEN Abfragen in Folge wird die Lampe sicherheitshalber
// ausgeschaltet (lieber "aus" bei unklarem Status als fälschlich "on air" anzeigen).
#define MAX_CONSECUTIVE_FAILURES 3

// ---------------------------------------------------------------------------

WiFiClient wifiClient;
int consecutiveFailures = 0;
bool relayOn = false;

void setRelay(bool on) {
  if (on == relayOn) return;
  relayOn = on;
  bool level = RELAY_ACTIVE_LOW ? !on : on;
  digitalWrite(RELAY_PIN, level ? HIGH : LOW);
  Serial.printf("[relay] On-Air-Lampe %s\n", on ? "EIN" : "AUS");
}

void setStatusLed(bool on) {
  if (STATUS_LED_PIN < 0) return;
  digitalWrite(STATUS_LED_PIN, on ? HIGH : LOW);
}

void connectWifi() {
  Serial.printf("[wifi] Verbinde mit \"%s\" ...\n", WIFI_SSID);
  WiFi.mode(WIFI_STA);
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);

  unsigned long start = millis();
  while (WiFi.status() != WL_CONNECTED) {
    if (STATUS_LED_PIN >= 0) {
      setStatusLed((millis() / 250) % 2 == 0);
    }
    delay(100);
    if (millis() - start > 20000) {
      Serial.println("[wifi] Timeout, versuche neu ...");
      WiFi.disconnect(true);
      delay(500);
      WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
      start = millis();
    }
  }
  Serial.printf("[wifi] Verbunden, IP %s\n", WiFi.localIP().toString().c_str());
  setStatusLed(true);
}

// Liefert true/false über *active, false als Rückgabewert = Abfrage fehlgeschlagen
// (Netzwerkfehler, Server nicht erreichbar, unerwartete Antwort).
bool fetchSimActive(bool *active) {
  if (WiFi.status() != WL_CONNECTED) return false;

  HTTPClient http;
  String url = String("http://") + SERVER_HOST + "/api/sim/status";
  http.setTimeout(HTTP_TIMEOUT_MS);
  if (!http.begin(wifiClient, url)) return false;

  int code = http.GET();
  if (code != HTTP_CODE_OK) {
    Serial.printf("[http] GET %s -> %d\n", url.c_str(), code);
    http.end();
    return false;
  }

  String body = http.getString();
  http.end();

  if (body.indexOf("\"active\":true") >= 0) {
    *active = true;
    return true;
  }
  if (body.indexOf("\"active\":false") >= 0) {
    *active = false;
    return true;
  }
  Serial.println("[http] Unerwartete Antwort, \"active\" nicht gefunden");
  return false;
}

void setup() {
  Serial.begin(115200);
  delay(200);
  Serial.println("\n[boot] relay-onair startet");

  pinMode(RELAY_PIN, OUTPUT);
  setRelay(false);

  if (STATUS_LED_PIN >= 0) {
    pinMode(STATUS_LED_PIN, OUTPUT);
    setStatusLed(false);
  }

  connectWifi();
}

void loop() {
  if (WiFi.status() != WL_CONNECTED) {
    setStatusLed(false);
    connectWifi();
  }

  bool active = false;
  if (fetchSimActive(&active)) {
    consecutiveFailures = 0;
    setRelay(active);
  } else {
    consecutiveFailures++;
    Serial.printf("[poll] Fehlschlag %d/%d\n", consecutiveFailures, MAX_CONSECUTIVE_FAILURES);
    if (consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) {
      setRelay(false);
    }
  }

  delay(POLL_INTERVAL_MS);
}
