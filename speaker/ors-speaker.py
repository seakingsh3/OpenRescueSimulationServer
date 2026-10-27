#!/usr/bin/env python3
"""
ors-speaker – Raumlautsprecher-Client für OpenRescueSim „Voice of Room“

Läuft auf einem Raspberry Pi (Zero 2 W o. ä.) und
  1. meldet sich per WebSocket beim Server an (/ws/voice/speaker),
  2. empfängt Durchsagen als RTP/Opus über UDP und spielt sie über ALSA ab,
  3. schickt Heartbeats (Pegel, Status) und führt Kommandos aus
     (Lautstärke, Stumm, Testton, Identifizieren, Neustart).

Abhängigkeiten (Raspberry Pi OS / Debian):
  python3-gi gir1.2-gstreamer-1.0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good
  gstreamer1.0-plugins-bad gstreamer1.0-alsa python3-websockets alsa-utils

Konfiguration: /etc/ors-speaker.conf (siehe ors-speaker.conf.example)
"""

import asyncio
import configparser
import json
import logging
import math
import os
import signal
import socket
import subprocess
import sys
import threading
import time
import uuid

import gi

gi.require_version("Gst", "1.0")
from gi.repository import GLib, Gst  # noqa: E402

try:
    import websockets
except ImportError:  # pragma: no cover
    print("Fehlt: python3-websockets (sudo apt install python3-websockets)", file=sys.stderr)
    sys.exit(1)

VERSION = "1.0.0"
CONFIG_PATHS = [
    os.environ.get("ORS_SPEAKER_CONF", ""),
    "/etc/ors-speaker.conf",
    os.path.expanduser("~/.config/ors-speaker.conf"),
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "ors-speaker.conf"),
]
STATE_PATH = "/var/lib/ors-speaker/state.json"

log = logging.getLogger("ors-speaker")


# ----------------------------------------------------------------------------
# Konfiguration
# ----------------------------------------------------------------------------
def load_config():
    cfg = configparser.ConfigParser()
    cfg["speaker"] = {
        "server": os.environ.get("ORS_SERVER", "192.168.1.10:8393"),
        "name": socket.gethostname(),
        "group": "default",
        "rtp_port": "5004",
        "alsa_device": "default",
        "volume": "80",
        "heartbeat_s": "5",
        "jitter_ms": "60",
        "led_gpio": "",
        "allow_reboot": "yes",
        "tls": "no",
    }
    for p in CONFIG_PATHS:
        if p and os.path.isfile(p):
            cfg.read(p, encoding="utf-8")
            log.info("Konfiguration: %s", p)
            break
    return cfg["speaker"]


def device_id():
    """Stabile Geräte-ID: machine-id, sonst MAC."""
    try:
        with open("/etc/machine-id", "r", encoding="utf-8") as f:
            mid = f.read().strip()
            if len(mid) >= 16:
                return mid[:16]
    except OSError:
        pass
    return f"{uuid.getnode():012x}"


def load_state():
    try:
        with open(STATE_PATH, "r", encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_state(state):
    try:
        os.makedirs(os.path.dirname(STATE_PATH), exist_ok=True)
        with open(STATE_PATH, "w", encoding="utf-8") as f:
            json.dump(state, f)
    except OSError as e:
        log.debug("State nicht gespeichert: %s", e)


def local_ip(server_host):
    """Eigene IP in Richtung Server (für den Fall, dass der Server sie nicht ableiten kann)."""
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect((server_host, 9))
        ip = s.getsockname()[0]
        s.close()
        return ip
    except OSError:
        return None


# ----------------------------------------------------------------------------
# Optionale Status-LED (GPIO), tolerant gegenüber fehlender Bibliothek
# ----------------------------------------------------------------------------
class Led:
    def __init__(self, pin):
        self.led = None
        if not pin:
            return
        try:
            from gpiozero import LED  # type: ignore

            self.led = LED(int(pin))
        except Exception as e:  # pragma: no cover
            log.info("LED deaktiviert (%s)", e)

    def on(self):
        if self.led:
            self.led.on()

    def off(self):
        if self.led:
            self.led.off()

    def blink(self, seconds):
        if self.led:
            self.led.blink(on_time=0.15, off_time=0.15, n=int(seconds / 0.3), background=True)


# ----------------------------------------------------------------------------
# Audio-Pipeline
#
#   udpsrc(RTP/Opus) → jitterbuffer → depay → opusdec → convert → queue ┐
#   audiotestsrc (Testton, normal stumm) → convert ─────────────────────┤→ audiomixer → volume → level → alsasink
#
# Der Mixer mit Live-Testton hält den Ausgang dauerhaft aktiv: keine Anlaufverzögerung,
# kein Gerätekonflikt für den Testton.
# ----------------------------------------------------------------------------
class Audio:
    def __init__(self, cfg, on_level):
        Gst.init(None)
        self.cfg = cfg
        self.on_level = on_level
        self.rtp_packets = 0
        self.last_packet = 0.0
        self.volume_pct = int(cfg.get("volume", "80"))
        self.muted = False
        self._tone_timer = None

        port = int(cfg.get("rtp_port", "5004"))
        jitter = int(cfg.get("jitter_ms", "60"))
        dev = cfg.get("alsa_device", "default").strip() or "default"
        sink = "alsasink" if dev == "default" else f'alsasink device="{dev}"'
        desc = (
            f'udpsrc name=udp port={port} buffer-size=212992 '
            f'caps="application/x-rtp,media=audio,clock-rate=48000,encoding-name=OPUS,payload=111" '
            f"! rtpjitterbuffer name=jb latency={jitter} drop-on-latency=true do-lost=true "
            "! rtpopusdepay name=depay ! opusdec plc=true use-inband-fec=true "
            "! audioconvert ! audioresample ! audio/x-raw,format=S16LE,rate=48000,channels=1 "
            "! queue max-size-time=200000000 leaky=downstream ! mix. "
            "audiotestsrc name=tone is-live=true wave=sine freq=880 volume=0.0 "
            "! audioconvert ! audio/x-raw,format=S16LE,rate=48000,channels=1 ! mix. "
            "audiomixer name=mix latency=20000000 ! audioconvert ! audioresample "
            "! volume name=vol ! level name=lvl interval=250000000 post-messages=true "
            f"! {sink} sync=false"
        )
        log.debug("Pipeline: %s", desc)
        self.pipeline = Gst.parse_launch(desc)
        self.vol = self.pipeline.get_by_name("vol")
        self.tone = self.pipeline.get_by_name("tone")
        depay = self.pipeline.get_by_name("depay")
        depay.get_static_pad("src").add_probe(Gst.PadProbeType.BUFFER, self._on_rtp_buffer)

        bus = self.pipeline.get_bus()
        bus.add_signal_watch()
        bus.connect("message", self._on_bus)

        self.apply_volume()
        self.pipeline.set_state(Gst.State.PLAYING)

        # GLib-Mainloop für Bus-Nachrichten in eigenem Thread
        self.loop = GLib.MainLoop()
        threading.Thread(target=self.loop.run, daemon=True, name="glib").start()

    def _on_rtp_buffer(self, _pad, _info):
        self.rtp_packets += 1
        self.last_packet = time.monotonic()
        return Gst.PadProbeReturn.OK

    def _on_bus(self, _bus, msg):
        t = msg.type
        if t == Gst.MessageType.ELEMENT:
            s = msg.get_structure()
            if s and s.get_name() == "level":
                rms = s.get_value("rms")
                try:
                    self.on_level(float(rms[0]) if len(rms) else -120.0)
                except Exception:
                    pass
        elif t == Gst.MessageType.ERROR:
            err, dbg = msg.parse_error()
            log.error("GStreamer-Fehler: %s (%s)", err, dbg)
            # Ausgang neu starten
            self.pipeline.set_state(Gst.State.NULL)
            time.sleep(1.0)
            self.pipeline.set_state(Gst.State.PLAYING)
        elif t == Gst.MessageType.WARNING:
            err, dbg = msg.parse_warning()
            log.warning("GStreamer: %s", err)

    @property
    def playing(self):
        return (time.monotonic() - self.last_packet) < 1.0

    def apply_volume(self):
        lin = 0.0 if self.muted else (self.volume_pct / 100.0) ** 2  # perzeptiv angenehmere Kurve
        self.vol.set_property("volume", max(0.0, min(2.0, lin)))
        self.vol.set_property("mute", bool(self.muted))

    def set_volume(self, pct):
        self.volume_pct = max(0, min(100, int(pct)))
        self.apply_volume()

    def set_mute(self, on):
        self.muted = bool(on)
        self.apply_volume()

    def test_tone(self, seconds=1.5, freq=880):
        self.tone.set_property("freq", float(freq))
        self.tone.set_property("volume", 0.35)
        if self._tone_timer:
            self._tone_timer.cancel()
        self._tone_timer = threading.Timer(float(seconds), lambda: self.tone.set_property("volume", 0.0))
        self._tone_timer.daemon = True
        self._tone_timer.start()

    def identify(self, seconds=5.0):
        """Kurze Pieptöne im Wechsel, damit man das Gerät im Raum findet."""
        def run():
            end = time.monotonic() + float(seconds)
            while time.monotonic() < end:
                self.test_tone(0.12, 1320)
                time.sleep(0.4)
        threading.Thread(target=run, daemon=True).start()

    def stop(self):
        self.pipeline.set_state(Gst.State.NULL)
        self.loop.quit()


# ----------------------------------------------------------------------------
# WebSocket-Client
# ----------------------------------------------------------------------------
class SpeakerClient:
    def __init__(self, cfg):
        self.cfg = cfg
        self.state = load_state()
        self.name = self.state.get("name") or cfg.get("name")
        self.group = self.state.get("group") or cfg.get("group")
        self.device_id = device_id()
        self.rms_db = -120.0
        self.led = Led(cfg.get("led_gpio", ""))
        self.audio = Audio(cfg, self._on_level)
        if "volume" in self.state:
            self.audio.set_volume(self.state["volume"])
        if "muted" in self.state:
            self.audio.set_mute(self.state["muted"])
        self.stop_event = asyncio.Event()

    def _on_level(self, rms_db):
        self.rms_db = rms_db

    def ws_url(self):
        server = self.cfg.get("server").strip().rstrip("/")
        if "://" in server:
            server = server.split("://", 1)[1]
        scheme = "wss" if self.cfg.get("tls", "no").lower() in ("1", "yes", "true") else "ws"
        return f"{scheme}://{server}/ws/voice/speaker"

    def server_host(self):
        server = self.cfg.get("server").strip()
        if "://" in server:
            server = server.split("://", 1)[1]
        return server.split("/")[0].split(":")[0]

    async def run(self):
        backoff = 2
        while not self.stop_event.is_set():
            try:
                await self._session()
                backoff = 2
            except (OSError, websockets.WebSocketException, asyncio.TimeoutError) as e:
                log.warning("Verbindung zum Server fehlgeschlagen: %s", e)
            except Exception as e:  # pragma: no cover
                log.exception("Unerwarteter Fehler: %s", e)
            self.led.off()
            if self.stop_event.is_set():
                break
            log.info("Neuer Versuch in %ss", backoff)
            try:
                await asyncio.wait_for(self.stop_event.wait(), timeout=backoff)
            except asyncio.TimeoutError:
                pass
            backoff = min(30, backoff * 2)

    async def _session(self):
        url = self.ws_url()
        log.info("Verbinde zu %s als „%s“ (Gruppe %s, Gerät %s)", url, self.name, self.group, self.device_id)
        async with websockets.connect(url, ping_interval=20, ping_timeout=20, max_size=1 << 20) as ws:
            await ws.send(json.dumps({
                "type": "register",
                "device_id": self.device_id,
                "name": self.name,
                "group": self.group,
                "rtp_port": int(self.cfg.get("rtp_port", "5004")),
                "codec": "opus",
                "version": VERSION,
                "ip": local_ip(self.server_host()),
            }))
            hb_task = asyncio.create_task(self._heartbeat(ws))
            try:
                async for raw in ws:
                    await self._handle(ws, raw)
            finally:
                hb_task.cancel()

    async def _heartbeat(self, ws):
        interval = float(self.cfg.get("heartbeat_s", "5"))
        while True:
            await ws.send(json.dumps({
                "type": "heartbeat",
                "playing": self.audio.playing,
                "rms_db": round(self.rms_db, 1),
                "volume": self.audio.volume_pct,
                "muted": self.audio.muted,
                "uptime_s": int(time.monotonic()),
                "rtp_packets": self.audio.rtp_packets,
            }))
            await asyncio.sleep(interval)

    async def _ack(self, ws, cmd, ok=True, error=None):
        msg = {"type": "ack", "cmd": cmd, "ok": ok}
        if error:
            msg["error"] = str(error)
        await ws.send(json.dumps(msg))

    async def _handle(self, ws, raw):
        try:
            msg = json.loads(raw)
        except ValueError:
            return
        t = msg.get("type")
        if t == "registered":
            self.name = msg.get("name") or self.name
            self.group = msg.get("group") or self.group
            if "volume" in msg:
                self.audio.set_volume(msg["volume"])
            if "muted" in msg:
                self.audio.set_mute(msg["muted"])
            self._persist()
            self.led.on()
            log.info("Angemeldet als „%s“ in Gruppe „%s“, Lautstärke %s%%", self.name, self.group, self.audio.volume_pct)
        elif t == "config":
            self.name = msg.get("name") or self.name
            self.group = msg.get("group") or self.group
            self._persist()
            await self._ack(ws, "config")
        elif t == "set_volume":
            self.audio.set_volume(msg.get("volume", self.audio.volume_pct))
            self._persist()
            await self._ack(ws, "set_volume")
        elif t == "mute":
            self.audio.set_mute(msg.get("on", True))
            self._persist()
            await self._ack(ws, "mute")
        elif t == "test_tone":
            self.audio.test_tone(msg.get("seconds", 1.5))
            await self._ack(ws, "test_tone")
        elif t == "identify":
            secs = float(msg.get("seconds", 5.0))
            self.audio.identify(secs)
            self.led.blink(secs)
            await self._ack(ws, "identify")
        elif t == "stream":
            log.info("Durchsage %s", "gestartet" if msg.get("active") else "beendet")
        elif t == "ping":
            await self._ack(ws, "ping")
        elif t == "reboot":
            if self.cfg.get("allow_reboot", "yes").lower() in ("1", "yes", "true"):
                await self._ack(ws, "reboot")
                log.warning("Neustart auf Anforderung des Servers")
                subprocess.Popen(["systemctl", "reboot"])
            else:
                await self._ack(ws, "reboot", ok=False, error="reboot deaktiviert")
        elif t == "deleted":
            log.warning("Vom Server entfernt, melde mich in 30 s neu an")
            await asyncio.sleep(30)
            raise OSError("re-register")
        elif t == "replaced":
            log.warning("Verbindung durch neue Instanz ersetzt, beende diese Sitzung")
            raise OSError("replaced")
        elif t == "error":
            log.error("Server: %s", msg.get("message"))

    def _persist(self):
        save_state({
            "name": self.name,
            "group": self.group,
            "volume": self.audio.volume_pct,
            "muted": self.audio.muted,
        })

    def shutdown(self):
        self.stop_event.set()
        self.audio.stop()
        self.led.off()


def main():
    logging.basicConfig(
        level=logging.DEBUG if os.environ.get("ORS_SPEAKER_DEBUG") else logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
    )
    cfg = load_config()
    client = SpeakerClient(cfg)

    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)

    def _stop(*_):
        log.info("Beende…")
        client.shutdown()

    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, _stop)

    try:
        loop.run_until_complete(client.run())
    finally:
        client.shutdown()
        loop.close()


if __name__ == "__main__":
    main()
