// debrief.js – kompletter Neuanfang
import { API_BASE_URL } from "/static/js/api.js";
import { authFetch } from "/static/js/http.js";

// === GLOBALE Variablen ===
let currentSessionId = null;
let sessionStartTime = 0;
let sessionLength = 1; // Sekunden
let selectedViewerId = null;
let lastDebriefVideoUrl = null;
let isScrubbing = false;
let lastScrubSec = 0;
let markerPrefTypes = [];
let markerPrefIds = [];
let markerPrefOnly = false;

const showToast = (msg, ok = true) => {
  if (typeof window.showUiToast === "function") {
    window.showUiToast(msg, ok);
  } else {
    console[ok ? "log" : "warn"](msg);
  }
};

// === Helfer ===
async function notifyDebriefStart() {
  try {
    await authFetch('/api/sim/debrief/start', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({}),
    });
  } catch (e) {
    console.warn('debrief start notify failed', e);
  }
}

notifyDebriefStart();

function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;"
  }[c]));
}

function getMarkerOffsetSeconds() {
  const offsetInput = document.getElementById("offsetSeconds");
  const raw = Number(offsetInput?.value ?? 0);
  return Math.max(0, Math.min(120, Number.isFinite(raw) ? raw : 0));
}

function setMarkerOffsetSeconds(value) {
  const clamped = Math.max(0, Math.min(120, Number(value) || 0));
  const offsetInput = document.getElementById("offsetSeconds");
  const range = document.getElementById("offsetRange");
  const badge = document.getElementById("offsetBadge");
  if (offsetInput) offsetInput.value = String(clamped);
  if (range) range.value = String(clamped);
  if (badge) badge.textContent = `${clamped}s`;
}

function initMarkerOffsetControls() {
  const offsetInput = document.getElementById("offsetSeconds");
  const range = document.getElementById("offsetRange");
  const bindStep = (id, delta) => {
    const btn = document.getElementById(id);
    if (!btn) return;
    btn.addEventListener("click", () => {
      setMarkerOffsetSeconds(getMarkerOffsetSeconds() + delta);
    });
  };
  if (offsetInput) {
    offsetInput.addEventListener("input", () => {
      setMarkerOffsetSeconds(offsetInput.value);
    });
  }
  if (range) {
    range.addEventListener("input", () => {
      setMarkerOffsetSeconds(range.value);
    });
  }
  bindStep("offsetMinus30", -30);
  bindStep("offsetMinus10", -10);
  bindStep("offsetPlus10", 10);
  bindStep("offsetPlus30", 30);
  setMarkerOffsetSeconds(offsetInput?.value || range?.value || 0);
}

function secondsToClock(sec) {
  sec = (sec || 0) | 0;
  if (sec < 0) sec = 0;
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function getTimeFromTimelineEvent(evt) {
  const timeline = document.getElementById("timeline");
  if (!timeline || !window.currentSessionLengthSec) return 0;

  const rect = timeline.getBoundingClientRect();
  const x = evt.clientX - rect.left;
  let ratio = x / rect.width;
  if (ratio < 0) ratio = 0;
  if (ratio > 1) ratio = 1;

  const sec = ratio * window.currentSessionLengthSec;
  return sec;
}

function updateTimelineTooltip(evt, sec) {
  const tooltip = document.getElementById("timelineTooltip");
  const timeline = document.getElementById("timeline");
  if (!tooltip || !timeline) return;

  const rect = timeline.getBoundingClientRect();
  const x = evt.clientX - rect.left;

  tooltip.textContent = secondsToClock(sec);
  tooltip.style.left = `${x}px`;
  tooltip.classList.remove("hidden");
}

function hideTimelineTooltip() {
  const tooltip = document.getElementById("timelineTooltip");
  if (tooltip) tooltip.classList.add("hidden");
}

function setupTimelineScrubbing() {
  const timeline = document.getElementById("timeline");
  if (!timeline) return;

  timeline.addEventListener("mousedown", (evt) => {
    if (evt.button !== 0) return; // nur Linksklick
    isScrubbing = true;
    const sec = getTimeFromTimelineEvent(evt);
    lastScrubSec = sec;

    updateTimelineProgress(sec);
    updateTimelineTooltip(evt, sec);
  });

  timeline.addEventListener("mousemove", (evt) => {
    const sec = getTimeFromTimelineEvent(evt);

    if (isScrubbing) {
      lastScrubSec = sec;
      updateTimelineProgress(sec);
      updateTimelineTooltip(evt, sec);
    } else {
      // nur Hover-Preview
      updateTimelineTooltip(evt, sec);
    }
  });

  timeline.addEventListener("mouseleave", () => {
    if (!isScrubbing) {
      hideTimelineTooltip();
    }
  });

  document.addEventListener("mouseup", (evt) => {
    if (!isScrubbing) return;
    isScrubbing = false;
    hideTimelineTooltip();

    const sec = lastScrubSec;

    // Offset berücksichtigen wie bei jumpToDebriefTime
    const vorlauf = getMarkerOffsetSeconds();
    const targetSec = Math.max(0, sec - vorlauf);

    console.log(
      `🧲 Scrub-Ende: springe zu ${targetSec}s (raw=${sec.toFixed(
        2
      )}s, Vorlauf=${vorlauf}s)`
    );

    // Debrief-Player setzen
    jumpToDebriefTime(sec); // nutzt offset intern und updated progress

    // Viewer springen lassen
    if (typeof sendViewerCommand === "function") {
      sendViewerCommand({
        action: "seek_to",
        time: targetSec
      });
    }
  });
}

// === Viewer-Steuerung ===

// JSON → POST /api/viewers/:id/cmd
async function sendViewerCommand(command) {
  if (!selectedViewerId) {
    showToast("Kein Viewer ausgewählt.", false);
    return;
  }

  try {
    console.log("[debrief] sendViewerCommand →", selectedViewerId, command);
    const res = await authFetch(
      `${API_BASE_URL}/api/viewers/${encodeURIComponent(selectedViewerId)}/cmd`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(command)
      }
    );

    if (!res.ok) {
      console.error("❌ Viewer-Cmd failed", res.status, await res.text());
      showToast("Viewer-Befehl fehlgeschlagen.", false);
    }
  } catch (err) {
    console.error("❌ Fehler beim Senden des Viewer-Befehls:", err);
    showToast("Verbindung zum Viewer-Backend fehlgeschlagen.", false);
  }
}

async function sendViewerCmd(body) {
  return sendViewerCommand(body);
}

function sendShowIdle() {
  return sendViewerCmd({ cmd: "show_idle" });
}

function sendShowVod(url) {
  if (!url) {
    showToast("Kein VOD für diese Session verfügbar.", false);
    return;
  }
  return sendViewerCmd({ cmd: "show_vod", url });
}

function sendShowLive(url) {
  if (!url) {
    showToast("Keine Live-URL vorhanden.", false);
    return;
  }
  return sendViewerCmd({ cmd: "show_live", url });
}

// === Viewer-Dropdown ===
async function loadViewerDropdown() {
  try {
    const res = await authFetch(`${API_BASE_URL}/api/viewers`);
    const data = await res.json();
    const viewers = data.viewers || [];

    const select = document.getElementById("viewerSelect");
    const badge = document.getElementById("viewerCountBadge");
    if (!select) return;

    select.innerHTML = `<option value="">— Viewer wählen —</option>`;

    viewers.forEach((v) => {
      const opt = document.createElement("option");
      opt.value = String(v.id);

      const online = v.online === true;
      const status = online ? "🟢" : "⚪";

      const alias = String(v.alias || "").trim();
      const baseName = String(v.name || "").trim() || v.url || "Viewer ohne Namen";
      const displayName = alias || baseName;
      const secondary = alias && alias !== baseName ? ` (${baseName})` : "";
      opt.textContent = `${status} #${v.id} • ${displayName}${secondary}`;
      select.appendChild(opt);
    });

    if (badge) {
      badge.textContent = String(viewers.length);
    }
  } catch (err) {
    console.error("❌ Fehler beim Laden der Viewer:", err);
  }
}

document.getElementById("viewerSelect")?.addEventListener("change", (e) => {
  const val = e.target.value;
  if (!val) {
    selectedViewerId = null;
    localStorage.removeItem("selectedViewerId");
    return;
  }

  selectedViewerId = Number(val) || null;
  localStorage.setItem("selectedViewerId", String(selectedViewerId));
  console.log("✅ Viewer gewählt:", selectedViewerId);

  // Sessions laden erlauben
  window.loadSessions = loadSessions;
});

// === Sessions laden & auswählen ===
async function loadSessions() {
  try {
    const res = await authFetch(`${API_BASE_URL}/api/sessions`);
    const data = await res.json();

    const modalContent = document.getElementById("sessionModalContent");
    if (!modalContent) return;
    modalContent.innerHTML = "";

    (data.sessions || []).forEach((session) => {
      const div = document.createElement("div");
      div.className =
        "bg-gray-800 rounded-lg p-4 flex flex-col items-center gap-3 cursor-pointer hover:bg-blue-600 transition shadow";

      const idBox = document.createElement("div");
      idBox.className =
        "w-full min-h-32 rounded-lg border border-gray-700 bg-black/40 flex items-center justify-center";
      idBox.innerHTML = `
        <div class="text-center">
          <div class="text-xs uppercase tracking-widest text-blue-200/80">Session ID</div>
          <div class="text-5xl font-black text-white leading-none mt-2">${escapeHtml(session.id)}</div>
        </div>
      `;

      const name = document.createElement("div");
      name.className = "text-white font-bold text-lg text-center";
      name.textContent = session.name;

      div.appendChild(idBox);
      div.appendChild(name);

      div.addEventListener("click", () => {
        console.log(`Session ${session.id} gewählt`);
        selectSession(session.id);
        document.getElementById("sessionModal")?.classList.add("hidden");
      });

      modalContent.appendChild(div);
    });

    document.getElementById("sessionModal")?.classList.remove("hidden");
  } catch (err) {
    console.error("Fehler beim Laden der Sitzungen:", err);
  }
}

function selectSession(sessionId) {
  console.log(`Lade Session ${sessionId}`);
  currentSessionId = sessionId;
  window.currentSessionId = sessionId;

  loadSessionDetails(sessionId);
  loadSessionMarkers(sessionId);

  document.getElementById("sessionModal")?.classList.add("hidden");
}

document
  .getElementById("closeSessionModal")
  ?.addEventListener("click", () => {
    console.log("Session-Auswahl abgebrochen");
    document.getElementById("sessionModal")?.classList.add("hidden");
  });

// === Session-Details ===
async function fetchSessionDetails(sessionId) {
  const res = await authFetch(`${API_BASE_URL}/api/sessions`);
  if (!res.ok) {
    throw new Error(`GET /api/sessions → ${res.status}`);
  }
  const data = await res.json();
  const sessions = Array.isArray(data.sessions) ? data.sessions : [];
  const idNum = Number(sessionId);
  const sess = sessions.find((s) => Number(s.id) === idNum);
  if (!sess) {
    throw new Error(`Session ${sessionId} nicht gefunden`);
  }
  return sess;
}

async function fetchSessionVod(sessionId) {
  const res = await authFetch(
    `/api/sessions/${encodeURIComponent(sessionId)}/vod`,
    {
      method: "GET",
      headers: { Accept: "application/json" }
    }
  );
  if (!res.ok) {
    throw new Error(`VOD-Request fehlgeschlagen (${res.status})`);
  }
  return await res.json(); // { items: [...] }
}

async function loadSessionDetails(sessionId) {
  try {
    const [data, vodData] = await Promise.all([
      fetchSessionDetails(sessionId),
      fetchSessionVod(sessionId).catch((err) => {
        console.warn("Konnte VOD-Liste nicht laden:", err);
        return { items: [] };
      })
    ]);

    console.log("Session-Details geladen:", data, vodData);

    const nameEl = document.getElementById("sessionName");
    if (nameEl) {
      nameEl.textContent = data.name || `Session #${data.id}`;
    }

    const lenMs = data.length ?? 0;
    const lenSec = Math.round(lenMs / 1000);
    sessionLength = lenSec || 1;
    window.currentSessionLengthSec = sessionLength;

    const durEl = document.getElementById("sessionDuration");
    if (durEl) {
      durEl.textContent = secondsToClock(lenSec);
    }

    sessionStartTime = data.start_time || 0;

    const recordingLink = document.getElementById("sessionRecording");
    if (recordingLink) {
      recordingLink.href = "#";
      recordingLink.textContent = "❌ Keine Aufnahme verfügbar";
      recordingLink.classList.add("text-gray-400");
      recordingLink.classList.remove("text-blue-400");

      const items = Array.isArray(vodData?.items) ? vodData.items : [];

      const vodItem =
        items.find(
          (it) => (it.kind || "").toLowerCase() === "mp4"
        ) || items[0];

      if (vodItem && vodItem.url) {
        const videoUrl = vodItem.url.startsWith("http")
          ? vodItem.url
          : `${window.location.origin}${vodItem.url}`;

        recordingLink.href = videoUrl;
        recordingLink.textContent = "🎥 Aufnahme ansehen";
        recordingLink.classList.remove("text-gray-400");
        recordingLink.classList.add("text-blue-400");

        lastDebriefVideoUrl = videoUrl;

        // Viewer VOD anzeigen, aber noch nicht abspielen (Play-Button)
        sendShowVod(videoUrl);
        console.log("🎬 VOD-Link gesetzt:", videoUrl);
      }
    }
  } catch (err) {
    console.error("Fehler beim Laden der Session-Details:", err);
  }
}

// === Marker laden ===
let markerSortMode = 'time'; // 'time' | 'user'

function sortMarkersDebrief(markers) {
  if (markerSortMode === 'user') {
    return [...markers].sort((a, b) => {
      const ua = (a.username || 'not logged in').toLowerCase();
      const ub = (b.username || 'not logged in').toLowerCase();
      if (ua === ub) {
        return (a.record_time ?? a.timestamp ?? 0) - (b.record_time ?? b.timestamp ?? 0);
      }
      return ua.localeCompare(ub);
    });
  }
  return [...markers].sort((a, b) => (a.record_time ?? a.timestamp ?? 0) - (b.record_time ?? b.timestamp ?? 0));
}

function normalizeMarkerTypes(list) {
  return (list || [])
    .map((s) => String(s || "").trim())
    .filter(Boolean);
}

function markerTypeSelected(markerType) {
  if (!markerType) return false;
  const target = String(markerType).toLowerCase();
  return markerPrefTypes.some((t) => t.toLowerCase() === target);
}

function markerIdSelected(markerId) {
  if (markerId == null) return false;
  return markerPrefIds.includes(Number(markerId));
}

function syncMarkerPrefToggle() {
  const toggle = document.getElementById("markerPrefToggle");
  if (!toggle) return;
  toggle.disabled = !markerPrefTypes.length && !markerPrefIds.length;
}

async function loadSessionMarkers(sessionId) {
  try {
    const res = await authFetch(
      `${API_BASE_URL}/api/sessions/${sessionId}/markers?with_snapshots=1`,
      {
        method: "GET"
      }
    );
    if (!res.ok) {
      throw new Error(
        `GET /api/sessions/${sessionId}/markers → ${res.status}`
      );
    }
    const data = await res.json();
    const rawMarkers = Array.isArray(data.markers) ? data.markers : [];
    const markers = sortMarkersDebrief(rawMarkers);

    console.log("Marker geladen:", markers);

   const markerList = document.getElementById("markerList");
const timeline   = document.getElementById("timeline");
if (!markerList || !timeline) return;

markerList.innerHTML = "";
// timeline-inner neu setzen, progressFill bleibt bestehen
timeline.innerHTML = `
  <div id="timelineTooltip"
       class="hidden absolute bottom-full mb-1 px-2 py-1 text-xs rounded bg-black/80 text-white shadow pointer-events-none">
    00:00
  </div>
  <div id="timeline-inner" class="relative h-full min-w-full">
    <div id="progressFill"
         class="absolute left-0 top-0 h-full bg-blue-500 rounded transition-all duration-150"
         style="width:0%"></div>
  </div>
`;
const inner = timeline.querySelector("#timeline-inner");

// Timeline-Breite: 5px pro Sekunde, min. 300px
inner.style.width = `${Math.max(300, sessionLength * 5)}px`;

  markers.forEach((marker) => {
    const hasPref = markerPrefIds.length || markerPrefTypes.length;
    const isPref = markerIdSelected(marker.id) || markerTypeSelected(marker.marker_type);
    if (markerPrefOnly && hasPref && !isPref) return;
    const relSec = (marker.record_time ?? marker.timestamp ?? 0) | 0;
    const absSec = (sessionStartTime || 0) + relSec;
    const timeLabelAbs = new Date(absSec * 1000).toLocaleTimeString();
    const timeLabelRel = secondsToClock(relSec);

      // Listen-Eintrag
    const div = document.createElement("div");
    div.className =
      "flex items-start gap-3 px-3 py-3 border bg-gray-900/60 hover:bg-gray-800/80 cursor-pointer transition rounded-lg";
    if (hasPref && isPref) {
      div.classList.add("marker-pref-selected");
      div.style.setProperty("--pref-color", marker.color || "#f59e0b");
    }

    const left = document.createElement("div");
    const snapshotUrl = `/api/markers/${marker.id}/snapshot`;
    const preview = document.createElement("div");
    preview.className = "w-40 flex-shrink-0";
    const idLabel = document.createElement("div");
    idLabel.className = "text-center text-2xl font-black leading-none text-white/80 mb-1";
    idLabel.textContent = `#${marker.id}`;
    const img = document.createElement("img");
    img.alt = "Snapshot";
    img.loading = "lazy";
    img.className = "w-40 h-24 object-cover rounded-md border border-gray-800 flex-shrink-0";
    preview.appendChild(idLabel);
    preview.appendChild(img);

    // mit Retry, falls Snapshot gerade erst erzeugt wird
    const tryLoad = (attempt = 0) => {
      img.src = attempt === 0 ? snapshotUrl : `${snapshotUrl}?t=${Date.now()}`;
      img.onerror = () => {
        if (attempt < 3) {
          setTimeout(() => tryLoad(attempt + 1), 1200);
        } else {
          img.classList.add("hidden");
        }
      };
    };
    tryLoad();

    left.innerHTML = `
      <div class="font-bold text-white">${escapeHtml(
        marker.marker_type || "Marker"
      )}</div>
      <div class="text-xs text-gray-400">
        t=${timeLabelRel} • ${timeLabelAbs}${
        marker.username ? " • " + escapeHtml(marker.username) : ""
      }
      </div>
      <div class="mt-1 text-sm text-gray-200">${escapeHtml(
        marker.text || "Marker ohne Beschreibung"
      )}</div>
    `;

    const color = document.createElement("div");
    color.className = "w-5 h-5 rounded-full";
    color.style.backgroundColor = marker.color || "#888";

    const borderColor = marker.color || "#3b82f6";
    div.style.borderColor = borderColor;
    div.style.boxShadow = `0 0 0 1px ${borderColor} inset`;

    div.appendChild(preview);
    div.appendChild(left);
    div.appendChild(color);

      div.addEventListener("click", () => {
        const vorlauf = getMarkerOffsetSeconds();
        const seekSeconds = Math.max(0, relSec - vorlauf);
        console.log(
          `📋 Marker-List-Klick: springe zu ${seekSeconds}s (mit ${vorlauf}s Vorlauf)`
        );

        jumpToDebriefTime(seekSeconds);
        sendViewerCommand({
          action: "seek_to",
          time: seekSeconds
        });
      });

      markerList.appendChild(div);

      // Timeline-Punkt
      if (sessionLength > 0) {
        const positionPercent = Math.min(
          100,
          Math.max(0, (relSec / sessionLength) * 100)
        );

        const point = document.createElement("div");
        point.className =
          "absolute top-0 w-2 h-6 bg-green-400 rounded-full cursor-pointer hover:bg-green-300 transition";
        if (hasPref && isPref) {
          point.classList.add("marker-pref-point-selected");
        }
        point.style.left = `calc(${positionPercent}% - 1px)`;
        point.title = `${
          marker.marker_type || "Marker"
        } @ t=${timeLabelRel} – ${marker.text || ""}`;

        point.addEventListener("click", () => {
          const vorlauf = getMarkerOffsetSeconds();
          const seekSeconds = Math.max(0, relSec - vorlauf);
          console.log(
            `⏩ Marker-Klick: springe zu ${seekSeconds}s (mit ${vorlauf}s Vorlauf)`
          );

          jumpToDebriefTime(seekSeconds);
          sendViewerCommand({
            action: "seek_to",
            time: seekSeconds
          });
        });

        inner.appendChild(point);
      }
    });
  } catch (err) {
    console.error("Fehler beim Laden der Marker:", err);
    const markerList = document.getElementById("markerList");
    if (markerList) {
      markerList.innerHTML =
        "<div class='text-red-500 text-sm'>Fehler beim Laden der Marker.</div>";
    }
  }
}

async function pollDebriefPosition() {
  try {
    const res = await authFetch('/api/sim/debrief/position', { method: 'GET' });
    if (!res.ok) return;

    const data = await res.json(); // { position_ms: 12345 }
    const posSec = (data.position_ms || 0) / 1000;

    updateTimelineProgress(posSec);

  } catch (e) {
    console.warn("pollDebriefPosition fehlgeschlagen:", e);
  }
}


// === Debrief-Playback-Position setzen ===
async function jumpToDebriefTime(tsSeconds) {
  const offset = getMarkerOffsetSeconds();

  // Zielzeit in Sekunden (>= 0)
  const targetSec = Math.max(0, tsSeconds - offset);

  try {
    await authFetch("/api/sim/debrief/position", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        // Backend erwartet nur dieses Feld:
        position_ms: Math.round(targetSec * 1000),
      }),
    });

    // Fortschritt in der UI aktualisieren
    updateTimelineProgress(targetSec);
  } catch (e) {
    console.error("set debrief position failed", e);
  }
}


function updateTimelineProgress(posSec) {
  const fill = document.getElementById("progressFill");
  if (!fill || !window.currentSessionLengthSec) return;

  const pct = Math.max(
    0,
    Math.min(100, (posSec / window.currentSessionLengthSec) * 100)
  );
  fill.style.width = pct + "%";
}

// === Volume-Modal ===
const volumeModal = document.getElementById("volumeModal");
const volumeSlider = document.getElementById("volumeSlider");

document.getElementById("btnVolume")?.addEventListener("click", () => {
  volumeModal?.classList.remove("hidden");
});

document.getElementById("confirmVolume")?.addEventListener("click", () => {
  const volume = parseInt(volumeSlider.value, 10);
  sendViewerCommand({
    action: "set_volume",
    volume
  });
  volumeModal?.classList.add("hidden");
});

document.getElementById("cancelVolume")?.addEventListener("click", () => {
  volumeModal?.classList.add("hidden");
});

// === Control-Buttons ===
document.getElementById("btnPlay")?.addEventListener("click", () => {
  if (!lastDebriefVideoUrl) {
    console.warn(
      "Kein VOD für diese Session geladen, schicke trotzdem 'play'."
    );
  }
  sendViewerCommand({ action: "play" });
});

document.getElementById("btnPause")?.addEventListener("click", () => {
  sendViewerCommand({ action: "pause" });
});

document.getElementById("btnBack")?.addEventListener("click", () => {
  sendViewerCommand({ action: "seek", offset: -5 });
});

document.getElementById("btnForward")?.addEventListener("click", () => {
  sendViewerCommand({ action: "seek", offset: 5 });
});

document.getElementById("btnBlank")?.addEventListener("click", () => {
  sendShowIdle();
});

document.getElementById("btnFullscreen")?.addEventListener("click", () => {
  sendViewerCommand({ action: "fullscreen" });
});

// === DOMContentLoaded ===
document.addEventListener("DOMContentLoaded", async () => {
  console.log("Debriefing geladen...");
  await loadViewerDropdown();
  initMarkerOffsetControls();

  const params = new URLSearchParams(window.location.search);
  const viewerParam = params.get("viewer_id");
  const sessionParam = params.get("session_id");
  const prefParam = params.get("marker_types");
  const prefIdsParam = params.get("marker_ids");
  const filterParam = params.get("marker_filter");
  if (prefParam) {
    markerPrefTypes = normalizeMarkerTypes(prefParam.split(","));
  }
  if (prefIdsParam) {
    markerPrefIds = normalizeMarkerTypes(prefIdsParam.split(","))
      .map((v) => Number(v))
      .filter((v) => Number.isFinite(v));
  }
  markerPrefOnly = filterParam === "1";
  syncMarkerPrefToggle();

  const saved = localStorage.getItem("selectedViewerId");
  if (saved) {
    selectedViewerId = Number(saved) || null;
    const select = document.getElementById("viewerSelect");
    if (select && selectedViewerId) {
      select.value = String(selectedViewerId);
    }
  }
  if (viewerParam) {
    selectedViewerId = Number(viewerParam) || selectedViewerId;
    const select = document.getElementById("viewerSelect");
    if (select && selectedViewerId) {
      select.value = String(selectedViewerId);
      localStorage.setItem("selectedViewerId", String(selectedViewerId));
    }
  }

  if (sessionParam) {
    selectSession(Number(sessionParam));
  }

  // Timeline-Scrubbing aktivieren
  setupTimelineScrubbing();
  // Alle 250ms aktuellen Fortschritt laden
  setInterval(pollDebriefPosition, 250);

  const sortSel = document.getElementById("markerSortSelect");
  if (sortSel) {
    sortSel.addEventListener("change", (e) => {
      markerSortMode = e.target.value === "user" ? "user" : "time";
      if (currentSessionId) {
        loadSessionMarkers(currentSessionId);
      }
    });
  }

  const prefToggle = document.getElementById("markerPrefToggle");
  if (prefToggle) {
    prefToggle.checked = markerPrefOnly;
    prefToggle.addEventListener("change", () => {
      markerPrefOnly = prefToggle.checked;
      if (currentSessionId) {
        loadSessionMarkers(currentSessionId);
      }
    });
  }
});

// Button, der irgendwo in deinem HTML loadSessions() aufruft, z.B.:
// <button onclick="loadSessions()">Session wählen</button>
window.loadSessions = loadSessions;
window.setMarkerSortMode = (mode) => {
  markerSortMode = mode === "user" ? "user" : "time";
  const sel = document.getElementById("markerSortSelect");
  if (sel) sel.value = markerSortMode;
  if (window.currentSessionId) {
    loadSessionMarkers(window.currentSessionId);
  }
};
