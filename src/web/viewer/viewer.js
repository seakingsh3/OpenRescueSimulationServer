// viewer.js – komplett neu, zugeschnitten auf OpenResqueSim-Server
(() => {
  console.log("[viewer] Initialisiere OpenResqueSim Viewer …");

  const logBuffer = [];
  const LOG_LIMIT = 60;
  const consoleOrig = {
    log: console.log.bind(console),
    warn: console.warn.bind(console),
    error: console.error.bind(console),
    info: console.info ? console.info.bind(console) : null,
  };
  function pushLog(level, args) {
    const parts = [];
    for (const a of args) {
      if (typeof a === "string") {
        parts.push(a);
      } else {
        try {
          parts.push(JSON.stringify(a));
        } catch {
          parts.push(String(a));
        }
      }
    }
    const entry = `[${new Date().toISOString()}] ${level}: ${parts.join(" ")}`;
    logBuffer.push(entry);
    if (logBuffer.length > LOG_LIMIT) {
      logBuffer.splice(0, logBuffer.length - LOG_LIMIT);
    }
  }
  console.log = (...args) => {
    pushLog("log", args);
    consoleOrig.log(...args);
  };
  console.warn = (...args) => {
    pushLog("warn", args);
    consoleOrig.warn(...args);
  };
  console.error = (...args) => {
    pushLog("error", args);
    consoleOrig.error(...args);
  };
  if (consoleOrig.info) {
    console.info = (...args) => {
      pushLog("info", args);
      consoleOrig.info(...args);
    };
  }

  // === DOM-Elemente ===
  const statusIndicator = document.getElementById("status-indicator");
  const statusText      = document.getElementById("status-text");
  const modeText        = document.getElementById("mode-text");
  const reportBtn       = document.getElementById("reportBtn");
  const offlineOverlay  = document.getElementById("offlineOverlay");

  const idleScreen = document.getElementById("idle-screen");
  const liveScreen = document.getElementById("live-screen");
  const vodScreen  = document.getElementById("vod-screen");

  const idleText       = document.getElementById("idle-text");
  const preStartBanner     = document.getElementById("pre-start-banner");
  const preStartBannerText = document.getElementById("pre-start-banner-text");
  const idleSlideshow  = document.getElementById("idle-slideshow");
  const idleImage      = document.getElementById("idle-image");
  const idleCaption    = document.getElementById("idle-caption");
  const tickerText     = document.getElementById("ticker-text");
  const tickerTextDup  = document.getElementById("ticker-text-dup");
  const tickerTrack    = document.getElementById("ticker-track");
  const pagingPill     = document.getElementById("paging-pill");
  const pagingMuteBtn  = document.getElementById("paging-mute-btn");
  const pagingLabel    = document.getElementById("paging-label");

  const liveVideo       = document.getElementById("live-video");
  const livePlaceholder = document.getElementById("live-placeholder");

  const vodVideo       = document.getElementById("vod-video");
  const vodPlaceholder = document.getElementById("vod-placeholder");

  // === globaler State ===
  const state = {
    // Steuer-WS (Viewer-Control)
    ws: null,
    reconnectAttempts: 0,
    reconnectTimer: null,

    // Modus
    currentMode: "idle",

    // Idle / Ticker
    triviaIndex: 0,
    tickerIndex: 0,
    triviaImages: [],
    trivia: [
      "Trivia: Dieser Viewer hat keine lokale Bedienung – alles läuft über den Server."
    ],
    tickerMessages: [
      "Jeder macht, kann und darf Fehler machen - keine Fehler = kein Training",
      "Im Zentrum: Teamarbeit, Entscheidungsfindung",
      "Hilfsmittel können und sollen im Szenario genutzt werden",
      "Selbsterkenntnis wird angestrebt",
      "Professioneller Umgang miteinander ist wichtig",
      "Leistung Simulation ist nicht gleich Leistung im Einsatz",
      "Simulation ist kein Schauspiel! Sei wie du bist!",
      "Fiktionsvertrag: Die gegebenen Umstände sind für HEUTE real",
      "KEINE Prüfung! Lernen steht im Vordergrund",
      "Alle Informationen sind vertraulich zu behandeln - KEINE Fotos/Videos mit dem Handy machen"
    ],

    // Medien

    // WebRTC
    webrtcPc: null,
    webrtcWs: null,
    webrtcStatsTimer: null,
    webrtcRetryCount: 0,
    webrtcRetryTimer: null,
    currentLiveUrl: null,
    currentSessionId: null,
    reloadGuard: false,
    livePlaybackCheck: null,

    // Paging-Audio (PTT → Viewer)
    paging: {
      pc: null,
      ws: null,
      audioEl: null,
      active: false,
      muted: false,
    }
  };
  // Für Debug in der Konsole (z.B. __ors_state.webrtcPc.getStats())
  window.__ors_state = state;
  window.state = state; // Legacy-Name, falls man "state" direkt nutzt

  // auch für Debugging im Browser:
  window.__ors_webrtcState = state;

  // === Helper: Screens umschalten ===
  function setScreenMode(mode) {
    const screens = [
      { el: idleScreen, key: "idle" },
      { el: liveScreen, key: "live" },
      { el: vodScreen,  key: "vod"  },
    ];

    screens.forEach(({ el, key }) => {
      if (!el) return;
      if (key === mode) {
        el.classList.add("screen-visible");
        el.style.display = "flex";
      } else {
        el.classList.remove("screen-visible");
        el.style.display = "none";
      }
    });

    state.currentMode = mode;

    if (modeText) {
      if (mode === "live") modeText.textContent = "Modus: LIVE";
      else if (mode === "vod") modeText.textContent = "Modus: VOD";
      else modeText.textContent = "Modus: IDLE";
    }
  }
  // Initialzustand sauber setzen
  setScreenMode("idle");

  // === Helper: Viewer-ID / WS-URL ===
  function getViewerIdFromPath() {
    const parts = window.location.pathname.split("/").filter(Boolean);
    const i = parts.indexOf("viewer");
    if (i >= 0 && parts[i + 1]) {
      return parts[i + 1];
    }
    return null;
  }

  function buildControlWebSocketUrl() {
    const proto = window.location.protocol === "https:" ? "wss" : "ws";
    const host  = window.location.host;
    const viewerId = getViewerIdFromPath();

    const path = viewerId
      ? `/api/viewers/${encodeURIComponent(viewerId)}/ws`
      : `/api/viewers/0/ws`;

    // Geräteschlüssel aus der Kopplung (ohne Schlüssel antwortet der Server „pair_required“)
    const deviceKey = window.ORSPairing && viewerId ? window.ORSPairing.key(viewerId) : "";
    const url = `${proto}://${host}${path}` + (deviceKey ? `?device_key=${encodeURIComponent(deviceKey)}` : "");
    console.log("[viewer] WS URL:", url.replace(/device_key=[^&]+/, "device_key=…"));
    return url;
  }

  // === Status-Anzeige ===
  function setConnected(connected) {
    if (!statusIndicator || !statusText) return;

    if (connected) {
      statusIndicator.classList.remove("status-disconnected");
      statusIndicator.classList.add("status-connected");
      statusText.textContent = "Verbunden";
      if (offlineOverlay) offlineOverlay.classList.remove("show");
    } else {
      statusIndicator.classList.remove("status-connected");
      statusIndicator.classList.add("status-disconnected");
      statusText.textContent = "Getrennt";
      if (offlineOverlay) offlineOverlay.classList.add("show");
    }
  }

  function updatePagingUi() {
    if (!pagingPill || !pagingMuteBtn || !pagingLabel) return;
    const { active, muted } = state.paging;
    pagingPill.classList.toggle("hidden", !active);
    pagingPill.classList.toggle("active", active && !muted);
    pagingLabel.textContent = active ? (muted ? "Durchsage stumm" : "Durchsage aktiv") : "Durchsage";
    pagingMuteBtn.textContent = muted ? "🔈 Unmute" : "🔇 Mute";
  }

  // === Idle-Slideshow / Ticker ===
  function updateIdleLayout(hasImages) {
    if (!idleScreen) return;
    idleScreen.classList.toggle("has-trivia", hasImages);
  }

  function showTriviaSlide(idx) {
    if (!idleSlideshow || !idleImage) return;
    const items = state.triviaImages;
    if (!items || items.length === 0) {
        idleSlideshow.classList.add("hidden");
        updateIdleLayout(false);
        return;
    }
    idleSlideshow.classList.remove("hidden");
    updateIdleLayout(true);
    const item = items[idx % items.length];
    idleImage.classList.remove("show");
    setTimeout(() => {
      idleImage.src = item.url || item.file || "/static/img/placeholder_snapshot.jpg";
      idleCaption.textContent = item.title || "";
      idleImage.classList.add("show");
    }, 80);
  }

  let triviaTimer = null;
  function startTriviaLoop() {
    if (triviaTimer) {
      clearInterval(triviaTimer);
      triviaTimer = null;
    }
    if (!state.triviaImages || state.triviaImages.length === 0) {
      if (idleSlideshow) idleSlideshow.classList.add("hidden");
      updateIdleLayout(false);
      return;
    }
    updateIdleLayout(true);
    state.triviaIndex = 0;
    showTriviaSlide(state.triviaIndex);
    triviaTimer = window.setInterval(() => {
      state.triviaIndex = (state.triviaIndex + 1) % state.triviaImages.length;
      showTriviaSlide(state.triviaIndex);
    }, 8000);
  }

  function applyTickerMessages() {
    if (!tickerText || !tickerTextDup || !tickerTrack) return;
    const msgs =
      state.tickerMessages && state.tickerMessages.length > 0
        ? state.tickerMessages
        : state.trivia;
    const text = msgs.join("  •  ");
    tickerText.textContent = text;
    tickerTextDup.textContent = text;
    tickerTrack.style.animation = "none";
    void tickerTrack.offsetHeight;
    tickerTrack.style.animation = "";
  }

  async function loadUiContent() {
    try {
      const res = await fetch("/api/ui/content");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      if (Array.isArray(data.ticker_messages)) {
        state.tickerMessages = data.ticker_messages;
      }
      if (Array.isArray(data.trivia_images)) {
        state.triviaImages = data.trivia_images;
      }
    } catch (e) {
      console.warn("[viewer] UI-Content konnte nicht geladen werden:", e);
    } finally {
      applyTickerMessages();
      startTriviaLoop();
    }
  }

  // === Medien-Handling ===
  function cleanupWebRtc() {
    console.log("[viewer] cleanupWebRtc()");

    if (state.webrtcStatsTimer) {
      window.clearInterval(state.webrtcStatsTimer);
      state.webrtcStatsTimer = null;
    }
    if (state.livePlaybackCheck) {
      window.clearInterval(state.livePlaybackCheck);
      state.livePlaybackCheck = null;
    }
    if (state.webrtcRetryTimer) {
      window.clearTimeout(state.webrtcRetryTimer);
      state.webrtcRetryTimer = null;
    }

    try {
      if (state.webrtcPc) {
        state.webrtcPc.getSenders().forEach((s) => {
          try { s.track && s.track.stop(); } catch {}
        });
        state.webrtcPc.close();
      }
    } catch (e) {
      console.warn("[viewer] Fehler beim Beenden der RTCPeerConnection:", e);
    }

    if (state.webrtcWs && state.webrtcWs.readyState === WebSocket.OPEN) {
      try { state.webrtcWs.close(); } catch {}
    }

    state.webrtcPc = null;
    state.webrtcWs = null;

    if (liveVideo && liveVideo.srcObject) {
      try {
        const tracks = liveVideo.srcObject.getTracks();
        tracks.forEach((t) => t.stop());
      } catch {}
      liveVideo.srcObject = null;
    }
  }

  function stopAllMedia() {
    console.log("[viewer] stopAllMedia()");
    cleanupWebRtc();
    // Eine laufende Durchsage bewusst NICHT beenden: Sie läuft über eine eigene Verbindung,
    // unabhängig davon, was gerade angezeigt wird (Live, VOD, Ruhebild). Sonst brach z.B.
    // show_live beim Wiederverbinden die Durchsage ab. Beendet wird sie per paging_stop.

    if (state.livePlaybackCheck) {
      window.clearInterval(state.livePlaybackCheck);
      state.livePlaybackCheck = null;
    }

    try {
      if (liveVideo) {
        liveVideo.pause();
        liveVideo.removeAttribute("src");
        liveVideo.load();
      }
    } catch {}

    try {
      if (vodVideo) {
        vodVideo.pause();
        vodVideo.removeAttribute("src");
        vodVideo.load();
      }
    } catch {}

    if (liveVideo) liveVideo.style.display = "none";
    if (vodVideo)  vodVideo.style.display  = "none";
    if (livePlaceholder) livePlaceholder.style.display = "flex";
    if (vodPlaceholder)  vodPlaceholder.style.display  = "flex";
  }

  // === Modi: Idle / Live / VOD ===
  function showPreStartNotice(name) {
    console.log("[viewer] Vor-Start-Hinweis:", name);
    if (!preStartBanner) return;
    if (preStartBannerText) {
      preStartBannerText.textContent = name
        ? `Start steht bevor: „${name}“ – bitte bereit machen!`
        : "Start steht bevor – bitte bereit machen!";
    }
    preStartBanner.classList.remove("hidden");
  }

  function clearPreStartNotice() {
    if (!preStartBanner) return;
    preStartBanner.classList.add("hidden");
  }

  function showIdle() {
    console.log("[viewer] Wechsel zu IDLE");
    stopAllMedia();
    setScreenMode("idle");
    state.currentSessionId = null;
  }

  function showLive(url) {
    console.log("[viewer] Wechsel zu LIVE, URL:", url);
    clearPreStartNotice();
    stopAllMedia();
    setScreenMode("live");
    state.webrtcRetryCount = 0;

    if (!url) {
      console.warn("[viewer] show_live ohne URL erhalten.");
      return;
    }

    // WebRTC-Variante: /api/webrtc/<id> → Signaling immer /ws/webrtc
    if (url.startsWith("/api/webrtc/")) {
      const match = url.match(/\/api\/webrtc\/(\d+)/);
      state.currentSessionId = match ? Number(match[1]) : null;
      if (livePlaceholder) {
        livePlaceholder.textContent = "Verbinde WebRTC-Live-Stream …";
        livePlaceholder.style.display = "flex";
      }
      if (liveVideo) liveVideo.style.display = "none";

      startLiveWebRTC();
      return;
    }

    // Direkt-Video Fallback
    console.log("[viewer] Starte LIVE als direktes Video (ohne Autoplay)");
    liveVideo.src = url;
    liveVideo.style.display = "block";
    if (livePlaceholder) livePlaceholder.style.display = "none";
  }

  function showAudience(url) {
    console.log("[viewer] showAudience:", url);
    clearPreStartNotice();
    stopAllMedia();
    setScreenMode("live");

    if (liveVideo) {
      liveVideo.style.display = "none";
      liveVideo.srcObject = null;
      liveVideo.src = "";
    }
    if (livePlaceholder) {
      const safeUrl = url || "tcp://<host>:9100";
      livePlaceholder.style.display = "flex";
      livePlaceholder.innerHTML = `
        <div class="space-y-2 text-center text-white">
          <div class="text-lg font-semibold">Audience-Stream (TCP, MPEG-TS)</div>
          <div class="text-sm text-gray-300">Öffne in VLC oder ffplay:</div>
          <div class="font-mono text-sm bg-black/60 px-3 py-2 rounded inline-block">${safeUrl}</div>
          <div class="text-xs text-gray-400">Beispiel: <code>vlc ${safeUrl}</code> oder <code>ffplay ${safeUrl}</code></div>
        </div>
      `;
    }
  }

  function showVod(url) {
    console.log("[viewer] Wechsel zu VOD, URL:", url);
    clearPreStartNotice();
    stopAllMedia();
    setScreenMode("vod");

    if (!url) {
      console.warn("[viewer] show_vod ohne URL erhalten.");
      return;
    }

    // Grund-Setup für Audio, kein Autoplay, sofort pausiert
    if (vodVideo) {
      vodVideo.muted = false; // Audio laden
      vodVideo.volume = 1.0;
      vodVideo.playsInline = true;
      vodVideo.autoplay = false;
      vodVideo.controls = true;
      vodVideo.pause();
    }

    // Kein Klick-Placeholder mehr: UI steuert Start/Play extern
    if (vodPlaceholder) {
      vodPlaceholder.style.display = "none";
      vodPlaceholder.onclick = null;
    }

    console.log("[viewer] Starte VOD als direktes Video");
    vodVideo.src = url;
    vodVideo.style.display = "block";
    vodVideo.pause(); // bleibt angehalten bis UI play triggert
  }

  async function sendViewerReport() {
    const viewerIdRaw = getViewerIdFromPath();
    const viewerId = viewerIdRaw ? Number(viewerIdRaw) : null;
    if (!viewerId || Number.isNaN(viewerId)) {
      alert("Viewer-ID fehlt.");
      return;
    }
    const note = window.prompt("Fehlerbeschreibung (optional):", "") || "";
    const payload = {
      session_id: Number.isFinite(state.currentSessionId) ? state.currentSessionId : null,
      note: note.trim() || null,
      logs: logBuffer.slice(-30),
      url: window.location.href,
    };
    try {
      const res = await fetch(`/api/viewers/${viewerId}/report`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      if (reportBtn) {
        const prev = reportBtn.textContent;
        reportBtn.textContent = "✓ Gesendet";
        setTimeout(() => { reportBtn.textContent = prev; }, 2000);
      }
    } catch (e) {
      console.error("[viewer] Fehlerbericht fehlgeschlagen", e);
      alert("Fehlerbericht konnte nicht gesendet werden.");
    }
  }

  // === WebRTC-Client – 1:1 auf deinen Server zugeschnitten ===
  function startLivePlaybackGuard() {
    if (state.livePlaybackCheck) {
      window.clearInterval(state.livePlaybackCheck);
    }
    state.livePlaybackCheck = window.setInterval(() => {
      if (!liveVideo || !liveVideo.srcObject) return;
      if (liveVideo.paused || liveVideo.ended) {
        const p = liveVideo.play();
        if (p && typeof p.catch === "function") {
          p.catch((err) => console.warn("[viewer] Playback-Guard play() failed:", err));
        }
      }
      if (liveVideo.paused && livePlaceholder) {
        livePlaceholder.textContent =
          "Autoplay blockiert – klicke hier, um den Live-Stream zu starten.";
        livePlaceholder.style.display = "flex";
        livePlaceholder.style.cursor = "pointer";
        livePlaceholder.onclick = () => {
          livePlaceholder.onclick = null;
          livePlaceholder.style.display = "none";
          liveVideo.play().catch((err2) =>
            console.warn("[viewer] play() nach Klick fehlgeschlagen:", err2)
          );
        };
      }
    }, 2000);
  }

  function startLiveWebRTC() {
    console.log("[viewer] Starte LIVE via WebRTC");

    cleanupWebRtc();

    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = proto + "//" + window.location.host + "/ws/webrtc"
      + (state.streamTicket ? "?ticket=" + encodeURIComponent(state.streamTicket) : "");

    const pc = new RTCPeerConnection({
      iceServers: [] /* LAN: nur Host-Kandidaten */
    });

    state.webrtcPc = pc;
    window.__ors_webrtcPc = pc; // Debug für Konsole

    pc.oniceconnectionstatechange = () => {
      console.log("[viewer] ICE state →", pc.iceConnectionState);
    };

    pc.onconnectionstatechange = () => {
      console.log("[viewer] PC connectionState →", pc.connectionState);
      if (pc.connectionState === "connected") {
        state.webrtcRetryCount = 0;
      }
      if (
        ["failed", "disconnected", "closed"].includes(pc.connectionState) &&
        state.webrtcRetryCount < 1 &&
        !state.webrtcRetryTimer
      ) {
        state.webrtcRetryCount += 1;
        console.warn("[viewer] WebRTC fehlgeschlagen – versuche einmal neu zu verbinden …");
        state.webrtcRetryTimer = window.setTimeout(() => {
          state.webrtcRetryTimer = null;
          startLiveWebRTC();
        }, 800);
      }
    };

  let webrtcStream = new MediaStream();
  pc.ontrack = (ev) => {
    console.log("[viewer] 🎥 ontrack:", ev.streams);

    // Immer alle Tracks in einen gemeinsamen Stream sammeln (auch wenn Audio zuerst kommt)
    if (!webrtcStream.getTracks().includes(ev.track)) {
      try { webrtcStream.addTrack(ev.track); } catch (e) { console.warn("[viewer] addTrack failed", e); }
    }
    const stream = webrtcStream;
    if (!stream || !liveVideo) return;

    // sicherstellen, dass unser Video-Element denselben Stream nutzt
    if (liveVideo.srcObject !== webrtcStream) {
      liveVideo.srcObject = webrtcStream;
    }

    // Stelle sicher, dass der Live-Screen sichtbar ist
    setScreenMode("live");
    // Autoplay-freundlich: zunächst stumm starten
    liveVideo.muted = true;
    liveVideo.playsInline = true;
    liveVideo.autoplay = true;
    liveVideo.controls = true;
    liveVideo.volume = 1.0;

    const showClickToPlay = () => {
      if (!livePlaceholder) return;
      livePlaceholder.textContent =
        "Autoplay blockiert – klicke hier, um den Live-Stream mit Ton zu starten.";
      livePlaceholder.style.display = "flex";
      livePlaceholder.style.cursor = "pointer";
      liveVideo.style.display = "none";
      livePlaceholder.onclick = () => {
        livePlaceholder.onclick = null;
        livePlaceholder.style.display = "none";
        liveVideo.style.display = "block";
        liveVideo.muted = false;
        liveVideo.play().catch((err2) =>
          console.warn("[viewer] play() nach Klick fehlgeschlagen:", err2)
        );
      };
    };

    const tryPlayMuted = () => {
      const p = liveVideo.play();
      if (p && typeof p.catch === "function") {
        p.catch((err) => {
          console.warn("[viewer] WebRTC liveVideo.play() Fehler:", err);
          showClickToPlay();
        });
      }
    };

    // erster Versuch (muted) + Playback-Guard
    tryPlayMuted();
    setTimeout(tryPlayMuted, 400);
    setTimeout(tryPlayMuted, 1200);
    startLivePlaybackGuard();

    // Bei erstem Playing nur unmute, wenn Browser es erlaubt
    liveVideo.addEventListener(
      "playing",
      () => {
        if (liveVideo.muted === false) return;
        // Nur nach echter Nutzeraktion unmuted
      },
      { once: true }
    );

    // Globale Klick-Geste: unmute + play
    const unlock = () => {
      liveVideo.muted = false;
      liveVideo.play().catch((err2) =>
        console.warn("[viewer] play() nach Klick fehlgeschlagen:", err2)
      );
      liveVideo.removeEventListener("click", unlock);
    };
    liveVideo.addEventListener("click", unlock);

    if (livePlaceholder) livePlaceholder.style.display = "none";
    liveVideo.style.display = "block";
  };

    pc.onicecandidate = (ev) => {
      if (ev.candidate && state.webrtcWs && state.webrtcWs.readyState === WebSocket.OPEN) {
        const msg = {
          type: "ice",
          candidate: ev.candidate.candidate,
          sdp_mline_index: ev.candidate.sdpMLineIndex
        };
        state.webrtcWs.send(JSON.stringify(msg));
      }
    };

    // Stats alle 2 Sekunden
    state.webrtcStatsTimer = window.setInterval(async () => {
      if (!state.webrtcPc) return;
      try {
        const stats = await state.webrtcPc.getStats(null);
        stats.forEach((report) => {
          if (report.type === "inbound-rtp" && report.kind === "video") {
            console.log("[viewer] STATS inbound video:", {
              ssrc: report.ssrc,
              packetsReceived: report.packetsReceived,
              bytesReceived: report.bytesReceived,
              framesDecoded: report.framesDecoded,
              framesPerSecond: report.framesPerSecond
            });
          }
        });
      } catch (e) {
        console.warn("[viewer] Fehler bei getStats:", e);
      }
    }, 2000);

    // Signaling-WS
    const webrtcWs = new WebSocket(wsUrl);
    state.webrtcWs = webrtcWs;
    window.__ors_webrtcWs = webrtcWs; // Debug

    webrtcWs.onopen = () => {
      console.log("[viewer] WebRTC-WS verbunden … warte auf Offer");
    };

    webrtcWs.onclose = (ev) => {
      console.log("[viewer] WebRTC-WS geschlossen:", ev.code, ev.reason || "");
    };

    webrtcWs.onerror = (err) => {
      console.error("[viewer] WebRTC-WS Fehler:", err);
    };

    webrtcWs.onmessage = async (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch (e) {
        console.warn("[viewer] Ungültiges WebRTC-Signal (kein JSON):", ev.data);
        return;
      }

      if (!msg || typeof msg !== "object") {
        console.warn("[viewer] WebRTC-Signal ist kein Objekt:", msg);
        return;
      }

      if (msg.type === "ready") {
        console.log("[viewer] WebRTC: Server sagt 'ready' – ignoriere (nur Info).");
        return;
      }

      if (msg.type === "offer" && msg.sdp) {
        console.log("[viewer] Offer erhalten → sende Answer …");
        try {
          await pc.setRemoteDescription({
            type: "offer",
            sdp: msg.sdp
          });

          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);

          webrtcWs.send(JSON.stringify({
            type: "answer",
            sdp: answer.sdp
          }));

          console.log("[viewer] Answer lokal gesetzt, ICE state jetzt:", pc.iceConnectionState);
        } catch (e) {
          console.error("[viewer] Fehler beim Verarbeiten des Offers:", e);
        }
        return;
      }

      if (msg.type === "ice" && msg.candidate) {
        try {
          await pc.addIceCandidate({
            candidate: msg.candidate,
            sdpMLineIndex: msg.sdp_mline_index ?? 0
          });
          console.log("[viewer] ← ICE vom Server:", msg.candidate);
        } catch (e) {
          console.warn("[viewer] ICE-Kandidat Fehler:", e);
        }
        return;
      }
      if (msg.type === "ice" && !msg.candidate) {
        // leere Kandidaten ignorieren
        return;
      }

      if (msg.type === "bye") {
        console.log("[viewer] WebRTC-Session beendet (bye)");
        // Du kannst hier z.B. auf Idle zurückschalten:
        // showIdle();
        return;
      }

      console.log("[viewer] Unbekannte WebRTC-Nachricht:", msg);
    };
  }

  // === Paging-Audio (PTT → Viewer) ===
  function stopPagingAudio() {
    const p = state.paging;
    if (!p) return;
    p.active = false;
    if (p.ws) {
      try {
        p.ws.onclose = null;
        p.ws.onerror = null;
        p.ws.close();
      } catch {}
    }
    if (p.pc) {
      try {
        p.pc.close();
      } catch {}
    }
    if (p.audioEl) {
      try {
        p.audioEl.pause();
        p.audioEl.srcObject = null;
      } catch {}
    }
    p.pc = null;
    p.ws = null;
    p.audioEl = null;
    updatePagingUi();
  }

  function startPagingAudio(wsPath = "/ws/voice/viewer", ticket = null) {
    console.log("[viewer] Starte Paging-Audio:", wsPath);
    stopPagingAudio();

    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const base = wsPath.startsWith("ws")
      ? wsPath
      : `${proto}//${window.location.host}${wsPath}`;
    // Ticket der laufenden Durchsage (vom Server mit paging_start geschickt)
    const url = ticket ? `${base}${base.includes("?") ? "&" : "?"}ticket=${encodeURIComponent(ticket)}` : base;

    const pc = new RTCPeerConnection({
      iceServers: [] /* LAN: nur Host-Kandidaten */
    });
    const ws = new WebSocket(url);
    state.paging.pc = pc;
    state.paging.ws = ws;
    state.paging.active = true;
    updatePagingUi();

    pc.onicecandidate = (ev) => {
      if (ev.candidate && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({
          type: "ice",
          candidate: ev.candidate.candidate,
          sdpMLineIndex: ev.candidate.sdpMLineIndex ?? 0
        }));
      }
    };

    pc.ontrack = (ev) => {
      if (ev.track.kind !== "audio") return;
      const [stream] = ev.streams;
      if (!stream) return;
      const audio = new Audio();
      audio.autoplay = true;
      audio.playsInline = true;
      audio.srcObject = stream;
      audio.volume = state.paging.muted ? 0 : 1.0;
      audio.muted = state.paging.muted;
      audio.play().catch((err) => console.warn("[viewer] Paging-Audio play() blockiert:", err));
      state.paging.audioEl = audio;
      console.log("[viewer] Paging-Audio-Stream gesetzt");
      updatePagingUi();
    };

    ws.onopen = () => {
      console.log("[viewer] Paging-WS verbunden");
      state.paging.active = true;
      updatePagingUi();
    };
    ws.onerror = (e) => console.warn("[viewer] Paging-WS Fehler", e);
    ws.onclose = () => {
      console.log("[viewer] Paging-WS geschlossen");
      stopPagingAudio();
    };
    ws.onmessage = async (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch (e) {
        console.warn("[viewer] Ungültiges Paging-Signal:", ev.data);
        return;
      }
      if (msg.type === "offer" && msg.sdp) {
        try {
          await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          ws.send(JSON.stringify({ type: "answer", sdp: answer.sdp }));
          state.paging.active = true;
          updatePagingUi();
        } catch (err) {
          console.warn("[viewer] Paging-Offer konnte nicht verarbeitet werden:", err);
        }
        return;
      }
      if (msg.type === "ice" && msg.candidate) {
        try {
          await pc.addIceCandidate({
            candidate: msg.candidate,
            sdpMLineIndex: msg.sdpMLineIndex ?? msg.sdpMlineIndex ?? 0
          });
        } catch (err) {
          console.warn("[viewer] Paging-ICE Fehler:", err);
        }
        return;
      }
      if (msg.type === "error") {
        console.warn("[viewer] Paging-Fehler:", msg.message);
      }
    };
  }

  function togglePagingMute() {
    const p = state.paging;
    p.muted = !p.muted;
    if (p.audioEl) {
      p.audioEl.muted = p.muted;
      p.audioEl.volume = p.muted ? 0 : 1;
    }
    updatePagingUi();
  }

  if (pagingMuteBtn) {
    pagingMuteBtn.addEventListener("click", togglePagingMute);
  }

  // === Viewer-Control-WebSocket (cmd / action) ===
  function handleActionCommand(msg) {
    const action = msg.action;
    console.log("[viewer] action:", action, msg);

    switch (action) {
      case "load_video": {
        const url = msg.video_url || msg.url;
        if (!url) {
          console.warn("[viewer] load_video ohne URL");
          return;
        }
        showVod(url);
        break;
      }

      case "play": {
        if (state.currentMode === "vod" && vodVideo) {
          vodVideo.play().catch((err) => console.warn("[viewer] play() Fehler (VOD):", err));
        } else if (state.currentMode === "live" && liveVideo) {
          liveVideo.play().catch((err) => console.warn("[viewer] play() Fehler (LIVE):", err));
        }
        break;
      }

      case "pause": {
        if (state.currentMode === "vod" && vodVideo) {
          vodVideo.pause();
        } else if (state.currentMode === "live" && liveVideo) {
          liveVideo.pause();
        }
        break;
      }

      case "seek": {
        if (state.currentMode === "vod" && vodVideo && typeof msg.offset === "number") {
          try {
            vodVideo.currentTime = Math.max(0, vodVideo.currentTime + msg.offset);
          } catch (e) {
            console.warn("[viewer] seek Fehler:", e);
          }
        }
        break;
      }

      case "seek_to": {
        if (state.currentMode === "vod" && vodVideo && typeof msg.time === "number") {
          try {
            vodVideo.currentTime = Math.max(0, msg.time);
          } catch (e) {
            console.warn("[viewer] seek_to Fehler:", e);
          }
        }
        break;
      }

      case "set_volume": {
        if (typeof msg.volume === "number") {
          const v = Math.max(0, Math.min(100, msg.volume)) / 100;
          if (vodVideo) {
            vodVideo.volume = v;
            if (v > 0) vodVideo.muted = false;
          }
          if (liveVideo) {
            liveVideo.volume = v;
            if (v > 0) liveVideo.muted = false;
          }
        }
        break;
      }

      case "mute": {
        if (vodVideo) vodVideo.muted = true;
        if (liveVideo) liveVideo.muted = true;
        break;
      }

      case "unmute": {
        if (vodVideo) {
          vodVideo.muted = false;
          if (typeof msg.volume === "number") {
            vodVideo.volume = Math.max(0, Math.min(100, msg.volume)) / 100;
          }
        }
        if (liveVideo) {
          liveVideo.muted = false;
          if (typeof msg.volume === "number") {
            liveVideo.volume = Math.max(0, Math.min(100, msg.volume)) / 100;
          }
        }
        break;
      }

      case "paging_start": {
        const wsPath = msg.ws || "/ws/voice/viewer";
        startPagingAudio(wsPath, msg.ticket || null);
        break;
      }
      case "paging_stop": {
        stopPagingAudio();
        break;
      }

      case "blank": {
        showIdle();
        break;
      }

      case "simulate_stop":
      case "sim_stop": {
        // Wenn Simulation stoppt → alles anhalten und Idle anzeigen
        stopAllMedia();
        showIdle();
        break;
      }

      case "fullscreen": {
        const target =
          state.currentMode === "vod"  ? vodVideo  :
          state.currentMode === "live" ? liveVideo :
          null;
        if (target && target.requestFullscreen) {
          target.requestFullscreen().catch((err) =>
            console.warn("[viewer] fullscreen Fehler:", err)
          );
        } else {
          console.warn("[viewer] Fullscreen API nicht verfügbar");
        }
        break;
      }
      case "report_ack": {
        showViewerToast(`✅ Fehler bestätigt${msg.ack_by ? ` (${msg.ack_by})` : ''}`, "success");
        break;
      }

      default:
        console.warn("[viewer] Unbekannte action:", action, msg);
    }
  }

  function handleMessage(raw) {
    console.log("[viewer] Nachricht vom Server:", raw);
    let msg;
    try {
      msg = JSON.parse(raw);
    } catch (e) {
      console.warn("[viewer] Ungültige JSON-Nachricht:", raw);
      return;
    }

    if (!msg || typeof msg !== "object") {
      console.warn("[viewer] Nachricht ist kein Objekt:", msg);
      return;
    }

    // Neue Debrief-Steuerung
    if (typeof msg.action === "string") {
      handleActionCommand(msg);
      return;
    }

    // Nicht (mehr) gekoppelt: Bild/Ton stoppen, PIN abfragen, dann neu verbinden
    if (msg.cmd === "pair_required") {
      const viewerId = getViewerIdFromPath();
      if (state.pairRequired || !window.ORSPairing || !viewerId) return;
      state.pairRequired = true;
      stopAllMedia();
      stopPagingAudio();
      showIdle();
      window.ORSPairing.prompt(viewerId).then(() => {
        state.pairRequired = false;
        state.reconnectAttempts = 0;
        try { state.ws && state.ws.close(); } catch {}
        connectControlWebSocket();
      });
      return;
    }

    // Alte show_* Kommandos
    switch (msg.cmd) {
      case "show_idle":
      case "idle":
        clearPreStartNotice();
        showIdle();
        break;

      case "pre_start_notice":
        showPreStartNotice(msg.name);
        break;

      case "reset_webrtc":
        console.warn("[viewer] reset_webrtc erhalten → WebRTC neu aufsetzen");
        cleanupWebRtc();
        state.currentLiveUrl = null;
        state.reloadGuard = false;
        if (msg.url) {
          showLive(msg.url);
        }
        break;

      case "show_live":
        // Zugangs-Ticket für /ws/webrtc (vom Server beigelegt, gilt für diese Session)
        state.streamTicket = msg.ticket || null;
        showLive(msg.url);
        break;

      case "show_audience":
        showAudience(msg.url);
        break;

      case "show_vod":
        showVod(msg.url);
        break;

      default:
        console.warn("[viewer] Unbekannter cmd-Befehl:", msg.cmd);
    }
  }

  function scheduleReconnect() {
    if (state.reconnectTimer || state.pairRequired) return;
    state.reconnectAttempts++;
    const delay = Math.min(30000, 1000 * Math.pow(2, state.reconnectAttempts));
    console.log(
      `[viewer] Plane Reconnect in ${delay}ms (Versuch ${state.reconnectAttempts})`
    );
    state.reconnectTimer = window.setTimeout(() => {
      state.reconnectTimer = null;
      connectControlWebSocket();
    }, delay);
  }

  function connectControlWebSocket() {
    const url = buildControlWebSocketUrl();
    console.log("[viewer] Verbinde mit WebSocket:", url);

    try {
      const ws = new WebSocket(url);
      state.ws = ws;

      ws.onopen = () => {
        console.log("[viewer] WebSocket verbunden");
        setConnected(true);
        state.reconnectAttempts = 0;
      };

      ws.onclose = (ev) => {
        console.log("[viewer] WebSocket geschlossen:", ev.code, ev.reason || "");
        setConnected(false);
        scheduleReconnect();
      };

      ws.onerror = (err) => {
        console.error("[viewer] WebSocket Fehler:", err);
      };

      ws.onmessage = (ev) => {
        handleMessage(ev.data);
      };
    } catch (e) {
      console.error("[viewer] Fehler beim Verbindungsaufbau:", e);
      setConnected(false);
      scheduleReconnect();
    }
  }

  let toastTimer = null;
  function showViewerToast(text, kind) {
    const root = document.getElementById("app") || document.body;
    let toastEl = document.getElementById("viewerToast");
    if (!toastEl) {
      toastEl = document.createElement("div");
      toastEl.id = "viewerToast";
      toastEl.className = "viewer-toast";
      root.appendChild(toastEl);
    }
    toastEl.textContent = text;
    toastEl.classList.toggle("success", kind === "success");
    toastEl.classList.add("show");
    if (toastTimer) window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => {
      toastEl.classList.remove("show");
    }, 3200);
  }

  // === Init ===
  function init() {
    console.log("[viewer] Starte Initialisierung …");
    showIdle();
    applyTickerMessages();
    loadUiContent();
    // QR-Link mit ?pair=<code> koppelt das Gerät vor dem ersten Verbinden
    const viewerId = getViewerIdFromPath();
    const pairing = window.ORSPairing && viewerId ? window.ORSPairing.fromLink(viewerId) : Promise.resolve(null);
    pairing.finally(() => connectControlWebSocket());

    if (reportBtn) {
      reportBtn.addEventListener("click", () => {
        sendViewerReport();
      });
    }
  }

  if (document.readyState === "complete" || document.readyState === "interactive") {
    init();
  } else {
    document.addEventListener("DOMContentLoaded", init);
  }
})();
