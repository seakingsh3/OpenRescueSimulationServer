(function () {
  const statusIndicator = document.getElementById("status-indicator");
  const statusText = document.getElementById("status-text");
  const pagerIdEl = document.getElementById("pager-id");
  const btnConnect = document.getElementById("btn-connect");
  const btnDisconnect = document.getElementById("btn-disconnect");
  const btnMute = document.getElementById("btn-mute");
  const pillState = document.getElementById("pill-state");
  const pillAudio = document.getElementById("pill-audio");
  const logEl = document.getElementById("log");
  const audioEl = document.getElementById("audio");

  const state = {
    controlWs: null,
    controlReconnectAttempts: 0,
    controlReconnectTimer: null,
    audioWs: null,
    pc: null,
    connected: false,
    muted: false,
  };

  function log(msg) {
    const ts = new Date().toISOString().split("T")[1].split(".")[0];
    logEl.textContent += `[${ts}] ${msg}\n`;
    logEl.scrollTop = logEl.scrollHeight;
  }

  function setStatus(connected) {
    state.connected = connected;
    statusIndicator.classList.toggle("status-connected", connected);
    statusText.textContent = connected ? "Verbunden" : "Getrennt";
    pillState.textContent = connected ? "AKTIV" : "IDLE";
    pillState.classList.toggle("active", connected);
  }

  function setAudioState(active) {
    pillAudio.textContent = active
      ? state.muted
        ? "Audio: stumm"
        : "Audio: aktiv"
      : "Audio: --";
    pillAudio.classList.toggle("active", active && !state.muted);
    btnMute.textContent = state.muted ? "🔈 Unmute" : "🔇 Mute";
  }

  function getPagerId() {
    const parts = window.location.pathname.split("/").filter(Boolean);
    const idx = parts.indexOf("pager");
    if (idx >= 0 && parts[idx + 1]) return parts[idx + 1];
    return "unknown";
  }

  function buildWsUrl() {
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const id = encodeURIComponent(getPagerId());
    // Geräteschlüssel aus der Kopplung (ohne Schlüssel antwortet der Server „pair_required“)
    const key = window.ORSPairing ? window.ORSPairing.key(getPagerId()) : "";
    return `${proto}//${window.location.host}/api/viewers/${id}/ws` + (key ? `?device_key=${encodeURIComponent(key)}` : "");
  }

  function cleanupAudio() {
    if (state.audioWs) {
      try {
        state.audioWs.onclose = null;
        state.audioWs.onerror = null;
        state.audioWs.close();
      } catch {}
    }
    if (state.pc) {
      try {
        state.pc.getSenders().forEach((s) => s.track && s.track.stop());
        state.pc.close();
      } catch {}
    }
    state.audioWs = null;
    state.pc = null;
    setAudioState(false);
  }

  function cleanupAll() {
    cleanupAudio();
    if (state.controlReconnectTimer) {
      clearTimeout(state.controlReconnectTimer);
      state.controlReconnectTimer = null;
    }
    if (state.controlWs) {
      try {
        state.controlWs.onclose = null;
        state.controlWs.onerror = null;
        state.controlWs.close();
      } catch {}
    }
    state.controlWs = null;
    setStatus(false);
  }

  function scheduleControlReconnect() {
    if (state.controlReconnectTimer || state.pairRequired) return;
    state.controlReconnectAttempts++;
    const delay = Math.min(30000, 1000 * Math.pow(2, state.controlReconnectAttempts));
    log(`Control-WS Reconnect in ${delay}ms`);
    state.controlReconnectTimer = setTimeout(() => {
      state.controlReconnectTimer = null;
      connectControl();
    }, delay);
  }

  async function startPagingAudio(wsPath = "/ws/voice/viewer", ticket = null) {
    cleanupAudio();
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const base = wsPath.startsWith("ws") ? wsPath : `${proto}//${window.location.host}${wsPath}`;
    log(`Starte Audio-WS: ${base}`);
    // Ticket der laufenden Durchsage (vom Server mit paging_start geschickt)
    const url = ticket ? `${base}${base.includes("?") ? "&" : "?"}ticket=${encodeURIComponent(ticket)}` : base;

    const pc = new RTCPeerConnection({ iceServers: [] /* LAN: nur Host-Kandidaten */ });
    state.pc = pc;
    const ws = new WebSocket(url);
    state.audioWs = ws;

    pc.onicecandidate = (ev) => {
      if (ev.candidate && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({
          type: "ice",
          candidate: ev.candidate.candidate,
          sdpMLineIndex: ev.candidate.sdpMLineIndex ?? 0,
        }));
      }
    };

    pc.ontrack = (ev) => {
      if (ev.track.kind !== "audio") return;
      const [stream] = ev.streams;
      if (!stream) return;
      audioEl.srcObject = stream;
      audioEl.autoplay = true;
      audioEl.playsInline = true;
      audioEl.muted = state.muted;
      audioEl.volume = state.muted ? 0 : 1;
      audioEl.play().catch(() => {
        log("Audio play() blockiert, bitte einmal klicken");
        audioEl.controls = true;
      });
      setAudioState(true);
    };

    ws.onopen = () => log("Audio-WS verbunden");
    ws.onclose = () => {
      log("Audio-WS geschlossen");
      cleanupAudio();
    };
    ws.onerror = (e) => log("Audio-WS Fehler: " + e.message);

    ws.onmessage = async (ev) => {
      let msg;
      try { msg = JSON.parse(ev.data); } catch { log("Audio MSG ungültig"); return; }
      if (msg.type === "offer" && msg.sdp) {
        try {
          await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          ws.send(JSON.stringify({ type: "answer", sdp: answer.sdp }));
        } catch (err) {
          log("Audio Offer/Answer Fehler: " + err);
        }
      } else if (msg.type === "ice" && msg.candidate) {
        try {
          await pc.addIceCandidate({
            candidate: msg.candidate,
            sdpMLineIndex: msg.sdpMLineIndex ?? msg.sdpMlineIndex ?? 0,
          });
        } catch (err) {
          log("Audio ICE Fehler: " + err);
        }
      } else if (msg.type === "error") {
        log("Audio Fehler: " + msg.message);
      }
    };
  }

  function handleControlMessage(raw) {
    let msg;
    try { msg = JSON.parse(raw); } catch { log("Control: kein JSON"); return; }
    // Nicht (mehr) gekoppelt: Ton stoppen, PIN abfragen, dann neu verbinden
    if (msg.cmd === "pair_required") {
      if (state.pairRequired || !window.ORSPairing) return;
      state.pairRequired = true;
      cleanupAudio();
      log("Gerät ist nicht gekoppelt – PIN eingeben");
      window.ORSPairing.prompt(getPagerId()).then(() => {
        state.pairRequired = false;
        state.controlReconnectAttempts = 0;
        connectControl();
      });
      return;
    }
    if (msg.action === "paging_start") {
      const wsPath = msg.ws || "/ws/voice/viewer";
      log("Control: paging_start");
      startPagingAudio(wsPath, msg.ticket || null);
      return;
    }
    if (msg.action === "paging_stop") {
      log("Control: paging_stop");
      cleanupAudio();
      return;
    }
    // ignorieren, andere Actions nicht relevant
  }

  function connectControl() {
    cleanupAll();
    const url = buildWsUrl();
    log(`Verbinde Control-WS: ${url.replace(/device_key=[^&]+/, "device_key=…")}`);
    try {
      const ws = new WebSocket(url);
      state.controlWs = ws;

      ws.onopen = () => {
        log("Control-WS verbunden");
        state.controlReconnectAttempts = 0;
        setStatus(true);
      };
      ws.onclose = () => {
        log("Control-WS geschlossen");
        setStatus(false);
        scheduleControlReconnect();
      };
      ws.onerror = (e) => {
        log("Control-WS Fehler: " + e.message);
      };
      ws.onmessage = (ev) => handleControlMessage(ev.data);
    } catch (err) {
      log("Control-WS konnte nicht geöffnet werden: " + err);
      scheduleControlReconnect();
    }
  }

  function toggleMute() {
    state.muted = !state.muted;
    if (audioEl) {
      audioEl.muted = state.muted;
      audioEl.volume = state.muted ? 0 : 1;
    }
    setAudioState(state.connected);
  }

  // Bindings (manuelle Knöpfe bleiben optional)
  if (btnConnect) btnConnect.addEventListener("click", connectControl);
  if (btnDisconnect) btnDisconnect.addEventListener("click", cleanupAll);
  if (btnMute) btnMute.addEventListener("click", toggleMute);

  // Init
  const pid = getPagerId();
  if (pagerIdEl) pagerIdEl.textContent = `Pager: ${pid}`;
  setStatus(false);
  setAudioState(false);
  log("Verbinde automatisch zum Control-WS …");
  // QR-Link mit ?pair=<code> koppelt das Gerät vor dem ersten Verbinden
  const startPager = () => {
    const pairing = window.ORSPairing ? window.ORSPairing.fromLink(getPagerId()) : Promise.resolve(null);
    pairing.finally(connectControl);
  };
  if (document.readyState === "complete" || document.readyState === "interactive") {
    startPager();
  } else {
    document.addEventListener("DOMContentLoaded", startPager, { once: true });
  }

  window.addEventListener("beforeunload", cleanupAll);
})();
