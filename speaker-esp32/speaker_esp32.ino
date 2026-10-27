/*
 * speaker-esp32 – Raumlautsprecher für OpenRescueSim „Voice of Room“ (ESP32-C3)
 *
 * Getestet mit XIAO ESP32-C3 + MAX98357A. Meldet sich am Server an
 * (/ws/voice/speaker, Protokoll identisch zu speaker/ors-speaker.py), empfängt
 * Durchsagen als RTP/L16-PCM (16 kHz, mono, unkomprimiert) über UDP und spielt
 * sie über I2S am MAX98357A ab.
 *
 * WICHTIG: codec:"l16" statt "opus" – der ESP32-C3 hat keine Hardware-FPU und nur
 * einen Kern, Opus-Decoding in Echtzeit wäre riskant. Der Server schickt für
 * Geräte, die sich mit codec "l16" anmelden, stattdessen unkomprimiertes PCM
 * (eigener Pipeline-Zweig in src/voice.rs, seit 2026-10-07). Für dieses Gerät
 * ist daher KEIN Opus-Decoder nötig – nur rohe RTP-Pakete einlesen und an I2S
 * weiterreichen. Siehe Handover_ESP32.md im Projekt-Root für den Hintergrund.
 *
 * Board: Arduino-Core "ESP32" (Boards-Verwalter: "esp32" von Espressif), Board
 * "XIAO_ESP32C3" (oder generisch "ESP32C3 Dev Module").
 *
 * Zusätzliche Bibliotheken (über den Bibliotheksverwalter installieren):
 *   - "WebSockets" von Markus Sattler (arduinoWebSockets)
 *   - "ArduinoJson" von Benoit Blanchon (Version 6.x)
 */

#include <WiFi.h>
#include <WiFiUdp.h>
#include <WebSocketsClient.h>
#include <ArduinoJson.h>
#include <driver/i2s.h>
#include <math.h>

// ---------------------------------------------------------------------------
// Konfiguration – bitte an dein Board/Netz anpassen
// ---------------------------------------------------------------------------
#define WIFI_SSID        "DEIN-WLAN-SSID"
#define WIFI_PASSWORD    "DEIN-WLAN-PASSWORT"

// Server-Host und -Port getrennt (WebSocketsClient braucht das so).
#define SERVER_HOST      "192.168.1.10"
#define SERVER_PORT      8393

// Anzeigename/Gruppe beim ERSTEN Anmelden (danach in der Web-Oberfläche änderbar,
// wie beim Pi-Client – der Server merkt sich Name/Gruppe über die Geräte-ID).
#define SPEAKER_NAME     "Simraum ESP32"
#define SPEAKER_GROUP    "default"

// UDP-Port, auf dem dieses Gerät RTP/L16 empfängt.
#define RTP_PORT         5006

// I2S-Pins zum MAX98357A (BCLK, LRCLK/WS, DIN des Verstärkers = DOUT hier).
// Pin-Nummern am XIAO ESP32-C3 nach eigener Verdrahtung anpassen.
#define I2S_BCLK_PIN     7
#define I2S_LRCLK_PIN    8
#define I2S_DOUT_PIN     9

// Muss zum Server-seitigen L16-Zweig passen (src/voice.rs, L16_RATE).
#define AUDIO_RATE       16000

#define HEARTBEAT_MS     5000
// Nach so langer Funkstille vom Server gilt "nicht am Empfangen" (für Heartbeat).
#define PLAYING_TIMEOUT_MS 1000

// ---------------------------------------------------------------------------

WebSocketsClient wsClient;
WiFiUDP rtpUdp;

String deviceId;
String speakerName = SPEAKER_NAME;
String speakerGroup = SPEAKER_GROUP;
int volumePct = 80;
bool muted = false;
uint32_t rtpPackets = 0;
unsigned long lastPacketMs = 0;
unsigned long lastHeartbeatMs = 0;
bool wsRegistered = false;

// Testton-Zustand (nicht-blockierend über loop() generiert, siehe playToneSamples)
bool toneActive = false;
unsigned long toneEndMs = 0;
float toneFreq = 880.0f;
float tonePhase = 0.0f;

// ---------------------------------------------------------------------------
// I2S
// ---------------------------------------------------------------------------
void setupI2S() {
  i2s_config_t cfg = {
    .mode = (i2s_mode_t)(I2S_MODE_MASTER | I2S_MODE_TX),
    .sample_rate = AUDIO_RATE,
    .bits_per_sample = I2S_BITS_PER_SAMPLE_16BIT,
    .channel_format = I2S_CHANNEL_FMT_RIGHT_LEFT,
    .communication_format = I2S_COMM_FORMAT_STAND_I2S,
    .intr_alloc_flags = 0,
    .dma_buf_count = 6,
    .dma_buf_len = 256,
    .use_apll = false,
    .tx_desc_auto_clear = true,
  };
  i2s_driver_install(I2S_NUM_0, &cfg, 0, NULL);

  i2s_pin_config_t pins = {
    .bck_io_num = I2S_BCLK_PIN,
    .ws_io_num = I2S_LRCLK_PIN,
    .data_out_num = I2S_DOUT_PIN,
    .data_in_num = I2S_PIN_NO_CHANGE,
  };
  i2s_set_pin(I2S_NUM_0, &pins);
  i2s_zero_dma_buffer(I2S_NUM_0);
}

// Schreibt `count` Mono-Samples (bereits lautstärkeskaliert) als Stereo-Frames
// (L=R=Sample) an I2S – funktioniert unabhängig davon, wie die SD/GAIN-Pin-
// Beschaltung des MAX98357A den Kanal auswählt.
void i2sWriteMono(const int16_t *samples, size_t count) {
  static int16_t stereoBuf[512]; // 256 Frames * 2 Kanäle
  size_t i = 0;
  while (i < count) {
    size_t chunk = count - i;
    size_t maxFrames = sizeof(stereoBuf) / sizeof(stereoBuf[0]) / 2;
    if (chunk > maxFrames) chunk = maxFrames;
    for (size_t j = 0; j < chunk; j++) {
      stereoBuf[j * 2] = samples[i + j];
      stereoBuf[j * 2 + 1] = samples[i + j];
    }
    size_t bytesWritten = 0;
    i2s_write(I2S_NUM_0, stereoBuf, chunk * 2 * sizeof(int16_t), &bytesWritten, portMAX_DELAY);
    i += chunk;
  }
}

int16_t applyVolume(int16_t sample) {
  if (muted) return 0;
  // Perzeptiv angenehmere Kurve, wie im Pi-Client (volume/100)^2.
  float gain = (volumePct / 100.0f) * (volumePct / 100.0f);
  int32_t v = (int32_t)(sample * gain);
  if (v > 32767) v = 32767;
  if (v < -32768) v = -32768;
  return (int16_t)v;
}

// ---------------------------------------------------------------------------
// RTP/L16-Empfang
// ---------------------------------------------------------------------------
void handleRtpPacket(uint8_t *buf, int len) {
  if (len < 12) return; // zu kurz für einen RTP-Header

  uint8_t vpxcc = buf[0];
  uint8_t cc = vpxcc & 0x0F;
  bool hasExtension = (vpxcc & 0x10) != 0;
  int offset = 12 + cc * 4; // Standard-Header + ggf. CSRC-Liste

  if (hasExtension) {
    if (offset + 4 > len) return;
    uint16_t extLenWords = (buf[offset + 2] << 8) | buf[offset + 3];
    offset += 4 + extLenWords * 4;
  }
  if (offset >= len) return;

  int payloadBytes = len - offset;
  int sampleCount = payloadBytes / 2; // S16BE Mono
  if (sampleCount <= 0) return;

  static int16_t pcm[1024];
  int n = sampleCount;
  if (n > (int)(sizeof(pcm) / sizeof(pcm[0]))) n = sizeof(pcm) / sizeof(pcm[0]);

  for (int i = 0; i < n; i++) {
    int idx = offset + i * 2;
    int16_t be = (int16_t)((buf[idx] << 8) | buf[idx + 1]); // Netzwerk-Byte-Order -> int16
    pcm[i] = applyVolume(be);
  }
  i2sWriteMono(pcm, n);

  rtpPackets++;
  lastPacketMs = millis();
}

void pollRtp() {
  int packetSize = rtpUdp.parsePacket();
  if (packetSize <= 0) return;
  static uint8_t buf[1472]; // typische max. UDP-Nutzlast unter Ethernet-MTU
  int len = rtpUdp.read(buf, sizeof(buf));
  if (len > 0) handleRtpPacket(buf, len);
}

bool isPlaying() {
  return lastPacketMs != 0 && (millis() - lastPacketMs) < PLAYING_TIMEOUT_MS;
}

// ---------------------------------------------------------------------------
// Testton / Identifizieren (blockierend, kurz – siehe Aufrufer)
// ---------------------------------------------------------------------------
void playToneBlocking(float freqHz, float seconds, float volume) {
  static int16_t buf[256];
  int totalSamples = (int)(seconds * AUDIO_RATE);
  float phase = 0.0f;
  float step = 2.0f * PI * freqHz / AUDIO_RATE;
  int done = 0;
  while (done < totalSamples) {
    int n = totalSamples - done;
    if (n > (int)(sizeof(buf) / sizeof(buf[0]))) n = sizeof(buf) / sizeof(buf[0]);
    for (int i = 0; i < n; i++) {
      buf[i] = (int16_t)(sinf(phase) * 32767.0f * volume);
      phase += step;
      if (phase > 2.0f * PI) phase -= 2.0f * PI;
    }
    i2sWriteMono(buf, n);
    done += n;
    wsClient.loop(); // WS-Verbindung während des Tons nicht verhungern lassen
  }
}

// ---------------------------------------------------------------------------
// WebSocket: Steuerkanal
// ---------------------------------------------------------------------------
void sendJson(JsonDocument &doc) {
  String out;
  serializeJson(doc, out);
  wsClient.sendTXT(out);
}

void sendRegister() {
  StaticJsonDocument<256> doc;
  doc["type"] = "register";
  doc["device_id"] = deviceId;
  doc["name"] = speakerName;
  doc["group"] = speakerGroup;
  doc["rtp_port"] = RTP_PORT;
  doc["codec"] = "l16";
  doc["version"] = "1.0.0-esp32c3";
  sendJson(doc);
}

void sendHeartbeat() {
  StaticJsonDocument<256> doc;
  doc["type"] = "heartbeat";
  doc["playing"] = isPlaying();
  doc["rms_db"] = -20.0; // keine Pegelmessung auf dem C3, Platzhalter
  doc["volume"] = volumePct;
  doc["muted"] = muted;
  doc["uptime_s"] = millis() / 1000;
  doc["rtp_packets"] = rtpPackets;
  sendJson(doc);
}

void sendAck(const char *cmd, bool ok, const char *error = nullptr) {
  StaticJsonDocument<192> doc;
  doc["type"] = "ack";
  doc["cmd"] = cmd;
  doc["ok"] = ok;
  if (error) doc["error"] = error;
  sendJson(doc);
}

void handleServerMessage(const char *payload, size_t len) {
  StaticJsonDocument<512> doc;
  DeserializationError err = deserializeJson(doc, payload, len);
  if (err) return;

  const char *type = doc["type"] | "";

  if (strcmp(type, "registered") == 0) {
    wsRegistered = true;
    if (doc.containsKey("name")) speakerName = doc["name"].as<String>();
    if (doc.containsKey("group")) speakerGroup = doc["group"].as<String>();
    if (doc.containsKey("volume")) volumePct = doc["volume"];
    if (doc.containsKey("muted")) muted = doc["muted"];
    Serial.printf("[ws] registriert als \"%s\" (Gruppe %s), Lautstaerke %d%%\n",
                  speakerName.c_str(), speakerGroup.c_str(), volumePct);
  } else if (strcmp(type, "config") == 0) {
    if (doc.containsKey("name")) speakerName = doc["name"].as<String>();
    if (doc.containsKey("group")) speakerGroup = doc["group"].as<String>();
    sendAck("config", true);
  } else if (strcmp(type, "set_volume") == 0) {
    volumePct = constrain((int)(doc["volume"] | volumePct), 0, 100);
    sendAck("set_volume", true);
  } else if (strcmp(type, "mute") == 0) {
    muted = doc["on"] | true;
    sendAck("mute", true);
  } else if (strcmp(type, "test_tone") == 0) {
    float seconds = doc["seconds"] | 1.5f;
    playToneBlocking(880.0f, seconds, 0.35f);
    sendAck("test_tone", true);
  } else if (strcmp(type, "identify") == 0) {
    float seconds = doc["seconds"] | 5.0f;
    unsigned long end = millis() + (unsigned long)(seconds * 1000);
    while (millis() < end) {
      playToneBlocking(1320.0f, 0.12f, 0.35f);
      delay(400);
      wsClient.loop();
    }
    sendAck("identify", true);
  } else if (strcmp(type, "reboot") == 0) {
    sendAck("reboot", true);
    Serial.println("[ws] Neustart auf Anforderung des Servers");
    delay(200);
    ESP.restart();
  } else if (strcmp(type, "deleted") == 0) {
    Serial.println("[ws] Vom Server entfernt, verbinde in 30s neu");
    wsRegistered = false;
    delay(30000);
  } else if (strcmp(type, "replaced") == 0) {
    Serial.println("[ws] Durch neue Instanz ersetzt, trenne Verbindung");
    wsClient.disconnect();
  }
  // "stream" und "ping" brauchen keine eigene Behandlung – PCM kommt direkt per
  // RTP/UDP, nicht über den WS-Kanal.
}

void onWsEvent(WStype_t type, uint8_t *payload, size_t len) {
  switch (type) {
    case WStype_CONNECTED:
      Serial.println("[ws] verbunden, melde an...");
      sendRegister();
      break;
    case WStype_DISCONNECTED:
      Serial.println("[ws] getrennt");
      wsRegistered = false;
      break;
    case WStype_TEXT:
      handleServerMessage((const char *)payload, len);
      break;
    default:
      break;
  }
}

// ---------------------------------------------------------------------------
void connectWifi() {
  Serial.printf("[wifi] Verbinde mit \"%s\" ...\n", WIFI_SSID);
  WiFi.mode(WIFI_STA);
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
  unsigned long start = millis();
  while (WiFi.status() != WL_CONNECTED) {
    delay(200);
    if (millis() - start > 20000) {
      Serial.println("[wifi] Timeout, versuche neu...");
      WiFi.disconnect(true);
      delay(500);
      WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
      start = millis();
    }
  }
  Serial.printf("[wifi] Verbunden, IP %s\n", WiFi.localIP().toString().c_str());
}

void setup() {
  Serial.begin(115200);
  delay(200);
  Serial.println("\n[boot] speaker-esp32 startet");

  deviceId = "esp32-" + WiFi.macAddress();
  deviceId.replace(":", "");

  setupI2S();
  connectWifi();

  rtpUdp.begin(RTP_PORT);
  Serial.printf("[rtp] lausche auf UDP-Port %d\n", RTP_PORT);

  wsClient.begin(SERVER_HOST, SERVER_PORT, "/ws/voice/speaker");
  wsClient.onEvent(onWsEvent);
  wsClient.setReconnectInterval(3000);
}

void loop() {
  if (WiFi.status() != WL_CONNECTED) {
    connectWifi();
  }
  wsClient.loop();
  pollRtp();

  unsigned long now = millis();
  if (wsRegistered && now - lastHeartbeatMs >= HEARTBEAT_MS) {
    lastHeartbeatMs = now;
    sendHeartbeat();
  }
}
