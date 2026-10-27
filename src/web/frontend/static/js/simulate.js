// === simulate.js ===================================================
import { authFetch, withAuthHeaders } from '/static/js/http.js';

const $ = (s) => document.querySelector(s);

// base.html lädt dieses Modul per dynamischem import() (für das globale FAB),
// nicht per <script>-Tag. Ein dynamischer import() blockiert DOMContentLoaded/load
// nicht – auf schnellen/lokalen Verbindungen (kein Internet, LAN) sind beide Events
// oft schon vorbei, bevor dieses Modul geladen ist. addEventListener käme dann nie
// zum Zug, und die Initialisierung (Marker, Viewer, Marker-Typen) liefe nie an.
// Darum: direkt ausführen, falls das jeweilige Event bereits durch ist.
function onDomReady(cb) {
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', cb, { once: true });
  } else {
    cb();
  }
}
function onWindowLoaded(cb) {
  if (document.readyState === 'complete') {
    cb();
  } else {
    window.addEventListener('load', cb, { once: true });
  }
}

// -------- Toast ----------------------------------------------------
function toast(msg, ok = true) {
  if (typeof window.showUiToast === 'function') {
    window.showUiToast(msg, ok);
    return;
  }
  const t = $('#toast');
  if (!t) return alert(msg);
  t.textContent = msg;
  t.style.background = ok ? '#16a34a' : '#e11d48';
  t.classList.remove('opacity-0', 'pointer-events-none');
  t.style.opacity = '1';
  setTimeout(() => {
    t.style.opacity = '0';
    t.classList.add('opacity-0', 'pointer-events-none');
  }, 4200);
}

function showToast(msg, ok) {
  toast(msg, ok);
}

// -------- Helpers --------------------------------------------------
let _simLoadingTimer = null;
let _simLoadingPending = null;

function showSimLoading(show, message) {
  const overlay = document.getElementById('simLoadingOverlay');
  const text = document.getElementById('simLoadingText');
  if (!overlay) return;
  if (text && message) text.textContent = message;
  overlay.classList.toggle('hidden', !show);
  document.body.classList.toggle('overflow-hidden', show);
}

function beginSimLoading(kind, message) {
  _simLoadingPending = kind;
  if (_simLoadingTimer) clearTimeout(_simLoadingTimer);
  _simLoadingTimer = setTimeout(() => {
    if (_simLoadingPending === kind) {
      showSimLoading(true, message);
    }
  }, 2000);
}

function endSimLoading(kind) {
  if (kind && _simLoadingPending !== kind) return;
  _simLoadingPending = null;
  if (_simLoadingTimer) clearTimeout(_simLoadingTimer);
  _simLoadingTimer = null;
  showSimLoading(false);
}

function toggleHidden(el, show) {
  if (!el) return;
  el.classList[show ? 'remove' : 'add']('hidden');
}

function setFocusMode(on) {
  const body = document.body;
  if (!body) return;
  body.classList.toggle('focus-mode', !!on);
  const toggle = document.querySelector('#focusModeToggle');
  if (toggle) toggle.checked = !!on;
}

function defaultSessionName() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, '0');
  return `Session_${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

const LAST_START_PAYLOAD_KEY = 'ors_last_start_payload_v1';

function normalizeStartPayload(payload) {
  if (!payload || typeof payload !== 'object') return null;
  const name = (payload.name || '').trim() || undefined;
  const viewer_ids = Array.isArray(payload.viewer_ids)
    ? payload.viewer_ids.map((v) => Number(v)).filter((v) => Number.isFinite(v) && v > 0)
    : [];
  const viewer_map = Array.isArray(payload.viewer_map)
    ? payload.viewer_map
        .map((v) => ({
          id: Number(v?.id),
          mode: String(v?.mode || 'webrtc'),
          role: String(v?.role || ''),
        }))
        .filter((v) => Number.isFinite(v.id) && v.id > 0)
    : [];
  const room_id = payload.room_id != null && Number.isFinite(Number(payload.room_id))
    ? Number(payload.room_id)
    : undefined;
  if (!name && !viewer_ids.length && !viewer_map.length && room_id == null) return null;
  return { name, viewer_ids, viewer_map, room_id };
}

function setLastStartPayload(payload) {
  const normalized = normalizeStartPayload(payload);
  if (!normalized) return;
  window.__orsLastStartPayload = normalized;
  try {
    localStorage.setItem(LAST_START_PAYLOAD_KEY, JSON.stringify(normalized));
  } catch {}
}

function getLastStartPayload() {
  if (window.__orsLastStartPayload) return window.__orsLastStartPayload;
  try {
    const raw = localStorage.getItem(LAST_START_PAYLOAD_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    const normalized = normalizeStartPayload(parsed);
    if (normalized) {
      window.__orsLastStartPayload = normalized;
      return normalized;
    }
  } catch {}
  return null;
}

async function jsonReq(url, { method = 'GET', body, timeoutMs = 4000 } = {}) {
  const ac = new AbortController();
  const t = setTimeout(() => ac.abort(), timeoutMs);
  const init = { method, signal: ac.signal, headers: {}, credentials: 'same-origin' };
  init.headers = withAuthHeaders(init.headers);
  if (body != null) {
    init.headers['Content-Type'] = 'application/json';
    init.body = typeof body === 'string' ? body : JSON.stringify(body);
  }
  try {
    const res = await authFetch(url, init);
    const ct = res.headers.get('content-type') || '';
    const parse = async () => (ct.includes('application/json') ? res.json() : res.text());
    if (!res.ok) {
      const payload = await parse().catch(() => null);
      const err = new Error(`${method} ${url} → ${res.status}${payload?.message ? `: ${payload.message}` : ''}`);
      err.status = res.status;
      err.payload = payload;
      throw err;
    }
    return await parse();
  } finally {
    clearTimeout(t);
  }
}



// -------- API-Wrappers ---------------------------------------------
const api = {
  start: (name, viewer_ids, viewer_map, room_id) => {
    const body = {};
    if (name) body.name = name;
    const vids = Array.isArray(viewer_ids)
      ? viewer_ids.filter(v => v != null && !isNaN(v) && Number(v) > 0).map(Number)
      : [];
    if (vids.length) body.viewer_ids = vids;
    if (Array.isArray(viewer_map) && viewer_map.length) body.viewer_map = viewer_map;
    if (room_id) body.room_id = Number(room_id);
    return jsonReq('/api/sim/start', { method: 'POST', body });
  },
  prestart: (name, instructor_ids, viewer_map, room_id) => {
    const body = {};
    if (name) body.name = name;
    const vids = Array.isArray(instructor_ids)
      ? instructor_ids.filter(v => v != null && !isNaN(v) && Number(v) > 0).map(Number)
      : [];
    if (vids.length) {
      body.instructor_ids = vids;
    }
    if (Array.isArray(viewer_map) && viewer_map.length) body.viewer_map = viewer_map;
    if (room_id) body.room_id = Number(room_id);
    return jsonReq('/api/sim/prestart', { method: 'POST', body });
  },
  commit: (viewer_ids, viewer_map, room_id, name) => {
    const body = {};
    const vids = Array.isArray(viewer_ids)
      ? viewer_ids.filter(v => v != null && !isNaN(v) && Number(v) > 0).map(Number)
      : [];
    if (vids.length) body.viewer_ids = vids;
    if (Array.isArray(viewer_map) && viewer_map.length) body.viewer_map = viewer_map;
    if (room_id) body.room_id = Number(room_id);
    if (name) body.name = name;
    return jsonReq('/api/sim/commit', { method: 'POST', body });
  },
};

let _markerNoteCache = [];

async function _fetchJson(url, { method = 'GET', body, timeoutMs = 4000 } = {}) {
  const ac = new AbortController();
  const to = setTimeout(() => ac.abort(), timeoutMs);
  const init = { method, signal: ac.signal, headers: {}, cache: 'no-store', credentials: 'same-origin' };
  init.headers = withAuthHeaders(init.headers);
  if (body != null) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  try {
    const res = await authFetch(url, init);
    const ct = res.headers.get('content-type') || '';
    const parse = async () => (ct.includes('application/json') ? res.json() : res.text());
    if (!res.ok) {
      const payload = await parse().catch(() => null);
      const err = new Error(`${method} ${url} → ${res.status}${payload?.message ? `: ${payload.message}` : ''}`);
      err.status = res.status;
      err.payload = payload;
      throw err;
    }
    return await parse();
  } finally {
    clearTimeout(to);
  }
}

// --------------------------------------------------------------
// Health (Pipeline/Layout/FPS)
// --------------------------------------------------------------
let _healthInFlight = false;
let _lastHealthJson = null;

async function updateHealthCard(force = false) {
  const elState = document.getElementById('healthPipelineState');
  const elSlots = document.getElementById('healthLayoutSlots');
  const elFps = document.getElementById('healthFpsDrop');
  const elVlc = document.getElementById('vlcStatus');
  const elVlcUrl = document.getElementById('vlcUrl');
  const btnVlc = document.getElementById('vlcToggleBtn');
  if (!elState || !elSlots || !elFps) return;

  // Keine Parallel-Requests, bei nicht sichtbarem Tab nur auf explizite force
  if (_healthInFlight || (!force && document.hidden)) return;
  _healthInFlight = true;

  try {
    const res = await _fetchJson('/api/health', { method: 'GET', timeoutMs: 2500 });
    // Wenn nichts geändert, DOM nicht anfassen
    if (_lastHealthJson && JSON.stringify(res) === JSON.stringify(_lastHealthJson) && !force) {
      return;
    }
    _lastHealthJson = res;

    elState.textContent = res?.pipeline_state ?? '–';
    elSlots.textContent = res?.layout_slots ?? '–';
    const fpsTxt = res?.fps != null ? `${res.fps} fps` : '–';
    const dropTxt = res?.dropped_frames != null ? `${res.dropped_frames} drop` : '–';
    elFps.textContent = `${fpsTxt} / ${dropTxt}`;
    if (elVlc && elVlcUrl) {
      if (res?.vlc_port) {
        const host = window.location.hostname || 'localhost';
        elVlc.textContent = 'aktiv';
        elVlcUrl.textContent = `tcp://${host}:${res.vlc_port}`;
        if (btnVlc) btnVlc.textContent = 'Deaktivieren';
        window.__vlc_enabled = true;
      } else {
        elVlc.textContent = 'aus';
        elVlcUrl.textContent = '';
        if (btnVlc) btnVlc.textContent = 'Aktivieren';
        window.__vlc_enabled = false;
      }
    }
  } catch (e) {
    console.warn('health fetch failed', e);
    elState.textContent = 'unbekannt';
    elSlots.textContent = '–';
    elFps.textContent = '–';
    if (elVlc) elVlc.textContent = '–';
    if (elVlcUrl) elVlcUrl.textContent = '';
  } finally {
    _healthInFlight = false;
  }
}

async function toggleVlcStream() {
  const btn = document.getElementById('vlcToggleBtn');
  const target = !window.__vlc_enabled;
  if (btn) btn.disabled = true;
  try {
    await _fetchJson('/api/options/vlc', { method: 'POST', body: { enabled: target } });
    showToast(target ? 'VLC-Stream aktiviert' : 'VLC-Stream deaktiviert', true);
    window.__vlc_enabled = target;
    await updateHealthCard();
  } catch (e) {
    console.error('toggle vlc failed', e);
    showToast('VLC-Stream konnte nicht geschaltet werden', false);
  } finally {
    if (btn) btn.disabled = false;
  }
}
setInterval(() => updateHealthCard(false), 5000);

// --------------------------------------------------------------
// Audio UI (Software-Gain)
// --------------------------------------------------------------

function toggleAudioPanel(open) {
  const chk = document.getElementById('toggleAudioBtn');
  if (!chk) return;
  if (typeof open === 'boolean') {
    chk.checked = open;
  } else {
    chk.checked = !chk.checked;
  }
}

async function fetchAudioStateSafe() {
  try {
    return await _fetchJson('/api/audio/state', { method: 'GET' });
  } catch (e) {
    console.warn('audio/state failed', e);
    return { volumePercent: 100, muted: false };
  }
}

async function renderAudioControls() {
  const wrap = document.getElementById('audioList');
  if (!wrap) return;

  wrap.innerHTML = `<div class="p-3 text-gray-300">Lade Audioquellen…</div>`;

  try {
    const [sources, state] = await Promise.all([
      _fetchJson('/api/audio/sources/list', { method: 'GET' }).catch(() => ({ sources: [] })),
      fetchAudioStateSafe(),
    ]);

    const vol = Number(state?.volumePercent ?? 100);
    const muted = !!state?.muted;
    const srcLabel = (sources?.sources && sources.sources.length)
      ? sources.sources.map(s => `${s.name || s.id}`).join(', ')
      : 'Standard-Input';

    wrap.innerHTML = `
      <div class="bg-gray-800 rounded-lg p-4 shadow">
        <div class="flex items-center justify-between mb-3">
          <div>
            <div class="text-white font-semibold">Digitaler Gain</div>
            <div class="text-xs text-gray-400">wirkt nur in der Pipeline, Hardware bleibt unverändert</div>
          </div>
          <button id="audioMuteBtn" class="px-3 py-1 rounded text-sm ${muted ? 'bg-red-600' : 'bg-gray-700 hover:bg-gray-600'} text-white">${muted ? '🔇 Entmuten' : '🔊 Muten'}</button>
        </div>
        <label class="text-sm text-gray-300 block mb-2">Quelle: ${srcLabel}</label>
        <div class="flex items-center gap-3">
          <input id="audioGainSlider" type="range" min="0" max="200" step="1" value="${vol}" class="w-full" />
          <span id="audioGainLabel" class="w-16 text-right text-gray-200 font-mono">${Math.round(vol)}%</span>
        </div>
      </div>
    `;

    const slider = document.getElementById('audioGainSlider');
    const label = document.getElementById('audioGainLabel');
    const muteBtn = document.getElementById('audioMuteBtn');

    if (slider && label) {
      const updateLabel = (v) => { label.textContent = `${Math.round(Number(v))}%`; };
      slider.addEventListener('input', () => updateLabel(slider.value));
      slider.addEventListener('change', async () => {
        const val = Number(slider.value);
        updateLabel(val);
        try {
          const res = await _fetchJson('/api/audio/volume/set', { method: 'POST', body: { volumePercent: val } });
          updateLabel(res?.volumePercent ?? val);
          showToast('Audio-Gain gesetzt ✅', true);
        } catch (e) {
          console.error('set volume failed', e);
          showToast('Audio-Gain konnte nicht gesetzt werden ❌', false);
        }
      });
    }

    if (muteBtn) {
      muteBtn.addEventListener('click', async () => {
        try {
          const res = await _fetchJson('/api/audio/mute/toggle', { method: 'POST', body: {} });
          muteBtn.textContent = res?.muted ? '🔇 Entmuten' : '🔊 Muten';
          muteBtn.className = `px-3 py-1 rounded text-sm ${res?.muted ? 'bg-red-600' : 'bg-gray-700 hover:bg-gray-600'} text-white`;
          showToast(res?.muted ? 'Audio stumm' : 'Audio aktiv', true);
        } catch (e) {
          console.error('toggle mute failed', e);
          showToast('Mute/Unmute fehlgeschlagen ❌', false);
        }
      });
    }
  } catch (e) {
    console.error('audio UI failed', e);
    wrap.innerHTML = `<div class="p-3 text-red-400">Audio-Steuerung konnte nicht geladen werden.</div>`;
  }
}

function initAudioSection() {
  const toggle = document.getElementById('toggleAudioBtn');
  const panel = document.getElementById('audioList');
  if (!panel || !toggle) return;

  if (!toggle.dataset.boundAudio) {
    toggle.dataset.boundAudio = '1';
    toggle.addEventListener('change', () => { /* DaisyUI steuert Sichtbarkeit */ });
  }

  // Inhalte laden (auch wenn keine Session läuft, damit Regler sichtbar bleiben)
  updateAudioVisibility();
}

async function updateAudioVisibility() {
  const section = document.querySelector('.audio-card');
  if (!section) return;
  section.classList.remove('hidden');
  try {
    await renderAudioControls();
  } catch {
    /* ignore */
  }
}

// --------------------------------------------------------------
// START: Session-Start + Dialog
// --------------------------------------------------------------

// Viewer-Auswahl im Start-Dialog – bei jedem Öffnen neu laden. Früher nur beim ersten
// Öffnen: Viewer, die danach online kamen, fehlten bis zum Neuladen der Seite.
function prepareViewerSelectOnce() {
  const container = document.getElementById('viewerSelectList');
  if (!container) return;
  container.innerHTML = `<div class="text-gray-400 text-sm">Lade Viewer…</div>`;

  // Viewer aus API laden und Liste rendern
  (async () => {
    try {
      const viewers = await listViewersRobust();   // nutzt deine Funktion weiter unten
      _viewerCache = viewers;                      // Cache auch für Modal updaten
      const onlineViewers = viewers.filter(v => !!v.online);
      _viewerSelections = onlineViewers.map((v) => {
        const logicalId = viewerLogicalIdFromUrl(v.url) ?? Number(v.id);
        const role = viewerRoleFromId(logicalId);
        const alias = (v.alias || '').trim();
        const baseName = (v.name || '').trim() || v.url || `Viewer ${logicalId || v.id}`;
        return {
          id: String(v.id),
          logicalId,
          name: alias || baseName,
          rawName: baseName,
          alias,
          online: !!v.online,
          // wenn nur ein Viewer vorhanden ist, selektiere ihn automatisch als Convenience
          selected: onlineViewers.length === 1,
          mode: 'webrtc',
          role,
        };
      });
      renderViewerSelectList();
      updateViewerBadge();
    } catch (e) {
      console.error('Viewer-List für Start-Dialog fehlgeschlagen:', e);
      container.innerHTML = `<div class="text-gray-400 text-sm">Fehler beim Laden der Viewer.</div>`;
    }
  })();
}

function renderViewerSelectList() {
  const container = document.getElementById('viewerSelectList');
  if (!container) return;
  if (!_viewerSelections.length) {
    container.innerHTML = `<div class="text-gray-400 text-sm">Keine aktiven Viewer verbunden.</div>`;
    return;
  }
  container.innerHTML = _viewerSelections.map((v) => {
    const active = !!v.online;
    const statusDot = active ? '🟢' : '⚪';
    const idLabel = Number.isFinite(v.logicalId) ? v.logicalId : v.id;
    const roleLabel = viewerRoleLabel(v.role);
    const secondary = v.alias && v.rawName && v.alias !== v.rawName ? `(${v.rawName})` : '';
    return `<div class="flex items-center gap-3 bg-gray-800 border border-gray-700 rounded px-3 py-2">
      <input type="checkbox" data-viewer-id="${v.id}" class="checkbox checkbox-sm" ${v.selected ? 'checked' : ''}>
      <div class="flex-1">
        <div class="text-white font-medium">${statusDot} ${escapeHtml(idLabel)} • ${escapeHtml(v.name)} ${secondary ? `<span class="text-xs text-gray-400">${escapeHtml(secondary)}</span>` : ''}</div>
        <div class="text-xs text-gray-400">${roleLabel}</div>
      </div>
      <div class="text-xs text-gray-400">Modus: WebRTC</div>
    </div>`;
  }).join('');

  container.querySelectorAll('[data-viewer-id]').forEach((cb) => {
    cb.addEventListener('change', () => {
      const id = cb.dataset.viewerId;
      _viewerSelections = _viewerSelections.map((v) =>
        v.id === id ? { ...v, selected: cb.checked } : v
      );
    });
  });
  // kein Modus-/Rollen-Switch nötig (nur WebRTC)
}

function collectViewerSelection() {
  const selected = _viewerSelections.filter((v) => v.selected);
  const viewer_ids = selected
    .map((v) => Number(v.id))
    .filter((n) => Number.isFinite(n) && n > 0);
  const viewer_map = selected
    .map((v) => ({
      id: Number(v.id),
      mode: 'webrtc',
      role: v.role || viewerRoleFromId(v.logicalId),
    }))
    .filter((x) => Number.isFinite(x.id) && x.id > 0);
  return { viewer_ids, viewer_map };
}

// --- ROOM-Auswahl-----------------------------------

async function fillRoomSelect(sel, emptyLabel) {
  sel.innerHTML = `<option value="">${emptyLabel}</option>`;
  try {
    const res = await _fetchJson('/api/rooms');
    const rooms = Array.isArray(res.rooms) ? res.rooms : [];
    rooms.forEach(r => {
      const opt = document.createElement('option');
      opt.value = r.id;
      opt.textContent = `#${r.id} • ${r.name} (${r.layout})`;
      opt.dataset.name = r.name || '';
      sel.appendChild(opt);
    });
    return rooms;
  } catch (e) {
    console.error('Room-Liste konnte nicht geladen werden:', e);
    sel.innerHTML += `<option value="" disabled>❌ Fehler beim Laden</option>`;
    return [];
  }
}

// Raumlisten bei jedem Öffnen neu laden (vorher nur einmal: neu angelegte Räume fehlten
// bis zum Neuladen der Seite); die aktuelle Auswahl bleibt erhalten.
async function refillRoomSelect(sel, placeholder) {
  const prev = sel.value;
  await fillRoomSelect(sel, placeholder);
  if (prev && [...sel.options].some((o) => o.value === prev)) sel.value = prev;
}

async function prepareRoomSelectOnce() {
  const sel = $('#roomSelect');
  if (!sel) return;
  await refillRoomSelect(sel, '⚙️ Kein Raum (Standard-Pipeline)');
}

// --- Live-Raumwechsel während einer laufenden Session --------------

// Live-Raumwechsel: einmal je Session laden. refreshMarkersUI ruft das regelmäßig auf –
// ein Neuaufbau bei jedem Aufruf würde ein gerade geöffnetes Auswahlmenü zerstören.
async function prepareLiveRoomSelectOnce(sessionId) {
  const sel = $('#liveRoomSelect');
  if (!sel || sel.dataset.preparedFor === String(sessionId)) return;
  sel.dataset.preparedFor = String(sessionId);
  await refillRoomSelect(sel, 'Raum wählen…');
}

function updateLiveRoomUI(st) {
  const wrap = $('#liveRoomWrap');
  if (!wrap) return;
  const active = !!st?.active;
  wrap.classList.toggle('hidden', !active);
  if (!active) return;

  const label = $('#liveRoomCurrent');
  const roomId = st?.room_id != null ? Number(st.room_id) : null;
  if (label) {
    label.textContent = roomId
      ? `#${roomId} ${st.room_name || ''}`.trim()
      : 'kein Raum';
    label.classList.toggle('badge-primary', !!roomId);
    label.classList.toggle('badge-outline', !roomId);
  }
  const sel = $('#liveRoomSelect');
  if (sel && roomId && sel.value === '' ) {
    // Vorauswahl auf den aktuellen Raum, sofern die Option existiert
    const has = Array.from(sel.options).some(o => Number(o.value) === roomId);
    if (has) sel.value = String(roomId);
  }
}

let _roomSwitchBusy = false;
async function switchRoomUI() {
  if (_roomSwitchBusy) return;
  const sel = $('#liveRoomSelect');
  const btn = $('#switchRoomBtn');
  const roomId = sel && sel.value !== '' ? Number(sel.value) : null;
  if (!roomId) {
    showToast('Bitte zuerst einen Raum auswählen.', false);
    return;
  }
  const st = await getSimStatus();
  if (!st?.active) {
    showToast('Keine aktive Session – Raumwechsel nur im laufenden Betrieb.', false);
    return;
  }
  if (st?.room_id != null && Number(st.room_id) === roomId) {
    showToast('Dieser Raum ist bereits aktiv.', true);
    return;
  }

  _roomSwitchBusy = true;
  if (btn) {
    btn.disabled = true;
    btn.dataset.label = btn.textContent;
    btn.textContent = 'Wechsle…';
  }
  try {
    const res = await jsonReq('/api/sim/switch_room', {
      method: 'POST',
      body: { room_id: roomId },
      timeoutMs: 15000,
    });
    const warnings = Array.isArray(res?.warnings) ? res.warnings : [];
    if (warnings.length) {
      showToast(`Raum „${res.room_name}“ aktiv, mit Hinweisen: ${warnings.join(' · ')}`, false);
    } else {
      showToast(`Raum gewechselt: ${res.room_name} (${res.layout}) ✅`, true);
    }
    // Startprofil für Notfall-Neustart mitziehen
    const last = getLastStartPayload();
    if (last) setLastStartPayload({ ...last, room_id: roomId });
    setSimStatusCache(null);
    await refreshMarkersUI();
  } catch (e) {
    console.error('room switch failed', e);
    const msg = e?.payload?.message || e?.message || 'Unbekannter Fehler';
    showToast(`Raumwechsel fehlgeschlagen ❌ ${msg}`, false);
  } finally {
    _roomSwitchBusy = false;
    if (btn) {
      btn.disabled = false;
      btn.textContent = btn.dataset.label || 'Wechseln';
    }
  }
}

(function bindLiveRoomSwitchOnce() {
  const btn = document.querySelector('#switchRoomBtn');
  if (!btn || btn.dataset.boundRoomSwitch === '1') return;
  btn.dataset.boundRoomSwitch = '1';
  btn.addEventListener('click', (e) => {
    e.preventDefault();
    switchRoomUI().catch(() => {});
  });
})();


async function startWithRetry(name, viewer_ids, viewer_map, room_id) {
  beginSimLoading('start', 'Session wird gestartet…');
  try {
    await api.start(name, viewer_ids, viewer_map, room_id);
    setLastStartPayload({ name, viewer_ids, viewer_map, room_id });
    toast('Session gestartet ✅', true);
    try { await startMarkerPoll(); } catch {}
    clearPendingCommit();
    return true;

  } catch (e) {
    if (e.status === 409) {
      const alt = `${name || 'Session'}_${Math.floor(Math.random() * 1e6)
        .toString()
        .padStart(6, '0')}`;
      try {
        await api.start(alt, viewer_ids, viewer_map, room_id);
        setLastStartPayload({ name: alt, viewer_ids, viewer_map, room_id });
        toast(`Session gestartet als „${alt}” ✅`, true);
        try { await startMarkerPoll(); } catch {}
        clearPendingCommit();
        return true;
      } catch (e2) {
        console.error('Retry failed:', e2);
        toast('Start fehlgeschlagen ❌', false);
        endSimLoading('start');
        return false;
      }
    }
    console.error(e);
    toast(e.payload?.message || 'Start fehlgeschlagen ❌', false);
    endSimLoading('start');
    return false;
  }
}



export function initSimulatePage() {
  console.log('[simulate] initSimulatePage');
  // Nach Neuladen der Seite: läuft auf dem Server noch ein Vor-Start, den Freigeben-Knopf
  // wieder anzeigen (Startprofil liegt seit dem Vor-Start im localStorage).
  _statusOnce().then((st) => {
    if (st && !st.active && st.prestart_session_id && !_pendingCommitPayload) {
      const payload = getLastStartPayload();
      if (payload) setPendingCommit(payload);
    }
  }).catch(() => {});
  const audioCard = document.querySelector('.audio-card');
  if (audioCard) audioCard.classList.remove('hidden');
  initAudioSection();
  updateHealthCard();
  bindMarkerModeToggleOnce();
  bindAmlsControlsOnce();
  bindSimHotkeysOnce();
  const btnVlc = document.getElementById('vlcToggleBtn');
  if (btnVlc) {
    btnVlc.addEventListener('click', toggleVlcStream);
  }

  const startBtn = $('#startBtn');
  const modal = $('#sessionNameModal');
  const input = $('#sessionNameInput');
  const confirm = $('#confirmSessionNameBtn');
  const cancel = $('#cancelSessionNameBtn');

  if (!startBtn || !modal || !input || !confirm || !cancel) return;

  startBtn.addEventListener('click', () => {
  input.value = defaultSessionName();
  prepareViewerSelectOnce();
  prepareRoomSelectOnce();       // NEU
  toggleHidden(modal, true);
  input.focus();
});


  cancel.addEventListener('click', () => toggleHidden(modal, false));

confirm.addEventListener('click', async () => {
  confirm.disabled = true;

  const name = input.value.trim() || undefined;

  const selection = collectViewerSelection();
  const viewer_ids = selection.viewer_ids;
  const viewer_map = selection.viewer_map;

  const roomSel = $('#roomSelect');
  const room_id =
    roomSel && roomSel.value !== '' ? Number(roomSel.value) : undefined;
  const prestartChecked = $('#prestartCheckbox')?.checked;

  if (prestartChecked) {
    const ok = await startPrestartFlow({ name, viewer_ids, viewer_map, room_id });
    if (ok) toggleHidden(modal, false);
  } else {
    const ok = await startWithRetry(name, viewer_ids, viewer_map, room_id);
    if (ok) toggleHidden(modal, false);
  }
  confirm.disabled = false;
});


  input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') confirm.click();
  });
}

// --------------------------------------------------------------
// STOP: alle Sessions stoppen
// --------------------------------------------------------------

async function _statusOnce() {
  try { return await _fetchJson('/api/sim/status', { method: 'GET' }); }
  catch { return null; }
}

async function idleAllViewers() {
  try {
    const list = await listViewersRobust();
    const ids = list.map(v => v.id).filter(Boolean);
    for (const id of ids) {
      try {
        await _fetchJson(`/api/viewers/${encodeURIComponent(id)}/cmd`, { method: 'POST', body: { cmd: 'idle' } });
      } catch (_) { /* best effort */ }
    }
  } catch (_) { /* best effort */ }
}

export async function stopAllSessionsUI(opts = {}) {
  const {
    skipDebriefModal = false,
    silent = false,
    externalLoading = false,
  } = opts;
  const btn = document.querySelector('#stopBtn');
  if (btn) btn.disabled = true;
  if (!externalLoading) beginSimLoading('stop', 'Session wird beendet…');

  const stBefore = await _statusOnce();
  // Nur eine laufende Session hat ein Debriefing – ein abgebrochener Vor-Start nicht
  const stoppedSessionId = stBefore?.active ? (stBefore.session_id || null) : null;

  try {
    try {
      await _fetchJson('/api/sim/stop_all', { method: 'POST' });
    } catch (e) {
      console.warn('stop_all:', e.status, e.message);
    }

    const st2 = await _statusOnce();
    if (!st2?.active) {
      clearPendingCommit();
      clearMarkerPoll();
      if (typeof window.refreshSessions === 'function') { try { await window.refreshSessions(); } catch {} }
      if (typeof window.refreshStatus === 'function') { try { await window.refreshStatus(); } catch {} }
      if (!silent) showToast('Sessions gestoppt ✅', true);
      if (!externalLoading) endSimLoading('stop');

      // Viewer in Idle setzen (Best-Effort)
      await idleAllViewers();

      // Debriefing anbieten
      if (!skipDebriefModal && stoppedSessionId) openDebriefSwitchModal(stoppedSessionId);
      return true;
    } else {
      if (!silent) showToast('Einige Sessions laufen weiter ❌', false);
      if (!externalLoading) endSimLoading('stop');
      return false;
    }
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function emergencyRestartSessionUI() {
  const btn = document.querySelector('#emergencyRestartBtn');
  if (btn) btn.disabled = true;
  try {
    const payload = getLastStartPayload();
    if (!payload) {
      showToast('Kein Startprofil vorhanden. Starte zuerst einmal normal eine Session.', false);
      return;
    }

    beginSimLoading('emergency', 'Notfall-Neustart läuft…');
    const stopped = await stopAllSessionsUI({
      skipDebriefModal: true,
      silent: true,
      externalLoading: true,
    });
    endSimLoading('emergency');
    if (!stopped) {
      showToast('Notfall-Neustart abgebrochen: Stop fehlgeschlagen ❌', false);
      return;
    }

    const ok = await startWithRetry(
      payload.name,
      payload.viewer_ids,
      payload.viewer_map,
      payload.room_id
    );
    if (ok) {
      showToast('Notfall-Neustart erfolgreich ✅', true);
    } else {
      showToast('Notfall-Neustart: Start fehlgeschlagen ❌', false);
    }
  } catch (e) {
    endSimLoading('emergency');
    console.error('emergency restart failed', e);
    showToast('Notfall-Neustart fehlgeschlagen ❌', false);
  } finally {
    if (btn) btn.disabled = false;
  }
}

// ======== MARKER-HOTKEYS (Tastenkürzel je Marker-Typ) ================
// Ein Kürzel wird als normalisierter String gespeichert, z.B. "F13", "Numpad1",
// "Ctrl+Shift+Digit1". Basis ist KeyboardEvent.code (layoutunabhängig), damit
// eine eigene Makro-Tastatur zuverlässig dieselben Codes liefert.

function hotkeyFromEvent(ev) {
  const code = ev.code || '';
  if (!code) return null;
  if (/^(Control|Shift|Alt|Meta|AltGraph)(Left|Right)?$/.test(code)) return null;
  const mods = [];
  if (ev.ctrlKey) mods.push('Ctrl');
  if (ev.altKey) mods.push('Alt');
  if (ev.shiftKey) mods.push('Shift');
  if (ev.metaKey) mods.push('Meta');
  return [...mods, code].join('+');
}

function hotkeyLabel(hk) {
  if (!hk) return '';
  return String(hk).split('+').map(part => {
    if (part.startsWith('Digit')) return part.slice(5);
    if (part.startsWith('Key')) return part.slice(3);
    if (part.startsWith('Numpad')) return 'Num ' + part.slice(6);
    if (part === 'Ctrl') return 'Strg';
    if (part === 'Space') return 'Leer';
    return part;
  }).join('+');
}

// F13–F24 sind für Makro-Tastaturen reserviert: keine Browser-Kollisionen,
// und sie wirken auch, wenn gerade ein Eingabefeld fokussiert ist.
function isDedicatedHotkeyCode(code) {
  const m = /^F(\d+)$/.exec(code || '');
  return !!m && Number(m[1]) >= 13;
}

function findMarkerTypeByHotkey(hk) {
  if (!hk || !_markerTypesCache || !_markerTypesCache.length) return null;
  const needle = hk.toLowerCase();
  return _markerTypesCache.find(t => t.hotkey && String(t.hotkey).toLowerCase() === needle) || null;
}

let _hotkeyCapture = null; // { typeId, resolve }
function captureHotkey(typeId) {
  return new Promise(resolve => { _hotkeyCapture = { typeId, resolve }; });
}

async function fireMarkerHotkey(t) {
  try {
    await createTypedMarker(t.id);
    await refreshMarkersUI();
    showToast(`Marker „${t.name}“ gesetzt ✅ (${hotkeyLabel(t.hotkey)})`, true);
  } catch (e) {
    if (e?.status === 409) {
      openNoSessionMarkerModal();
    } else {
      console.error('hotkey marker failed', e);
      showToast(`Marker „${t.name}“ setzen fehlgeschlagen ❌`, false);
    }
  }
}

async function saveMarkerTypeHotkey(t, hk) {
  const next = hk ? String(hk) : '';
  if (next) {
    const other = findMarkerTypeByHotkey(next);
    if (other && String(other.id) !== String(t.id)) {
      showToast(`Taste ${hotkeyLabel(next)} ist schon bei „${other.name}“ belegt ❌`, false);
      return false;
    }
  }
  try {
    await updateMarkerType(t.id, { hotkey: next });
    t.hotkey = next || null;
    renderMarkerTypesList(false);
    renderMarkerTypeButtonsFromCache();
    showToast(next ? `Taste ${hotkeyLabel(next)} → „${t.name}“ ✅` : `Taste für „${t.name}“ entfernt ✅`, true);
    return true;
  } catch (e) {
    if (e?.status === 409) {
      showToast(`Taste ${hotkeyLabel(next)} ist bereits belegt ❌`, false);
    } else {
      console.error('hotkey save failed', e);
      showToast('Taste speichern fehlgeschlagen ❌', false);
    }
    return false;
  }
}

// Belegt freie Typen der Reihe nach mit 1–9 (Ziffernreihe). Bereits vergebene
// Tasten und Typen mit Kürzel werden übersprungen.
async function autoAssignDigitHotkeys() {
  if (!_markerTypesCache || !_markerTypesCache.length) return;
  const used = new Set(_markerTypesCache.map(t => (t.hotkey || '').toLowerCase()).filter(Boolean));
  let digit = 1;
  let assigned = 0;
  for (const t of _markerTypesCache) {
    if (t.hotkey) continue;
    while (digit <= 9 && used.has(`digit${digit}`)) digit++;
    if (digit > 9) break;
    const hk = `Digit${digit}`;
    try {
      await updateMarkerType(t.id, { hotkey: hk });
      t.hotkey = hk;
      used.add(hk.toLowerCase());
      assigned++;
      digit++;
    } catch (e) {
      console.warn('auto-assign hotkey failed', t.id, e);
    }
  }
  renderMarkerTypesList(false);
  renderMarkerTypeButtonsFromCache();
  showToast(assigned ? `${assigned} Taste(n) automatisch zugewiesen ✅` : 'Keine freien Typen/Tasten für Auto-Belegung', assigned > 0);
}

(function bindMarkerHotkeysOnce() {
  if (window.__orsMarkerHotkeysBound) return;
  window.__orsMarkerHotkeysBound = true;

  function isTypingTarget(el) {
    if (!el) return false;
    const tag = (el.tagName || '').toLowerCase();
    return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable;
  }

  let _lastFire = 0;
  // Capture-Phase: läuft vor allen anderen keydown-Handlern (z.B. Esc schließt Modal)
  window.addEventListener('keydown', (ev) => {
    if (ev.repeat) return;

    // 1) Zuweisungsmodus im Marker-Typen-Modal
    if (_hotkeyCapture) {
      ev.preventDefault();
      ev.stopPropagation();
      if (ev.key === 'Escape') { const c = _hotkeyCapture; _hotkeyCapture = null; c.resolve(null); return; }
      if (ev.key === 'Backspace' || ev.key === 'Delete') { const c = _hotkeyCapture; _hotkeyCapture = null; c.resolve(''); return; }
      const hk = hotkeyFromEvent(ev);
      if (!hk) return; // reiner Modifier → weiter warten
      const c = _hotkeyCapture; _hotkeyCapture = null; c.resolve(hk);
      return;
    }

    // 2) Marker auslösen
    const hk = hotkeyFromEvent(ev);
    if (!hk) return;
    const t = findMarkerTypeByHotkey(hk);
    if (!t) return;
    const typesModal = document.querySelector('#markerTypesModal');
    if (typesModal && !typesModal.classList.contains('hidden')) return; // im Verwaltungsmodal nicht auslösen
    if (isTypingTarget(ev.target) && !isDedicatedHotkeyCode(ev.code)) return;

    ev.preventDefault();
    ev.stopPropagation();
    const now = Date.now();
    if (now - _lastFire < 250) return; // Entprellung bei Doppelauslösung
    _lastFire = now;
    fireMarkerHotkey(t).catch(() => {});
  }, true);
})();

function bindSimHotkeysOnce() {
  if (window.__orsSimHotkeysBound) return;
  window.__orsSimHotkeysBound = true;

  function isTypingTarget(el) {
    if (!el) return false;
    const tag = (el.tagName || '').toLowerCase();
    return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable;
  }

  window.addEventListener('keydown', (ev) => {
    if (!ev.ctrlKey || ev.altKey || ev.metaKey) return;
    if (isTypingTarget(ev.target)) return;

    const key = ev.key.toLowerCase();
    if (key === 's') {
      ev.preventDefault();
      const startBtn = document.getElementById('startBtn');
      if (startBtn) startBtn.click();
      return;
    }
    if (key === 'h') {
      ev.preventDefault();
      stopAllSessionsUI().catch(() => {});
      return;
    }
    if (key === 'delete') {
      ev.preventDefault();
      openDeleteModal();
    }
  });
}

(function bindStopOnce() {
  const btn = document.querySelector('#stopBtn');
  if (!btn || btn.dataset.boundStop === '1') return;
  btn.dataset.boundStop = '1';
  btn.addEventListener('click', (e) => { e.preventDefault(); stopAllSessionsUI(); });
  
})();

(function bindFocusToggle() {
  const toggle = document.querySelector('#focusModeToggle');
  if (!toggle || toggle.dataset.boundFocus === '1') return;
  toggle.dataset.boundFocus = '1';
  toggle.addEventListener('change', () => setFocusMode(toggle.checked));

  // Default: aus, bis User einschaltet
  setFocusMode(false);
})();

// --------------------------------------------------------------
// DELETE: Sessions löschen (Modal) – nur kurz angerissen, da es vorher lief
// --------------------------------------------------------------

async function listSessionsRobust() {
  try {
    const r = await _fetchJson('/api/sessions', { method: 'GET' });
    return Array.isArray(r?.sessions) ? r.sessions : [];
  } catch {
    return [];
  }
}

async function deleteOneSessionRobust(id) {
  try {
    await _fetchJson(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' });
    return true;
  } catch (e) {
    console.warn('Session löschen fehlgeschlagen:', id, e.status, e.message);
    return false;
  }
}

// Bulk delete – wie gehabt, aber per Modal ausgelöst
async function deleteAllSessionsRobust() {
  try {
    await _fetchJson('/api/sessions', { method: 'DELETE' });
    return true;
  } catch (e) {
    console.warn('Alle Sessions löschen fehlgeschlagen:', e.status, e.message);
    return false;
  }
}

// Delete-Modal: UI binden/aufbauen (gleicht deiner letzten Version, nur komprimiert)
function openDeleteModal() {
  const m = document.querySelector('#deleteSessionsModal');
  if (!m) return;
  m.classList.remove('hidden');
  renderDeleteSessionsList();
}

function closeDeleteModal() {
  const m = document.querySelector('#deleteSessionsModal');
  if (!m) return;
  m.classList.add('hidden');
}

async function renderDeleteSessionsList() {
  const wrap = document.querySelector('#deleteSessionsList');
  if (!wrap) return;
  wrap.innerHTML = `<div class="p-4 text-gray-400">Lade Sessions…</div>`;

  const sessions = await listSessionsRobust();

  if (!sessions.length) {
    wrap.innerHTML = `<div class="p-4 text-gray-400">Keine Sessions vorhanden.</div>`;
    updateDeleteSelectionState();
    return;
  }

  const rows = sessions.map(s => {
    const isActive = Number(s.record_is_active) === 1;
    const badgeClass = isActive ? 'bg-yellow-500 text-black' : 'bg-gray-700 text-gray-200';
    const name = s.name || `Session #${s.id ?? '—'}`;
    const started = s.start_time ? new Date(Number(s.start_time) * 1000) : null;
    const ts = started ? started.toLocaleString() : '—';
    return `
      <label class="flex items-center gap-3 p-3 hover:bg-gray-750">
        <input type="checkbox" class="delete-session-checkbox accent-red-600"
               data-id="${s.id ?? ''}" data-active="${isActive ? '1' : '0'}">
        <div class="flex-1">
          <div class="text-white font-medium">${name}</div>
          <div class="text-xs text-gray-400">ID: ${s.id ?? '—'} • Start: ${ts}</div>
        </div>
        <span class="text-xs px-2 py-1 rounded ${badgeClass}">
          ${isActive ? 'aktiv' : 'inaktiv'}
        </span>
      </label>
    `;
  }).join('');

  wrap.innerHTML = rows;

  wrap.querySelectorAll('.delete-session-checkbox').forEach(cb => {
    cb.addEventListener('change', updateDeleteSelectionState);
  });

  updateDeleteSelectionState();
}

function updateDeleteSelectionState() {
  const cbs = [...document.querySelectorAll('.delete-session-checkbox')];
  const selected = cbs.filter(cb => cb.checked);
  const counter = document.querySelector('#deleteSessionsCounter');
  const delBtn = document.querySelector('#deleteSelectedBtn');
  if (counter) counter.textContent = `${selected.length} ausgewählt`;
  if (delBtn) delBtn.disabled = selected.length === 0;
}

async function deleteSelectedSessions() {
  const cbs = [...document.querySelectorAll('.delete-session-checkbox')].filter(cb => cb.checked);
  if (!cbs.length) return;

  const hasActive = cbs.some(cb => cb.dataset.active === '1');
  if (hasActive) {
    try { await stopAllSessionsUI(); } catch { }
  }

  let okAll = true;
  for (const cb of cbs) {
    const id = cb.dataset.id;
    if (!id) continue;
    const ok = await deleteOneSessionRobust(id);
    if (!ok) okAll = false;
  }

  await renderDeleteSessionsList();
  if (typeof window.refreshSessions === 'function') { try { await window.refreshSessions(); } catch { } }
  if (typeof window.refreshStatus === 'function') { try { await window.refreshStatus(); } catch { } }

  showToast(okAll ? 'Ausgewählte Sessions gelöscht ✅' : 'Einige Sessions konnten nicht gelöscht werden ❌', okAll);
}

async function deleteAllFromModal() {
  if (!window.confirm('Wirklich ALLE Sessions löschen?')) return;

  try { await stopAllSessionsUI(); } catch { }
  const ok = await deleteAllSessionsRobust();

  await renderDeleteSessionsList();
  if (typeof window.refreshSessions === 'function') { try { await window.refreshSessions(); } catch { } }
  if (typeof window.refreshStatus === 'function') { try { await window.refreshStatus(); } catch { } }

  showToast(ok ? 'Alle Sessions gelöscht ✅' : 'Einige Sessions blieben übrig ❌', ok);
}

// Bind Delete-Modal
(function bindDeleteModalUIOnce() {
  const openBtn = document.querySelector('#deleteAllSessionsBtn');
  const closeBtn = document.querySelector('#closeDeleteSessionsModal');
  const selAll = document.querySelector('#selectAllSessionsBtn');
  const refresh = document.querySelector('#refreshSessionsListBtn');
  const delSel = document.querySelector('#deleteSelectedBtn');
  const delAll = document.querySelector('#deleteAllInsideModalBtn');

  if (openBtn && !openBtn.dataset.boundOpen) {
    openBtn.dataset.boundOpen = '1';
    openBtn.addEventListener('click', (e) => { e.preventDefault(); openDeleteModal(); });
  }
  if (closeBtn && !closeBtn.dataset.boundClose) {
    closeBtn.dataset.boundClose = '1';
    closeBtn.addEventListener('click', (e) => { e.preventDefault(); closeDeleteModal(); });
  }
  if (selAll && !selAll.dataset.boundSelAll) {
    selAll.dataset.boundSelAll = '1';
    selAll.addEventListener('click', () => {
      const cbs = document.querySelectorAll('.delete-session-checkbox');
      const allChecked = [...cbs].every(cb => cb.checked);
      cbs.forEach(cb => { cb.checked = !allChecked; });
      updateDeleteSelectionState();
    });
  }
  if (refresh && !refresh.dataset.boundRefresh) {
    refresh.dataset.boundRefresh = '1';
    refresh.addEventListener('click', () => renderDeleteSessionsList());
  }
  if (delSel && !delSel.dataset.boundDelSel) {
    delSel.dataset.boundDelSel = '1';
    delSel.addEventListener('click', () => deleteSelectedSessions());
  }
  if (delAll && !delAll.dataset.boundDelAll) {
    delAll.dataset.boundDelAll = '1';
    delAll.addEventListener('click', () => deleteAllFromModal());
  }

  const modal = document.querySelector('#deleteSessionsModal');
  if (modal && !modal.dataset.boundBackdrop) {
    modal.dataset.boundBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) closeDeleteModal();
    });
  }

  window.addEventListener('keydown', (ev) => {
    const m = document.querySelector('#deleteSessionsModal');
    if (!m || m.classList.contains('hidden')) return;
    if (ev.key === 'Escape') closeDeleteModal();
  });
})();

// --------------------------------------------------------------
// VIEWERS: API + Modal + Badge
// --------------------------------------------------------------

async function listViewersRobust() {
  try {
    const r = await _fetchJson('/api/viewers', { method: 'GET' });
    return Array.isArray(r?.viewers) ? r.viewers : [];
  } catch {
    return [];
  }
}

function viewerLogicalIdFromUrl(url) {
  if (!url) return null;
  const match = String(url).match(/\/viewer\/(\d+)/);
  if (!match) return null;
  const id = Number(match[1]);
  return Number.isFinite(id) ? id : null;
}

function viewerRoleFromId(id) {
  if (!Number.isFinite(id)) return 'participant';
  if (id >= 1 && id <= 49) return 'instructor';
  if (id >= 50 && id <= 99) return 'participant';
  if (id >= 100) return 'other';
  return 'participant';
}

function viewerRoleLabel(role) {
  switch ((role || '').toLowerCase()) {
    case 'instructor':
      return 'Ausbilder (ID 1–49)';
    case 'participant':
      return 'Teilnehmer (ID 50–99)';
    case 'other':
      return 'Sonstiges (ID 100+)';
    default:
      return 'Teilnehmer (ID 50–99)';
  }
}

async function createViewer(body) {
  return _fetchJson('/api/viewers', { method: 'POST', body });
}

async function updateViewer(id, patch) {
  return _fetchJson(`/api/viewers/${encodeURIComponent(id)}`, { method: 'PATCH', body: patch });
}

async function deleteViewer(id) {
  return _fetchJson(`/api/viewers/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

let _viewerCache = [];
let _viewerFilter = '';

function openViewerModal() {
  const m = document.querySelector('#viewerModal');
  if (!m) return;
  m.classList.remove('hidden');
  document.documentElement.classList.add('overflow-hidden');
  renderViewerList(true);
}

function closeViewerModal() {
  const m = document.querySelector('#viewerModal');
  if (!m) return;
  m.classList.add('hidden');
  document.documentElement.classList.remove('overflow-hidden');
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}

function viewerRowHtml(v) {
  const isActive = !!v.active;
  const isSelected = !!v.selected;
  const badgeClass = isActive ? 'bg-green-600 text-white' : 'bg-gray-700 text-gray-200';
  const type = v.type || 'unbekannt';
  const url = v.url || '—';
  const alias = (v.alias || '').trim();
  const baseName = (v.name || '').trim() || `Viewer #${v.id}`;
  const displayName = alias || baseName;
  const secondaryName = alias && alias !== baseName ? `Name: ${baseName}` : '';
  return `
    <div class="p-4 flex items-start gap-4">
      <input type="checkbox" class="mt-1 accent-blue-500"
             data-action="toggle-selected" data-id="${v.id}" ${isSelected ? 'checked' : ''}>
      <div class="flex-1">
        <div class="flex items-center gap-2">
          <div class="text-white font-medium">${escapeHtml(displayName)}</div>
          <span class="text-[10px] px-2 py-0.5 rounded ${v.online ? 'bg-emerald-700 text-white' : 'bg-gray-800 text-gray-400'}" title="Bildschirm gerade verbunden?">${v.online ? '🟢 online' : 'offline'}</span>
          <span class="text-[10px] px-2 py-0.5 rounded ${badgeClass}">${isActive ? 'aktiv' : 'inaktiv'}</span>
          <span class="text-[10px] px-2 py-0.5 rounded bg-gray-700 text-gray-200">${escapeHtml(type)}</span>
          <span class="text-[10px] px-2 py-0.5 rounded bg-gray-700 text-gray-300">ID ${escapeHtml(v.id)}</span>
        </div>
        ${secondaryName ? `<div class="text-xs text-gray-400 mt-0.5">${escapeHtml(secondaryName)}</div>` : ''}
        <div class="text-xs text-gray-400 break-all">${escapeHtml(url)}</div>
        <div class="mt-2 flex gap-2">
          <button data-action="toggle-active" data-id="${v.id}"
                  class="px-2 py-1 rounded text-xs ${isActive ? 'bg-yellow-600 hover:bg-yellow-500' : 'bg-green-600 hover:bg-green-500'} text-white">
            ${isActive ? 'Deaktivieren' : 'Aktivieren'}
          </button>
          <button data-action="rename" data-id="${v.id}"
                  class="px-2 py-1 rounded text-xs bg-gray-700 hover:bg-gray-600 text-white">Alias</button>
          <button data-action="delete" data-id="${v.id}"
                  class="px-2 py-1 rounded text-xs bg-red-600 hover:bg-red-500 text-white">Löschen</button>
        </div>
      </div>
    </div>
  `;
}

function updateViewerBadge() {
  const badge = document.querySelector('#viewerBtnBadge');
  if (!badge) return;
  const active = _viewerCache.filter(v => v.active).length;
  const selected = _viewerCache.filter(v => v.selected).length;
  const showNum = selected || active;
  if (showNum) {
    badge.textContent = String(showNum);
    badge.classList.remove('hidden');
  } else {
    badge.classList.add('hidden');
  }
}

// Viewer-Listen: im globalen Viewer-Dialog (base.html) und in den Einstellungen
// (Reiter „Kameras & Räume“ → Aktive Viewer). Früher hatten beide die ID „viewerList“;
// auf der Einstellungsseite landete alles in der Seitenliste, der Dialog blieb auf
// „Lade Viewer…“ stehen und die Seitenliste füllte sich nur, wenn der Dialog geöffnet wurde.
function viewerListTargets() {
  return [document.querySelector('#viewerModal #viewerList'), document.getElementById('settingsViewerList')]
    .filter(Boolean);
}

async function renderViewerList(forceFetch = false) {
  const targets = viewerListTargets();
  const stats = document.querySelector('#viewerStats');
  if (!targets.length) return;

  if (forceFetch || !_viewerCache.length) {
    // „Lade…“ nur, solange noch nichts angezeigt wird (kein Flackern beim Aktualisieren)
    if (!_viewerCache.length) targets.forEach((el) => { el.innerHTML = `<div class="p-4 text-gray-400">Lade Viewer…</div>`; });
    _viewerCache = await listViewersRobust();
  }
  targets.forEach((list) => renderViewerListInto(list));


  if (stats) {
    const total = _viewerCache.length;
    const active = _viewerCache.filter(v => v.active).length;
    const selected = _viewerCache.filter(v => v.selected).length;
    stats.textContent = `${active} aktiv • ${selected} ausgewählt • ${total} gesamt`;
  }
}

function renderViewerListInto(list) {
  // Das Filterfeld gibt es nur im Dialog
  const filter = list.closest('#viewerModal') ? (_viewerFilter || '').toLowerCase() : '';
  let rows = _viewerCache.filter(v => {
    const s = `${v.alias || ''} ${v.name || ''} ${v.type || ''} ${v.url || ''}`.toLowerCase();
    return !filter || s.includes(filter);
  });

  if (!rows.length) {
    list.innerHTML = `<div class="p-6 text-gray-400">Keine Viewer gefunden.</div>`;
  } else {
    list.innerHTML = rows.map(v => viewerRowHtml(v)).join('');
  }

  list.querySelectorAll('[data-action="toggle-active"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.id;
      const v = _viewerCache.find(x => String(x.id) === String(id));
      if (!v) return;
      const next = !v.active;
      try { await updateViewer(id, { active: next }); v.active = next; updateViewerBadge(); renderViewerList(false); }
      catch { showToast('Aktivieren/Deaktivieren fehlgeschlagen ❌', false); }
    });
  });

  list.querySelectorAll('[data-action="toggle-selected"]').forEach(cb => {
    cb.addEventListener('change', async () => {
      const id = cb.dataset.id;
      const v = _viewerCache.find(x => String(x.id) === String(id));
      if (!v) return;
      const next = !!cb.checked;
      try { await updateViewer(id, { selected: next }); v.selected = next; updateViewerBadge(); }
      catch { cb.checked = !!v.selected; showToast('Ändern der Auswahl fehlgeschlagen ❌', false); }
    });
  });

  list.querySelectorAll('[data-action="rename"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.id;
      const v = _viewerCache.find(x => String(x.id) === String(id));
      if (!v) return;
      const alias = prompt('Alias für Viewer (leer = zurücksetzen):', v.alias || '');
      if (alias == null) return;
      const trimmed = alias.trim();
      const payload = { alias: trimmed ? trimmed : null };
      try {
        await updateViewer(id, payload);
        v.alias = trimmed ? trimmed : null;
        renderViewerList(false);
        updateViewerBadge();
      } catch {
        showToast('Alias setzen fehlgeschlagen ❌', false);
      }
    });
  });

  list.querySelectorAll('[data-action="delete"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.id;
      if (!confirm('Viewer wirklich löschen?')) return;
      try { await deleteViewer(id); _viewerCache = _viewerCache.filter(v => String(v.id) !== String(id)); renderViewerList(false); updateViewerBadge(); }
      catch { showToast('Löschen fehlgeschlagen ❌', false); }
    });
  });

}

// Einstellungen → Aktive Viewer: selbst laden und den Online-Status aktuell halten
(function autoLoadSettingsViewerList() {
  if (!document.getElementById('settingsViewerList')) return;
  renderViewerList(true);
  setInterval(() => { if (!document.hidden) renderViewerList(true); }, 10000);
})();

(function bindViewerUIOnce() {
  const btn = document.querySelector('#viewerBtn');
  const close = document.querySelector('#closeViewerModal');
  const addBtn = document.querySelector('#addViewerBtn');
  const refreshBtn = document.querySelector('#refreshViewersBtn');
  const selectAllBtn = document.querySelector('#selectAllViewersBtn');
  const selectNoneBtn = document.querySelector('#selectNoneViewersBtn');
  const filterInput = document.querySelector('#viewerFilterInput');

  if (btn && !btn.dataset.boundViewerOpen) {
    btn.dataset.boundViewerOpen = '1';
    btn.addEventListener('click', (e) => { e.preventDefault(); openViewerModal(); });
  }
  if (close && !close.dataset.boundViewerClose) {
    close.dataset.boundViewerClose = '1';
    close.addEventListener('click', (e) => { e.preventDefault(); closeViewerModal(); });
  }
  if (addBtn && !addBtn.dataset.boundViewerAdd) {
    addBtn.dataset.boundViewerAdd = '1';
    addBtn.addEventListener('click', async () => {
      const name = prompt('Viewer-Name:', 'Viewer');
      if (name == null) return;
    const type = prompt('Typ (z.B. webrtc, dash, rtsp):', 'webrtc') || 'webrtc';
      const url = prompt('URL (optional, je nach Typ):', '') || null;
      try {
        await createViewer({ name, type, url, selected: false, active: false });
        _viewerCache = [];
        await renderViewerList(true);
        updateViewerBadge();
      } catch {
        showToast('Viewer anlegen fehlgeschlagen ❌', false);
      }
    });
  }
  if (refreshBtn && !refreshBtn.dataset.boundViewerRefresh) {
    refreshBtn.dataset.boundViewerRefresh = '1';
    refreshBtn.addEventListener('click', () => renderViewerList(true));
  }
  if (selectAllBtn && !selectAllBtn.dataset.boundViewerSelAll) {
    selectAllBtn.dataset.boundViewerSelAll = '1';
    selectAllBtn.addEventListener('click', async () => {
      const ids = _viewerCache.map(v => v.id);
      try {
        await Promise.all(ids.map(id => updateViewer(id, { selected: true })));
        _viewerCache.forEach(v => { v.selected = true; });
        renderViewerList(false);
        updateViewerBadge();
      } catch { showToast('Alle auswählen fehlgeschlagen ❌', false); }
    });
  }
  if (selectNoneBtn && !selectNoneBtn.dataset.boundViewerSelNone) {
    selectNoneBtn.dataset.boundViewerSelNone = '1';
    selectNoneBtn.addEventListener('click', async () => {
      const ids = _viewerCache.map(v => v.id);
      try {
        await Promise.all(ids.map(id => updateViewer(id, { selected: false })));
        _viewerCache.forEach(v => { v.selected = false; });
        renderViewerList(false);
        updateViewerBadge();
      } catch { showToast('Auswahl leeren fehlgeschlagen ❌', false); }
    });
  }
  if (filterInput && !filterInput.dataset.boundViewerFilter) {
    filterInput.dataset.boundViewerFilter = '1';
    filterInput.addEventListener('input', () => {
      _viewerFilter = filterInput.value || '';
      renderViewerList(false);
    });
  }

  const modal = document.querySelector('#viewerModal');
  if (modal && !modal.dataset.boundViewerBackdrop) {
    modal.dataset.boundViewerBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) closeViewerModal();
    });
  }

  window.addEventListener('keydown', (ev) => {
    const m = document.querySelector('#viewerModal');
    if (!m || m.classList.contains('hidden')) return;
    if (ev.key === 'Escape') closeViewerModal();
  });

onWindowLoaded(async () => {
  try {
    _viewerCache = await listViewersRobust();
    updateViewerBadge();
  } catch {}

  try {
    _markerTypesCache = await listMarkerTypes();
    renderMarkerTypeButtonsFromCache();
  } catch (e) {
    console.error('MarkerTypes load failed', e);
  }

  try { await refreshMarkersUI(); } catch {}
  try { await startMarkerPoll(); } catch {}
});

})();

// --------------------------------------------------------------
// MARKER: Modal bei keiner Session + Liste + Poll
// --------------------------------------------------------------

function openNoSessionMarkerModal() {
  const m = document.querySelector('#noSessionMarkerModal');
  if (!m) {
    showToast('Marker setzen nur während einer Session möglich ❌', false);
    return;
  }
  m.classList.remove('hidden');
}

function closeNoSessionMarkerModal() {
  const m = document.querySelector('#noSessionMarkerModal');
  if (!m) return;
  m.classList.add('hidden');
}

(function bindNoSessionMarkerModalOnce() {
  const modal = document.querySelector('#noSessionMarkerModal');
  if (!modal || modal.dataset.bound) return;
  modal.dataset.bound = '1';

  const closeBtn = document.querySelector('#closeNoSessionMarkerModal');
  const okBtn = document.querySelector('#okNoSessionMarkerBtn');

  [closeBtn, okBtn].forEach(btn => {
    if (!btn) return;
    btn.addEventListener('click', (e) => {
      e.preventDefault();
      closeNoSessionMarkerModal();
    });
  });

  modal.addEventListener('click', (e) => {
    if (e.target === modal) closeNoSessionMarkerModal();
  });

  window.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && !modal.classList.contains('hidden')) {
      closeNoSessionMarkerModal();
    }
  });
})();

async function getSimStatus() {
  if (_simStatusCache) return _simStatusCache;
  try {
    _simStatusCache = await _fetchJson('/api/sim/status', { method: 'GET' });
    return _simStatusCache;
  } catch {
    return null;
  }
}

function setSimStatusCache(obj) {
  _simStatusCache = obj;
}

async function listMarkersForSession(sessionId) {
  const r = await _fetchJson(`/api/sessions/${encodeURIComponent(sessionId)}/markers`, { method: 'GET' });
  return Array.isArray(r?.markers) ? r.markers : [];
}

async function createFreeTextMarker(text) {
  const typeId = await ensureFreitextTypeId();
  const body = { text, marker_type_id: typeId };
  return await _fetchJson('/api/markers', {
    method: 'POST',
    body,
  });
}

async function deleteMarker(markerId) {
  return await _fetchJson(`/api/markers/${encodeURIComponent(markerId)}`, { method: 'DELETE' });
}

function secondsToClock(s) {
  s = Math.max(0, Number(s) || 0);
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${String(m).padStart(2, '0')}:${String(r).padStart(2, '0')}`;
}

function renderMarkersList(markers) {
  // Rendert in jedes Marker-Listen-Ziel (eingebettet + angeheftete Seitenleiste),
  // damit beide Ansichten immer denselben Stand zeigen, unabhängig davon, welche
  // gerade sichtbar ist (siehe pinMarkersSideToggle).
  const targets = Array.from(document.querySelectorAll('.markers-list-target'));
  if (!targets.length) return;

  if (!markers.length) {
    targets.forEach(list => { list.innerHTML = `<div class="alert alert-info">Noch keine Marker gesetzt.</div>`; });
    return;
  }

  // Aktuellen User bestimmen
  const me = (window._currentUser && window._currentUser.username) || localStorage.getItem('username') || '';
  const own = [];
  const others = [];
  markers.forEach(m => {
    const uname = (m.username || '').trim();
    if (me && uname && uname === me) {
      own.push(m);
    } else {
      others.push(m);
    }
  });
  const sortNewestFirst = (a, b) => {
    const ta = a.record_time ?? a.timestamp ?? 0;
    const tb = b.record_time ?? b.timestamp ?? 0;
    if (tb !== ta) return tb - ta;
    return (b.id ?? 0) - (a.id ?? 0);
  };
  own.sort(sortNewestFirst);
  others.sort(sortNewestFirst);

  const renderGroup = (items, title) => {
    if (!items.length) return '';
    return `
      <div class="space-y-2">
        <div class="text-xs font-semibold text-gray-400 uppercase tracking-wide">${escapeHtml(title)}</div>
        ${items.map(renderMarkerCard).join('')}
      </div>
    `;
  };

  const renderMarkerCard = (m) => {
    const label = m.marker_type || 'Marker';
    const text  = (m.text || '').trim();
    const who   = m.username ? ` • ${escapeHtml(m.username)}` : ' • not logged in';
    const tSec  = m.record_time ?? m.timestamp ?? 0;
    const tStr  = secondsToClock(tSec);
    const color = m.color || '#4b5563';

    return `
      <div class="p-3 rounded bg-gray-800 flex items-start justify-between gap-3">
        <div>
          <div class="flex items-center gap-2">
            <span class="inline-block w-3 h-3 rounded-full"
                  style="background:${color}"></span>
            <span class="text-sm font-semibold"
                  style="color:${color}">${escapeHtml(label)}</span>
          </div>

          ${text
            ? `<div class="text-white mt-1">${escapeHtml(text)}</div>`
            : `<div class="text-gray-300 mt-1 text-sm">Marker gesetzt</div>`}

          <div class="text-xs text-gray-500 mt-1">
            t=${tStr}${who}
          </div>
        </div>

        <div class="flex flex-col items-end gap-2">
          <div class="text-3xl font-black leading-none opacity-40"
               style="color:${color}">#${m.id}</div>
          <button class="text-xs bg-red-600 hover:bg-red-500 text-white px-2 py-1 rounded"
                  data-action="delete-marker" data-id="${m.id}">
            Löschen
          </button>
        </div>
      </div>
    `;
  };

  const html = [
    renderGroup(own, 'Eigene Marker'),
    renderGroup(others, 'Marker von anderen'),
  ].filter(Boolean).join('<div class="h-4"></div>');

  const finalHtml = html || `<div class="alert alert-info">Noch keine Marker gesetzt.</div>`;

  targets.forEach(list => {
    list.innerHTML = finalHtml;
    // Delete-Handling (je Ziel-Element gebunden, Marker-IDs sind sonst doppelt im DOM)
    list.querySelectorAll('[data-action="delete-marker"]').forEach(btn => {
      btn.addEventListener('click', async () => {
        const id = btn.dataset.id;
        if (!id) return;
        if (!confirm('Marker wirklich löschen?')) return;
        try {
          await _fetchJson(`/api/markers/${encodeURIComponent(id)}`, { method: 'DELETE' });
          await refreshMarkersUI();
          showToast('Marker gelöscht ✅', true);
        } catch (e) {
          console.error('delete marker failed', e);
          showToast('Marker löschen fehlgeschlagen ❌', false);
        }
      });
    });
  });
}


// --- Marker-Liste optional an der Seite anheften -------------------
const PIN_MARKERS_SIDE_KEY = 'ors_pin_markers_side';

function isMarkerPinWanted() {
  try { return localStorage.getItem(PIN_MARKERS_SIDE_KEY) === '1'; } catch { return false; }
}

function setMarkerPinWanted(on) {
  try { localStorage.setItem(PIN_MARKERS_SIDE_KEY, on ? '1' : '0'); } catch {}
  const toggle = document.getElementById('pinMarkersSideToggle');
  if (toggle) toggle.checked = !!on;
}

// Zeigt die Marker-Liste entweder eingebettet im Marker-Kartenbereich (Standard)
// oder angeheftet in der Seitenleiste (Toggle „An der Seite“), nie beides zugleich.
// Die Seitenleiste existiert nur sinnvoll bei laufender Session.
function updateMarkerPinLayout(sessionActive) {
  const pinned = isMarkerPinWanted() && !!sessionActive;
  const sideCol = document.getElementById('markerSideCol');
  const inlineWrap = document.getElementById('markerListInlineWrap');
  if (sideCol) sideCol.classList.toggle('is-pinned', pinned);
  if (inlineWrap) inlineWrap.classList.toggle('hidden', pinned);
}

(function bindMarkerPinToggleOnce() {
  const toggle = document.getElementById('pinMarkersSideToggle');
  const unpinBtn = document.getElementById('unpinMarkersSideBtn');
  const sideRefreshBtn = document.getElementById('refreshMarkersSideBtn');
  if (toggle && !toggle.dataset.bound) {
    toggle.dataset.bound = '1';
    toggle.checked = isMarkerPinWanted();
    toggle.addEventListener('change', async () => {
      setMarkerPinWanted(toggle.checked);
      const st = await getSimStatus();
      updateMarkerPinLayout(!!st?.active);
    });
  }
  if (unpinBtn && !unpinBtn.dataset.bound) {
    unpinBtn.dataset.bound = '1';
    unpinBtn.addEventListener('click', () => {
      setMarkerPinWanted(false);
      updateMarkerPinLayout(false);
    });
  }
  if (sideRefreshBtn && !sideRefreshBtn.dataset.bound) {
    sideRefreshBtn.dataset.bound = '1';
    sideRefreshBtn.addEventListener('click', () => refreshMarkersUI().catch(() => {}));
  }
})();

async function refreshMarkersUI() {
  const st = await getSimStatus();
  const active = !!st?.active;
  const sid = st?.session_id;
  const emergencyWrap = document.querySelector('#emergencyRestartWrap');
  if (emergencyWrap) {
    emergencyWrap.classList.toggle('hidden', !active);
  }
  if (active) {
    await prepareLiveRoomSelectOnce(sid);
  }
  updateLiveRoomUI(st);

  const container = document.querySelector('#markerContainer');
  if (container) {
    container.style.maxHeight = active ? 'none' : '0px';
    container.style.overflow = active ? 'visible' : 'hidden';
  }

  updateMarkerPinLayout(active);

  if (active && sid) {
    const markers = await listMarkersForSession(sid);
    renderMarkersList(markers);
  } else {
    renderMarkersList([]);
  }
}

let _markerPollTimer = null;
let _lastSessionIdForMarkers = null;

async function startMarkerPoll() {
  clearMarkerPoll();
  const st = await getSimStatus();
  if (!st?.active || !st?.session_id) return;

  _lastSessionIdForMarkers = st.session_id;
  await refreshMarkersUI();

  _markerPollTimer = setInterval(async () => {
    try {
      const st2 = await getSimStatus();
      if (!st2?.active || st2.session_id !== _lastSessionIdForMarkers) {
        clearMarkerPoll();
        if (st2?.active) startMarkerPoll();
        return;
      }
      await refreshMarkersUI();
    } catch { }
  }, 5000);
}

function clearMarkerPoll() {
  if (_markerPollTimer) {
    clearInterval(_markerPollTimer);
    _markerPollTimer = null;
  }
}

const MARKER_MODE_KEY = 'simulate_marker_mode';

function setMarkerMode(mode) {
  const marker = document.getElementById('markerModeMarker');
  const amls = document.getElementById('markerModeAmls');
  const toggle = document.getElementById('markerModeToggle');
  const isAmls = mode === 'amls';
  console.log('[simulate] setMarkerMode', { mode, isAmls, hasMarker: !!marker, hasAmls: !!amls, toggleChecked: toggle?.checked });
  if (toggle) toggle.checked = isAmls;
  if (marker) {
    marker.classList.toggle('hidden', isAmls);
    marker.hidden = isAmls;
    marker.style.display = isAmls ? 'none' : '';
    marker.setAttribute('aria-hidden', isAmls ? 'true' : 'false');
  }
  if (amls) {
    amls.classList.toggle('hidden', !isAmls);
    amls.hidden = !isAmls;
    amls.style.display = isAmls ? '' : 'none';
    amls.setAttribute('aria-hidden', isAmls ? 'false' : 'true');
  }
  try {
    localStorage.setItem(MARKER_MODE_KEY, isAmls ? 'amls' : 'marker');
  } catch {}
}

function bindMarkerModeToggleOnce() {
  const toggle = document.getElementById('markerModeToggle');
  if (!toggle || toggle.dataset.bound) return;
  toggle.dataset.bound = '1';
  console.log('[simulate] bindMarkerModeToggleOnce', { toggleFound: !!toggle });
  let saved = null;
  try {
    saved = localStorage.getItem(MARKER_MODE_KEY);
  } catch {}
  setMarkerMode(saved === 'amls' ? 'amls' : 'marker');
  toggle.addEventListener('change', () => {
    console.log('[simulate] markerModeToggle change', { checked: toggle.checked });
    setMarkerMode(toggle.checked ? 'amls' : 'marker');
  });
  const sortNewestFirst = (a, b) => {
    const ta = a.record_time ?? a.timestamp ?? 0;
    const tb = b.record_time ?? b.timestamp ?? 0;
    if (tb !== ta) return tb - ta;
    return (b.id ?? 0) - (a.id ?? 0);
  };
  toggle.addEventListener('input', () => {
    console.log('[simulate] markerModeToggle input', { checked: toggle.checked });
    setMarkerMode(toggle.checked ? 'amls' : 'marker');
  });
}

function formatAmlsText(kind, item, value) {
  if (value === 'gesetzt' || !value) return `${kind}: ${item}`;
  if (!item) return `${kind}: ${value}`;
  return `${kind} ${item}: ${value}`;
}

async function createAmlsMarker(text) {
  const typeId = await ensureAmlsTypeId();
  return await createTypedMarker(typeId, text);
}

function bindAmlsControlsOnce() {
  const container = document.getElementById('markerModeAmls');
  if (!container || container.dataset.bound === '1') return;
  container.dataset.bound = '1';

  container.querySelectorAll('[data-amls-kind][data-amls-item][data-amls-value]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const kind = btn.dataset.amlsKind || '';
      const item = btn.dataset.amlsItem || '';
      const value = btn.dataset.amlsValue || '';
      const text = formatAmlsText(kind, item, value);
      try {
        await createAmlsMarker(text);
        const group = btn.closest('[data-amls-group]');
        if (group) {
          group.querySelectorAll('[data-amls-kind][data-amls-item][data-amls-value]').forEach(other => {
            other.classList.remove('btn-active');
          });
          btn.classList.add('btn-active');
        }
        await refreshMarkersUI();
        showToast('AMLS gesetzt ✅', true);
      } catch (e) {
        if (e.status === 409) {
          openNoSessionMarkerModal();
        } else {
          console.error('createAmlsMarker failed', e);
          showToast('AMLS setzen fehlgeschlagen ❌', false);
        }
      }
    });
  });

  const funnelBtn = document.getElementById('amlsFunnelSubmit');
  const funnelInput = document.getElementById('amlsFunnelInput');
  if (funnelBtn && funnelInput) {
    const submit = async () => {
      const text = (funnelInput.value || '').trim();
      if (!text) {
        showToast('Diagnosetrichter bitte ausfüllen', false);
        return;
      }
      try {
        await createAmlsMarker(`Diagnosetrichter: ${text}`);
        funnelInput.value = '';
        await refreshMarkersUI();
        showToast('Diagnosetrichter gesetzt ✅', true);
      } catch (e) {
        if (e.status === 409) {
          openNoSessionMarkerModal();
        } else {
          console.error('diagnosetrichter failed', e);
          showToast('Diagnosetrichter setzen fehlgeschlagen ❌', false);
        }
      }
    };
    funnelBtn.addEventListener('click', submit);
    funnelInput.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') submit();
    });
  }
}

(function bindDebriefSwitchModal() {
  const modal = document.getElementById('debriefSwitchModal');
  const cancelBtn = document.getElementById('debriefSwitchCancel');
  const confirmBtn = document.getElementById('debriefSwitchConfirm');
  const viewerSelect = document.getElementById('debriefViewerSelect');
  const carryViewer = document.getElementById('debriefCarryViewer');
  const carrySession = document.getElementById('debriefCarrySession');
  const markerList = document.getElementById('debriefMarkerTypeList');
  const markerOnly = document.getElementById('debriefMarkerFilterOnly');

  function close() {
    if (modal) modal.classList.add('hidden');
  }

  if (cancelBtn) cancelBtn.addEventListener('click', (e) => {
    e.preventDefault();
    close();
  });
  if (modal) {
    modal.addEventListener('click', (e) => {
      if (e.target === modal) close();
    });
  }
  if (confirmBtn) {
    confirmBtn.addEventListener('click', () => {
      const params = new URLSearchParams();
      const sessionId = modal?.dataset.sessionId;
      if (carrySession?.checked && sessionId) {
        params.set('session_id', sessionId);
      }
      if (carryViewer?.checked && viewerSelect?.value) {
        params.set('viewer_id', viewerSelect.value);
      }
      const selectedIds = [];
      markerList?.querySelectorAll('input[type="checkbox"][data-marker-id]').forEach(cb => {
        if (cb.checked) selectedIds.push(cb.dataset.markerId);
      });
      if (selectedIds.length) {
        params.set('marker_ids', selectedIds.join(','));
      }
      if (markerOnly?.checked) {
        params.set('marker_filter', '1');
      }
      const suffix = params.toString();
      window.location.href = `/ui/debrief${suffix ? `?${suffix}` : ''}`;
    });
  }

  window.openDebriefSwitchModal = function(sessionId) {
    if (!modal) return;
    modal.dataset.sessionId = sessionId ? String(sessionId) : '';
    if (carrySession) {
      carrySession.checked = !!sessionId;
      carrySession.disabled = !sessionId;
    }

    if (viewerSelect) {
      const selected = _viewerCache.find(v => v.selected) || _viewerCache[0];
      const options = _viewerCache.map(v => {
        const alias = (v.alias || '').trim();
        const baseName = (v.name || '').trim() || v.url || 'Viewer';
        const displayName = alias || baseName;
        const secondary = alias && alias !== baseName ? ` (${baseName})` : '';
        const label = `${v.online ? '🟢' : '⚪'} #${v.id} • ${displayName}${secondary}`;
        const sel = selected && String(selected.id) === String(v.id) ? ' selected' : '';
        return `<option value="${v.id}"${sel}>${escapeHtml(label)}</option>`;
      });
      viewerSelect.innerHTML = `<option value="">— Viewer wählen —</option>${options.join('')}`;
    }

    if (markerList) {
      markerList.innerHTML = '<span class="text-xs text-gray-500">Marker werden geladen…</span>';
      if (sessionId) {
        listMarkersForSession(sessionId).then((markers) => {
          if (!markers.length) {
            markerList.innerHTML = '<span class="text-xs text-gray-500">Keine Marker vorhanden.</span>';
            return;
          }
          markerList.innerHTML = markers.map(m => {
            const tSec = m.record_time ?? m.timestamp ?? 0;
            const tStr = secondsToClock(tSec);
            const label = escapeHtml(m.marker_type || 'Marker');
            const text = escapeHtml(m.text || 'Marker gesetzt');
            const preselected = !!m.preselected;
            return `
              <label class="flex items-start gap-3 text-xs p-2 rounded bg-gray-800/70 border border-gray-700">
                <input type="checkbox" class="checkbox checkbox-xs mt-1" data-marker-id="${m.id}" ${preselected ? 'checked' : ''} />
                <div class="w-16 flex-shrink-0">
                  <div class="text-center text-lg font-black leading-none text-white/80 mb-1">#${m.id}</div>
                  <img src="/api/markers/${m.id}/snapshot" alt="" class="w-16 h-10 object-cover rounded border border-gray-700" data-marker-snapshot="1" />
                </div>
                <div class="flex-1">
                  <div class="font-semibold">${label}</div>
                  <div class="text-gray-400">t=${tStr}</div>
                  <div class="text-gray-300">${text}</div>
                </div>
              </label>
            `;
          }).join('');

          markerList.querySelectorAll('img[data-marker-snapshot="1"]').forEach((img) => {
            img.addEventListener('error', () => {
              img.classList.add('hidden');
            }, { once: true });
          });

          markerList.querySelectorAll('input[type="checkbox"][data-marker-id]').forEach(cb => {
            cb.addEventListener('change', async () => {
              const id = cb.dataset.markerId;
              try {
                await _fetchJson(`/api/markers/${encodeURIComponent(id)}`, {
                  method: 'PATCH',
                  body: { preselected: cb.checked },
                });
              } catch (e) {
                console.error('preselect marker failed', e);
                cb.checked = !cb.checked;
                toast('Vorauswahl konnte nicht gespeichert werden ❌', false);
              }
            });
          });
        }).catch(() => {
          markerList.innerHTML = '<span class="text-xs text-red-400">Marker laden fehlgeschlagen.</span>';
        });
      } else {
        markerList.innerHTML = '<span class="text-xs text-gray-500">Keine Session verfügbar.</span>';
      }
    }

    if (markerOnly) markerOnly.checked = false;
    if (modal) modal.classList.remove('hidden');
  };
})();

(function bindEmergencyRestartOnce() {
  const btn = document.querySelector('#emergencyRestartBtn');
  if (!btn || btn.dataset.boundEmergency === '1') return;
  btn.dataset.boundEmergency = '1';
  btn.addEventListener('click', (e) => {
    e.preventDefault();
    emergencyRestartSessionUI().catch(() => {});
  });
})();

(function bindPapierMarkerOnce() {
  const btn = document.getElementById('papierMarkerBtn');
  if (!btn || btn.dataset.bound === '1') return;
  btn.dataset.bound = '1';

  btn.addEventListener('click', async () => {
    try {
      const typeId = await ensurePapierTypeId();
      await createTypedMarker(typeId);
      await refreshMarkersUI();
      showToast('Papier-Marker gesetzt ✅', true);
    } catch (e) {
      if (e.status === 409) {
        openNoSessionMarkerModal();
      } else {
        console.error('createPapierMarker failed', e);
        showToast('Papier-Marker setzen fehlgeschlagen ❌', false);
      }
    }
  });
})();

(function bindMarkerNoteModal() {
  const modal = document.getElementById('markerNoteModal');
  const closeBtn = document.getElementById('closeMarkerNoteModal');
  const saveBtn = document.getElementById('markerNoteSave');
  const skipBtn = document.getElementById('markerNoteSkip');
  const input = document.getElementById('markerNoteInput');
  const suggestions = document.getElementById('markerNoteSuggestions');

  function hide() { if (modal) modal.classList.add('hidden'); }

  function applySuggestion(text) {
    if (!input) return;
    input.value = text;
    input.focus();
  }

  function renderSuggestions(list) {
    if (!suggestions) return;
    if (!list || list.length === 0) {
      suggestions.innerHTML = '<span class="text-xs text-gray-500">Keine Textbausteine verfügbar.</span>';
      return;
    }
    suggestions.innerHTML = list.map(t =>
      `<button type="button" class="btn btn-xs btn-ghost border border-gray-700" data-suggestion="${escapeHtml(t)}">${escapeHtml(t)}</button>`
    ).join('');
    suggestions.querySelectorAll('[data-suggestion]').forEach(btn => {
      btn.addEventListener('click', () => applySuggestion(btn.dataset.suggestion || ''));
    });
  }

  renderSuggestions(_markerNoteCache);

  if (closeBtn) closeBtn.addEventListener('click', () => hide());
  if (modal) {
    modal.addEventListener('click', (e) => {
      if (e.target === modal) hide();
    });
  }

  async function submit(withoutText = false) {
    const id = window.__pendingMarkerTypeId;
    if (!id) return hide();
    const txt = withoutText ? '' : (input?.value || '').trim();
    try {
      await createTypedMarker(id, txt || undefined);
      hide();
      if (input) input.value = '';
      await refreshMarkersUI();
      showToast('Marker gesetzt ✅', true);
    } catch (e) {
      if (e.status === 409) {
        hide();
        openNoSessionMarkerModal();
      } else {
        console.error('createTypedMarker failed', e);
        showToast('Marker setzen fehlgeschlagen ❌', false);
      }
    }
  }

  if (saveBtn) saveBtn.addEventListener('click', () => submit(false));
  if (skipBtn) skipBtn.addEventListener('click', () => submit(true));
  if (input) input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') submit(false);
    if (ev.key === 'Escape') hide();
  });

  window.openMarkerNoteModal = function(markerTypeId) {
    const label = document.getElementById('markerNoteLabel');
    const mt = _markerTypesCache.find(t => String(t.id) === String(markerTypeId));
    if (label) label.textContent = mt?.name ? `„${mt.name}”` : 'Marker';
    window.__pendingMarkerTypeId = markerTypeId;
    if (modal) modal.classList.remove('hidden');
    if (input) {
      input.value = '';
      setTimeout(() => input.focus(), 50);
    }
  };
})();

(function bindFreeTextMarkerOnce() {
  const btn = document.querySelector('#addFreeTextMarker');
  const input = document.querySelector('#freeTextInput');
  if (!btn || btn.dataset.bound === '1') return;
  btn.dataset.bound = '1';

  btn.addEventListener('click', async () => {
    const t = (input?.value || '').trim();
    if (!t) {
      showToast('Bitte Text eingeben', false);
      return;
    }
    try {
      await createFreeTextMarker(t);
      input.value = '';
      await refreshMarkersUI();
      showToast('Marker gesetzt ✅', true);
    } catch (e) {
      if (e.status === 409) {
        openNoSessionMarkerModal();
      } else {
        showToast('Marker setzen fehlgeschlagen ❌', false);
      }
    }
  });
})();

// ======== MARKER TYPES: API ======================================

let _markerTypesCache = [];
let _amlsTypeIdCache = null;
let _papierTypeIdCache = null;
let _pendingCommitPayload = null;
let _simStatusCache = null;
let _viewerSelections = []; // {id, name, selected, mode, role, online}

function setPendingCommit(payload) {
  _pendingCommitPayload = payload;
  if (payload) setLastStartPayload(payload);
  const wrap = document.querySelector('#commitBtnWrapper');
  const btn = document.querySelector('#commitBtn');
  const show = !!payload;
  if (wrap) {
    wrap.classList.toggle('hidden', !show);
    wrap.classList.toggle('pointer-events-none', !show);
  }
  if (btn) btn.classList.toggle('hidden', !show);
}

function clearPendingCommit() {
  setPendingCommit(null);
}

async function startPrestartFlow(payload) {
  beginSimLoading('prestart', 'Vor-Start wird vorbereitet…');
  try {
    const instructor_ids = Array.isArray(payload.viewer_map)
      ? payload.viewer_map
          .filter((v) => {
            const role = (v.role || viewerRoleFromId(Number(v.id))).toLowerCase();
            return role === 'instructor';
          })
          .map((v) => Number(v.id))
          .filter((id) => Number.isFinite(id) && id > 0)
      : [];
    await api.prestart(payload.name, instructor_ids, payload.viewer_map, payload.room_id);
    setPendingCommit(payload);
    toast('Vor-Start ausgelöst ✅', true);
    endSimLoading('prestart');
    return true;
  } catch (e) {
    console.error('Prestart failed', e);
    if (e.status === 404) {
      // Fallback: Prestart-API nicht vorhanden → direkt starten
      endSimLoading('prestart');
      const ok = await startWithRetry(payload.name, payload.viewer_ids, payload.viewer_map, payload.room_id);
      return ok;
    }
    toast(e.payload?.message || 'Pre-Start fehlgeschlagen ❌', false);
    clearPendingCommit();
    endSimLoading('prestart');
    return false;
  }
}

const commitBtn = document.querySelector('#commitBtn');
if (commitBtn && !commitBtn.dataset.boundCommit) {
  commitBtn.dataset.boundCommit = '1';
  commitBtn.addEventListener('click', async () => {
    if (!_pendingCommitPayload) {
      toast('Kein Vor-Start aktiv', false);
      return;
    }
    beginSimLoading('start', 'Session wird gestartet…');
    commitBtn.disabled = true;
    try {
      await api.commit(
        _pendingCommitPayload.viewer_ids,
        _pendingCommitPayload.viewer_map,
        _pendingCommitPayload.room_id,
        _pendingCommitPayload.name
      );
      setLastStartPayload(_pendingCommitPayload);
      toast('Session gestartet ✅', true);
      try { await startMarkerPoll(); } catch {}
      clearPendingCommit();
    } catch (e) {
      toast(e.payload?.message || 'Start fehlgeschlagen ❌', false);
      endSimLoading('start');
    }
    commitBtn.disabled = false;
  });
}

function openColorPickerDialog(initial = '#3b82f6') {
  return new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.className = 'fixed inset-0 bg-black bg-opacity-60 z-50 flex items-center justify-center';

    const box = document.createElement('div');
    box.className = 'bg-gray-900 p-6 rounded-xl shadow-xl w-full max-w-sm space-y-4';
    box.innerHTML = `
      <h3 class="text-lg font-semibold text-white">Farbe wählen</h3>
      <input type="color" id="pickerColorInput" class="w-full h-12 rounded" value="${initial || '#3b82f6'}" />
      <div class="flex justify-end gap-3">
        <button id="pickerCancelBtn" class="btn btn-sm">Abbrechen</button>
        <button id="pickerOkBtn" class="btn btn-sm btn-primary">OK</button>
      </div>
    `;

    overlay.appendChild(box);
    document.body.appendChild(overlay);
    const cleanup = () => overlay.remove();

    box.querySelector('#pickerOkBtn')?.addEventListener('click', () => {
      const val = box.querySelector('#pickerColorInput')?.value || initial || '#3b82f6';
      cleanup();
      resolve(val);
    });
    box.querySelector('#pickerCancelBtn')?.addEventListener('click', () => {
      cleanup();
      resolve(null);
    });
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) {
        cleanup();
        resolve(null);
      }
    });
  });
}

async function listMarkerTypes() {
  try {
    const r = await _fetchJson('/api/marker_types', { method: 'GET' });
    return Array.isArray(r?.marker_types) ? r.marker_types : [];
  } catch (e) {
    console.error('marker_types_list failed', e);
    return [];
  }
}

async function createMarkerType(body) {
  return await _fetchJson('/api/marker_types', {
    method: 'POST',
    body,
  });
}

async function ensureFreitextTypeId() {
  // Suche nach Typ "Freitext" (case-insensitive). Falls nicht vorhanden, versuchen anzulegen.
  if (_markerTypesCache && _markerTypesCache.length) {
    const existing = _markerTypesCache.find(t => (t.name || '').toLowerCase() === 'freitext');
    if (existing) return existing.id;
  }
  try {
    const created = await createMarkerType({ name: 'Freitext', color: '#3b82f6' });
    if (created) {
      _markerTypesCache.push(created);
      renderMarkerTypesList(false);
      return created.id;
    }
  } catch (e) {
    console.warn('Freitext-Typ konnte nicht angelegt werden, fallback auf ersten Typ', e);
  }
  // Fallback: erster vorhandener Typ oder 1
  if (_markerTypesCache && _markerTypesCache.length) {
    return _markerTypesCache[0].id;
  }
  return 1;
}

async function ensureAmlsTypeId() {
  if (_amlsTypeIdCache) return _amlsTypeIdCache;
  if (!_markerTypesCache || !_markerTypesCache.length) {
    _markerTypesCache = await listMarkerTypes();
  }
  if (_markerTypesCache && _markerTypesCache.length) {
    const existing = _markerTypesCache.find(t => (t.name || '').toLowerCase() === 'amls');
    if (existing) {
      _amlsTypeIdCache = existing.id;
      return existing.id;
    }
  }
  try {
    const created = await createMarkerType({ name: 'AMLS', color: '#f59e0b' });
    if (created) {
      _markerTypesCache.push(created);
      renderMarkerTypesList(false);
      _amlsTypeIdCache = created.id;
      return created.id;
    }
  } catch (e) {
    console.warn('AMLS-Typ konnte nicht angelegt werden, fallback auf Freitext', e);
  }
  return await ensureFreitextTypeId();
}

async function ensurePapierTypeId() {
  if (_papierTypeIdCache) return _papierTypeIdCache;
  if (!_markerTypesCache || !_markerTypesCache.length) {
    _markerTypesCache = await listMarkerTypes();
  }
  if (_markerTypesCache && _markerTypesCache.length) {
    const existing = _markerTypesCache.find(t => (t.name || '').toLowerCase() === 'papier');
    if (existing) {
      _papierTypeIdCache = existing.id;
      return existing.id;
    }
  }
  try {
    const created = await createMarkerType({ name: 'Papier', color: '#f59e0b' });
    if (created) {
      _markerTypesCache.push(created);
      renderMarkerTypesList(false);
      _papierTypeIdCache = created.id;
      return created.id;
    }
  } catch (e) {
    console.warn('Papier-Typ konnte nicht angelegt werden, fallback auf Freitext', e);
  }
  return await ensureFreitextTypeId();
}

async function updateMarkerType(id, body) {
  return await _fetchJson(`/api/marker_types/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body,
  });
}

async function deleteMarkerType(id) {
  return await _fetchJson(`/api/marker_types/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

// ======== MARKER TYPES: UI =======================================

function openMarkerTypesModal() {
  const m = document.querySelector('#markerTypesModal');
  if (!m) return;
  m.classList.remove('hidden');
  document.documentElement.classList.add('overflow-hidden');
  renderMarkerTypesList(true);
}

function closeMarkerTypesModal() {
  const m = document.querySelector('#markerTypesModal');
  if (!m) return;
  m.classList.add('hidden');
  document.documentElement.classList.remove('overflow-hidden');
}

function markerTypeRowHtml(t, index, total) {
  const scopeLabel = t.user_id ? 'Persönlich' : 'Standard';
  const scopeClass = t.user_id ? 'border-emerald-500/40 text-emerald-200 bg-emerald-900/30' : 'border-gray-600/40 text-gray-200 bg-gray-800/40';
  const sortInfo = Number.isFinite(t.sort_index) ? `#${t.sort_index}` : `#${index + 1}`;
  return `
    <div class="p-4 flex items-center gap-3">
      <div class="w-6 h-6 rounded border border-gray-700"
           style="background:${t.color || '#4b5563'}"></div>
      <div class="flex-1">
        <div class="text-white font-medium">${escapeHtml(t.name)}</div>
        <div class="text-xs text-gray-400">ID: ${t.id} • ${escapeHtml(t.color)} • ${sortInfo}</div>
        <div class="mt-1 inline-flex items-center gap-2 text-[10px] px-2 py-0.5 rounded-full border ${scopeClass}">
          ${scopeLabel}
        </div>
        <div class="mt-2 flex items-center gap-2 text-xs flex-wrap">
          <span class="text-gray-400">Taste:</span>
          ${t.hotkey
            ? `<kbd class="kbd kbd-sm">${escapeHtml(hotkeyLabel(t.hotkey))}</kbd>`
            : `<span class="text-gray-500">–</span>`}
          <button data-action="assign-hotkey-marker-type" data-id="${t.id}"
                  class="px-2 py-0.5 text-[11px] rounded bg-gray-800 hover:bg-gray-700 text-white">
            ⌨️ Taste zuweisen
          </button>
          ${t.hotkey
            ? `<button data-action="clear-hotkey-marker-type" data-id="${t.id}"
                  class="px-2 py-0.5 text-[11px] rounded bg-gray-800 hover:bg-gray-700 text-white" title="Taste entfernen">✕</button>`
            : ''}
          <span data-hotkey-status="${t.id}" class="text-amber-300"></span>
        </div>
      </div>
      <div class="flex items-center gap-2">
        <div class="flex flex-col gap-1">
          <button data-action="sort-up-marker-type" data-id="${t.id}" data-index="${index}"
                  class="px-2 py-1 text-[11px] rounded bg-gray-800 hover:bg-gray-700 text-white"
                  ${index === 0 ? 'disabled' : ''}>
            ▲
          </button>
          <button data-action="sort-down-marker-type" data-id="${t.id}" data-index="${index}"
                  class="px-2 py-1 text-[11px] rounded bg-gray-800 hover:bg-gray-700 text-white"
                  ${index === total - 1 ? 'disabled' : ''}>
            ▼
          </button>
        </div>
        <button data-action="edit-marker-type" data-id="${t.id}"
                class="px-2 py-1 text-xs rounded bg-gray-700 hover:bg-gray-600 text-white">
          Bearbeiten
        </button>
        <button data-action="delete-marker-type" data-id="${t.id}"
                class="px-2 py-1 text-xs rounded bg-red-600 hover:bg-red-500 text-white">
          Löschen
        </button>
      </div>
    </div>
  `;
}

async function renderMarkerTypesList(force = false) {
  const list = document.querySelector('#markerTypesList');
  const stats = document.querySelector('#markerTypesStats');
  if (!list) return;

  if (force || !_markerTypesCache.length) {
    list.innerHTML = `<div class="p-4 text-gray-400">Lade Marker-Typen…</div>`;
    _markerTypesCache = await listMarkerTypes();
  }

renderMarkerTypeButtonsFromCache();

  if (!_markerTypesCache.length) {
    list.innerHTML = `<div class="p-4 text-gray-400">Keine Marker-Typen definiert.</div>`;
  } else {
    list.innerHTML = _markerTypesCache.map((t, idx) => markerTypeRowHtml(t, idx, _markerTypesCache.length)).join('');
  }

  list.querySelectorAll('[data-action="sort-up-marker-type"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const index = Number(btn.dataset.index);
      if (!Number.isFinite(index) || index <= 0) return;
      const current = _markerTypesCache[index];
      const target = _markerTypesCache[index - 1];
      if (!current || !target) return;
      const sameGroup = (current.user_id ?? null) === (target.user_id ?? null);
      if (!sameGroup) return;
      _markerTypesCache[index - 1] = current;
      _markerTypesCache[index] = target;
      await persistMarkerTypeSort(current.user_id != null ? 'user' : 'default');
      renderMarkerTypesList(false);
    });
  });
  list.querySelectorAll('[data-action="sort-down-marker-type"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const index = Number(btn.dataset.index);
      if (!Number.isFinite(index) || index >= _markerTypesCache.length - 1) return;
      const current = _markerTypesCache[index];
      const target = _markerTypesCache[index + 1];
      if (!current || !target) return;
      const sameGroup = (current.user_id ?? null) === (target.user_id ?? null);
      if (!sameGroup) return;
      _markerTypesCache[index + 1] = current;
      _markerTypesCache[index] = target;
      await persistMarkerTypeSort(current.user_id != null ? 'user' : 'default');
      renderMarkerTypesList(false);
    });
  });

  list.querySelectorAll('[data-action="edit-marker-type"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.id;
      const t = _markerTypesCache.find(x => String(x.id) === String(id));
      if (!t) return;
      const name = prompt('Name des Marker-Typs:', t.name || '');
      if (name == null) return;
      const color = await openColorPickerDialog(t.color || '#3b82f6');
      if (!color) return;
      try {
        await updateMarkerType(id, { name, color });
        t.name = name;
        t.color = color;
        renderMarkerTypesList(false);
        showToast('Marker-Typ aktualisiert ✅', true);
      } catch {
        showToast('Marker-Typ aktualisieren fehlgeschlagen ❌', false);
      }
    });
  });

  list.querySelectorAll('[data-action="assign-hotkey-marker-type"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.id;
      const t = _markerTypesCache.find(x => String(x.id) === String(id));
      if (!t) return;
      const status = list.querySelector(`[data-hotkey-status="${id}"]`);
      if (status) status.textContent = 'Jetzt Taste drücken… (Esc = Abbruch, Entf = entfernen)';
      btn.disabled = true;
      const hk = await captureHotkey(t.id);
      btn.disabled = false;
      if (status) status.textContent = '';
      if (hk === null) return;
      await saveMarkerTypeHotkey(t, hk);
    });
  });

  list.querySelectorAll('[data-action="clear-hotkey-marker-type"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const t = _markerTypesCache.find(x => String(x.id) === String(btn.dataset.id));
      if (!t) return;
      await saveMarkerTypeHotkey(t, '');
    });
  });

  list.querySelectorAll('[data-action="delete-marker-type"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.id;
      if (!confirm('Marker-Typ wirklich löschen? Marker, die diesen Typ nutzen, könnten ungültig werden.')) return;
      try {
        await deleteMarkerType(id);
        _markerTypesCache = _markerTypesCache.filter(t => String(t.id) !== String(id));
        renderMarkerTypesList(false);
        showToast('Marker-Typ gelöscht ✅', true);
      } catch {
        showToast('Marker-Typ löschen fehlgeschlagen ❌', false);
      }
    });
  });

  if (stats) {
    stats.textContent = `${_markerTypesCache.length} Typen`;
  }
}

async function persistMarkerTypeSort(groupKey) {
  if (!_markerTypesCache || !_markerTypesCache.length) return;
  const groups = {
    user: _markerTypesCache.filter(t => t.user_id != null),
    default: _markerTypesCache.filter(t => t.user_id == null),
  };
  const group = groups[groupKey] || [];
  if (!group.length) return;
  const updates = [];
  group.forEach((t, idx) => {
      const nextIndex = idx + 1;
      if (t.sort_index !== nextIndex) {
        updates.push({ id: t.id, sort_index: nextIndex });
        t.sort_index = nextIndex;
      }
  });
  if (!updates.length) return;
  let failed = false;
  for (const u of updates) {
    try {
      await updateMarkerType(u.id, { sort_index: u.sort_index });
    } catch (e) {
      failed = true;
    }
  }
  if (failed) {
    showToast('Sortierung nicht vollständig gespeichert', false);
  }
}

// Bindings

(function bindMarkerTypesUIOnce() {
  const btn = document.querySelector('#markerTypesBtn');
  const refresh = document.querySelector('#markerTypesRefreshBtn');
  const close = document.querySelector('#closeMarkerTypesModal');
  const addBtn = document.querySelector('#addMarkerTypeBtn');
  const modal = document.querySelector('#markerTypesModal');

  if (btn && !btn.dataset.boundMarkerTypesOpen) {
    btn.dataset.boundMarkerTypesOpen = '1';
    btn.addEventListener('click', (e) => { e.preventDefault(); openMarkerTypesModal(); });
  }
  const refreshBtn = document.querySelector('#markerTypesRefreshBtn');
  if (refreshBtn && !refreshBtn.dataset.boundMarkerTypesRefresh) {
    refreshBtn.dataset.boundMarkerTypesRefresh = '1';
    refreshBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      await renderMarkerTypesList(true);
      showToast('Marker-Typen aktualisiert ✅', true);
    });
  }
  if (close && !close.dataset.boundMarkerTypesClose) {
    close.dataset.boundMarkerTypesClose = '1';
    close.addEventListener('click', (e) => { e.preventDefault(); closeMarkerTypesModal(); });
  }
  if (refresh && !refresh.dataset.boundMarkerTypesRefresh) {
    refresh.dataset.boundMarkerTypesRefresh = '1';
    refresh.addEventListener('click', async (e) => {
      e.preventDefault();
      await renderMarkerTypesList(true);
      showToast('Marker-Typen aktualisiert ✅', true);
    });
  }
  if (addBtn && !addBtn.dataset.boundMarkerTypesAdd) {
    addBtn.dataset.boundMarkerTypesAdd = '1';
    addBtn.addEventListener('click', async () => {
      const name = prompt('Name des neuen Marker-Typs:', 'Neuer Typ');
      if (!name) return;
      const color = await openColorPickerDialog('#3b82f6') || '#3b82f6';
      try {
        const created = await createMarkerType({ name, color });
        _markerTypesCache.push(created);
        renderMarkerTypesList(false);
        showToast('Marker-Typ angelegt ✅', true);
      } catch {
        showToast('Marker-Typ anlegen fehlgeschlagen ❌', false);
      }
    });
  }

  const autoBtn = document.querySelector('#autoAssignHotkeysBtn');
  if (autoBtn && !autoBtn.dataset.boundAutoHotkeys) {
    autoBtn.dataset.boundAutoHotkeys = '1';
    autoBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      autoBtn.disabled = true;
      try { await autoAssignDigitHotkeys(); } finally { autoBtn.disabled = false; }
    });
  }

  if (modal && !modal.dataset.boundMarkerTypesBackdrop) {
    modal.dataset.boundMarkerTypesBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) closeMarkerTypesModal();
    });
  }
})();

window.addEventListener('keydown', (ev) => {
  const m = document.querySelector('#markerTypesModal');
  if (!m || m.classList.contains('hidden')) return;
  if (ev.key === 'Escape') closeMarkerTypesModal();
});

// ===== Marker-Notizen Verwaltung (Textbausteine) =====
(function initMarkerNotes() {
  const listEl = document.getElementById('markerNoteList');
  const addBtn = document.getElementById('markerNoteAdd');
  const input = document.getElementById('markerNoteNew');

  function renderList() {
    if (!listEl) return;
    listEl.innerHTML = '';
    if (!_markerNoteCache || _markerNoteCache.length === 0) {
      listEl.innerHTML = '<span class="text-gray-400 text-sm">Keine Bausteine vorhanden.</span>';
      return;
    }
    _markerNoteCache.forEach((txt, idx) => {
      const chip = document.createElement('div');
      chip.className = 'inline-flex items-center gap-2 px-3 py-1 rounded-full bg-gray-800 border border-gray-700 text-sm text-gray-100';
      const span = document.createElement('span');
      span.textContent = txt;
      const up = document.createElement('button');
      up.type = 'button';
      up.className = 'text-xs text-gray-300 hover:text-white';
      up.textContent = '▲';
      up.disabled = idx === 0;
      up.addEventListener('click', async (e) => {
        e.stopPropagation();
        if (idx <= 0) return;
        const tmp = _markerNoteCache[idx - 1];
        _markerNoteCache[idx - 1] = _markerNoteCache[idx];
        _markerNoteCache[idx] = tmp;
        await saveNotes();
      });
      const down = document.createElement('button');
      down.type = 'button';
      down.className = 'text-xs text-gray-300 hover:text-white';
      down.textContent = '▼';
      down.disabled = idx === _markerNoteCache.length - 1;
      down.addEventListener('click', async (e) => {
        e.stopPropagation();
        if (idx >= _markerNoteCache.length - 1) return;
        const tmp = _markerNoteCache[idx + 1];
        _markerNoteCache[idx + 1] = _markerNoteCache[idx];
        _markerNoteCache[idx] = tmp;
        await saveNotes();
      });
      const del = document.createElement('button');
      del.className = 'text-red-400 hover:text-red-300 text-xs';
      del.textContent = '✕';
      del.addEventListener('click', async () => {
        _markerNoteCache.splice(idx, 1);
        await saveNotes();
      });
      chip.appendChild(span);
      chip.appendChild(up);
      chip.appendChild(down);
      chip.appendChild(del);
      listEl.appendChild(chip);
    });
    // Auch Modal-Suggestions aktualisieren
    const suggestions = document.getElementById('markerNoteSuggestions');
    if (suggestions) {
      suggestions.innerHTML = _markerNoteCache.map(t =>
        `<button type="button" class="btn btn-xs btn-ghost border border-gray-700" data-suggestion="${escapeHtml(t)}">${escapeHtml(t)}</button>`
      ).join('');
      suggestions.querySelectorAll('[data-suggestion]').forEach(btn => {
        btn.addEventListener('click', () => {
          const inputModal = document.getElementById('markerNoteInput');
          if (inputModal) {
            inputModal.value = btn.dataset.suggestion || '';
            inputModal.focus();
          }
        });
      });
    }
  }

  async function saveNotes() {
    try {
      await _fetchJson('/api/marker_notes', { method: 'POST', body: { notes: _markerNoteCache } });
      renderList();
      showToast('Textbausteine gespeichert', true);
    } catch (e) {
      console.error('Marker-Notizen speichern fehlgeschlagen', e);
      showToast('Konnte Textbausteine nicht speichern', false);
    }
  }

  if (addBtn) {
    addBtn.addEventListener('click', async () => {
      const val = (input?.value || '').trim();
      if (!val) return;
      _markerNoteCache.push(val);
      if (input) input.value = '';
      await saveNotes();
    });
  }

  async function loadNotes() {
    try {
      const res = await _fetchJson('/api/marker_notes', { method: 'GET' });
      _markerNoteCache = Array.isArray(res.notes) ? res.notes : [];
    } catch (e) {
      console.warn('Konnte Marker-Notizen nicht laden, nutze Standard', e);
      if (!_markerNoteCache || _markerNoteCache.length === 0) {
        _markerNoteCache = ['Patient stabil', 'Transport bereit', 'Team unterwegs', 'Rettungsweg frei'];
      }
    }
    renderList();
  }

  loadNotes();
})();

// ======== UI-Broadcast (central WS via base.js) ===================
(function initUiBroadcast() {
  if (!window.orsUiWs || typeof window.orsUiWs.on !== 'function') {
    console.warn('UI WS not available (base.js)');
    return;
  }
  window.orsUiWs.on((msg) => {
    if (msg.type === 'pre_start') {
      toast(`Vor-Start: ${msg.name || 'Session'} – jetzt zu Simulate wechseln?`, true);
      setSimStatusCache({ active: false, session_id: msg.session_id, name: msg.name, pending: true });
      endSimLoading('prestart');
      return;
    }
    if (msg.type === 'sim_started') {
      toast(`Session gestartet: ${msg.name || msg.session_id}`, true);
      setSimStatusCache({
        active: true,
        session_id: msg.session_id,
        name: msg.name,
        start_time: msg.start_time,
      });
      clearPendingCommit();
      clearMarkerPoll();
      startMarkerPoll().catch(() => {});
      updateAudioVisibility();
      endSimLoading('start');
      return;
    }
    if (msg.type === 'pre_start_cancelled') {
      clearPendingCommit();
      setSimStatusCache({ active: false, session_id: null, name: null });
      endSimLoading('prestart');
      return;
    }
    if (msg.type === 'sim_start_wait') {
      showSimLoading(true, msg.message || 'Pipeline wird vorbereitet…');
      return;
    }
    if (msg.type === 'sim_start_ready') {
      showSimLoading(true, msg.message || 'Pipeline bereit – Viewer verbinden…');
      return;
    }
    if (msg.type === 'sim_stopped') {
      toast('Session beendet', true);
      setSimStatusCache({
        active: false,
        session_id: msg.session_id || null,
        name: null,
      });
      clearPendingCommit();
      clearMarkerPoll();
      refreshMarkersUI().catch(() => {});
      updateAudioVisibility();
      endSimLoading('stop');
      return;
    }
    if (msg.type === 'room_switched') {
      const warnings = Array.isArray(msg.warnings) ? msg.warnings : [];
      if (!_roomSwitchBusy) {
        // Wechsel kam von einer anderen UI-Instanz
        toast(
          warnings.length
            ? `Raum gewechselt: ${msg.room_name} (mit Hinweisen)`
            : `Raum gewechselt: ${msg.room_name}`,
          warnings.length === 0
        );
      }
      const last = getLastStartPayload();
      if (last && msg.room_id != null) setLastStartPayload({ ...last, room_id: Number(msg.room_id) });
      const sel = $('#liveRoomSelect');
      if (sel && msg.room_id != null) sel.value = String(msg.room_id);
      setSimStatusCache(null);
      refreshMarkersUI().catch(() => {});
      return;
    }
    if (msg.type === 'marker_add' || msg.type === 'marker_delete' || msg.type === 'marker_update') {
      refreshMarkersUI().catch(() => {});
    }
  });
})();

// Marker mit bestimmtem Typ (optionalem Text)
async function createTypedMarker(markerTypeId, text) {
  const body = { marker_type_id: markerTypeId };
  if (text) body.text = text;
  return await _fetchJson('/api/markers', {
    method: 'POST',
    body,
  });
}

// ======== INIT (DOMContentLoaded) ================================
onDomReady(async () => {
  bindMarkerModeToggleOnce();
  bindAmlsControlsOnce();
  refreshMarkersUI().catch(() => {});
  try {
    // Marker-Typen initial laden
    await renderMarkerTypesList(true);
  } catch (e) {
    console.warn('renderMarkerTypesList failed', e);
  }
  // Audio-Section Sichtbarkeit je nach Session
  updateAudioVisibility();
});

// Buttons über dem Freitext-Feld
function renderMarkerTypeButtonsFromCache() {
  const container = document.querySelector('#markerControls');
  const statusLbl = document.querySelector('#markerStatusLabel');
  if (!container) return;

  if (!_markerTypesCache || !_markerTypesCache.length) {
    container.innerHTML = `<span class="text-gray-400 text-sm">Keine Marker-Typen definiert.</span>`;
    if (statusLbl) statusLbl.textContent = 'ℹ️ Keine Marker-Typen definiert';
    return;
  }

  const visibleTypes = _markerTypesCache.filter(t => !['amls', 'papier'].includes((t.name || '').toLowerCase()));
  if (!visibleTypes.length) {
    container.innerHTML = `<span class="text-gray-400 text-sm">Keine Marker-Typen definiert.</span>`;
    if (statusLbl) statusLbl.textContent = 'ℹ️ Keine Marker-Typen definiert';
    return;
  }

  container.innerHTML = visibleTypes.map(t => `
    <button
      class="btn btn-sm font-bold border-0 shadow marker-type-btn"
      style="background:${t.color || '#4b5563'};color:#fff"
      data-marker-type-id="${t.id}"
      title="${t.hotkey ? `Taste: ${escapeHtml(hotkeyLabel(t.hotkey))}` : 'Keine Taste zugewiesen'}"
    >
      ${escapeHtml(t.name)}
      ${t.hotkey ? `<kbd class="kbd kbd-xs ml-1 text-neutral">${escapeHtml(hotkeyLabel(t.hotkey))}</kbd>` : ''}
    </button>
  `).join('');

  // Papier-Button: Kürzel anzeigen, falls zugewiesen
  const papier = _markerTypesCache.find(t => (t.name || '').toLowerCase() === 'papier');
  const papierKbd = document.getElementById('papierHotkeyKbd');
  if (papierKbd) {
    if (papier?.hotkey) {
      papierKbd.textContent = hotkeyLabel(papier.hotkey);
      papierKbd.classList.remove('hidden');
    } else {
      papierKbd.classList.add('hidden');
    }
  }

  if (statusLbl) {
    statusLbl.textContent = `📍 ${visibleTypes.length} Marker-Typ(en) verfügbar`;
  }

  container.querySelectorAll('[data-marker-type-id]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const typeId = parseInt(btn.dataset.markerTypeId, 10);
      if (!Number.isFinite(typeId)) return;
      if (typeof window.openMarkerNoteModal === 'function') {
        window.openMarkerNoteModal(typeId);
      } else {
        try {
          await createTypedMarker(typeId);
          await refreshMarkersUI();
          showToast('Marker gesetzt ✅', true);
        } catch (e) {
          if (e.status === 409) {
            openNoSessionMarkerModal();
          } else {
            console.error('createTypedMarker failed', e);
            showToast('Marker setzen fehlgeschlagen ❌', false);
          }
        }
      }
    });
  });
}
