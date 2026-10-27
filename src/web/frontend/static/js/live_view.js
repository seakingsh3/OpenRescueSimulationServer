// live_view.js – Ausbilder-Liveansicht (/ui/viewer).
// Verbindet sich per WebRTC direkt mit der laufenden Pipeline (/ws/webrtc), ohne als
// ferngesteuerter Viewer registriert zu werden. Schaltet bei Vor-Start/Start/Stopp
// automatisch um (UI-WebSocket-Ereignisse + Status-Abfrage als Rückfallebene).
import { authJsonFetch } from '/static/js/http.js';

const $ = (id) => document.getElementById(id);
const video = $('lvVideo');
const stage = $('lvStage');
const overlay = $('lvOverlay');
const overlayIcon = $('lvOverlayIcon');
const overlayText = $('lvOverlayText');
const stateBadge = $('lvState');
const muteBtn = $('lvMuteBtn');
const fullscreenBtn = $('lvFullscreenBtn');
const reconnectBtn = $('lvReconnectBtn');
const sessionInfo = $('lvSessionInfo');
const distributorInfo = $('lvDistributorInfo');

const STATUS_POLL_MS = 4000;
const DISTRIBUTOR_POLL_MS = 10000;

const lv = {
  pc: null,
  ws: null,
  wanted: false,       // soll gerade ein Bild gezeigt werden?
  prestart: false,     // laufender Vor-Start (aus /api/sim/status bzw. UI-Ereignis)
  sessionId: null,
  retryTimer: null,
  retries: 0,
};

function escapeHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function setBadge(kind, text) {
  stateBadge.className = 'badge ' + ({ live: 'badge-success', prestart: 'badge-warning', connecting: 'badge-info', idle: 'badge-ghost', error: 'badge-error' }[kind] || 'badge-ghost');
  stateBadge.textContent = text;
}

function showOverlay(icon, text) {
  overlayIcon.textContent = icon;
  overlayText.textContent = text;
  overlay.classList.remove('hidden');
}

function hideOverlay() {
  overlay.classList.add('hidden');
}

function updateMuteBtn() {
  muteBtn.textContent = video.muted ? '🔇 Ton aus' : '🔊 Ton an';
}

// ---------------------------------------------------------------------------
// WebRTC
// ---------------------------------------------------------------------------
function teardown() {
  if (lv.retryTimer) { clearTimeout(lv.retryTimer); lv.retryTimer = null; }
  if (lv.ws) { lv.ws.onclose = null; lv.ws.onerror = null; try { lv.ws.close(); } catch {} lv.ws = null; }
  if (lv.pc) { try { lv.pc.close(); } catch {} lv.pc = null; }
  video.srcObject = null;
  video.classList.add('hidden');
  muteBtn.disabled = true;
  fullscreenBtn.disabled = true;
}

function scheduleReconnect() {
  if (!lv.wanted || lv.retryTimer) return;
  lv.retries += 1;
  const wait = Math.min(5000, 600 * lv.retries);
  lv.retryTimer = setTimeout(() => { lv.retryTimer = null; if (lv.wanted) connect(); }, wait);
}

function connect() {
  teardown();
  lv.wanted = true;
  setBadge('connecting', 'Verbinde…');
  showOverlay('⏳', 'Verbinde mit dem Live-Bild…');

  const pc = new RTCPeerConnection({ iceServers: [] }); // LAN: nur Host-Kandidaten
  lv.pc = pc;
  const stream = new MediaStream();

  pc.ontrack = (ev) => {
    if (!stream.getTracks().includes(ev.track)) stream.addTrack(ev.track);
    if (video.srcObject !== stream) video.srcObject = stream;
    video.classList.remove('hidden');
    hideOverlay();
    muteBtn.disabled = false;
    fullscreenBtn.disabled = false;
    updateMuteBtn();
    video.play().catch(() => showOverlay('▶️', 'Zum Abspielen klicken'));
  };

  pc.onconnectionstatechange = () => {
    if (pc !== lv.pc) return;
    if (pc.connectionState === 'connected') {
      lv.retries = 0;
      setBadge(lv.prestart ? 'prestart' : 'live', lv.prestart ? 'Vor-Start' : 'Live');
    } else if (['failed', 'disconnected', 'closed'].includes(pc.connectionState)) {
      scheduleReconnect();
    }
  };

  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  // Live-Bild nur mit Login: Token als Parameter (WebSockets können keinen Header setzen)
  const token = localStorage.getItem('access_token') || '';
  const ws = new WebSocket(`${proto}//${location.host}/ws/webrtc?token=${encodeURIComponent(token)}`);
  lv.ws = ws;

  pc.onicecandidate = (ev) => {
    if (ev.candidate && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: 'ice', candidate: ev.candidate.candidate, sdp_mline_index: ev.candidate.sdpMLineIndex }));
    }
  };

  ws.onmessage = async (ev) => {
    let msg;
    try { msg = JSON.parse(ev.data); } catch { return; }
    try {
      if (msg.type === 'offer' && msg.sdp) {
        await pc.setRemoteDescription({ type: 'offer', sdp: msg.sdp });
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        ws.send(JSON.stringify({ type: 'answer', sdp: answer.sdp }));
      } else if (msg.type === 'ice' && msg.candidate) {
        await pc.addIceCandidate({ candidate: msg.candidate, sdpMLineIndex: msg.sdp_mline_index ?? 0 });
      } else if (msg.type === 'bye') {
        // Pipeline wird umgebaut oder gestoppt – Status entscheidet, ob neu verbunden wird
        setTimeout(refreshStatus, 800);
      }
    } catch (e) {
      console.warn('[live] Signaling-Fehler', e);
    }
  };
  ws.onclose = () => { if (ws === lv.ws) scheduleReconnect(); };
  ws.onerror = () => { try { ws.close(); } catch {} };
}

function stopShowing(icon, text) {
  lv.wanted = false;
  lv.retries = 0;
  teardown();
  showOverlay(icon, text);
}

// ---------------------------------------------------------------------------
// Session-Status
// ---------------------------------------------------------------------------
async function refreshStatus() {
  let st;
  try {
    const r = await fetch('/api/sim/status', { cache: 'no-store' });
    st = await r.json();
  } catch {
    setBadge('error', 'Server nicht erreichbar');
    return;
  }

  // Der Server meldet einen laufenden Vor-Start selbst (auch nach Neuladen der Seite)
  lv.prestart = !st.active && !!st.prestart_session_id;

  if (st.active) {
    sessionInfo.textContent = `Session „${st.name || '–'}“` + (st.room_name ? ` · Raum ${st.room_name}` : '');
    if (!lv.wanted || lv.sessionId !== st.session_id) {
      lv.sessionId = st.session_id;
      connect();
    }
  } else if (lv.prestart) {
    sessionInfo.textContent = 'Vor-Start: Bild nur für Ausbilder, noch keine Aufnahme';
    if (!lv.wanted) connect();
  } else {
    lv.sessionId = null;
    sessionInfo.textContent = 'Keine Session aktiv';
    setBadge('idle', 'Keine Session');
    if (lv.wanted || !overlay.classList.contains('hidden')) {
      stopShowing('📡', 'Keine Session aktiv – das Bild erscheint automatisch, sobald eine Übung startet.');
    }
  }
}

function onUiEvent(msg) {
  switch (msg?.type) {
    case 'pre_start':
      lv.prestart = true;
      setTimeout(refreshStatus, 300);
      break;
    case 'sim_started':
    case 'sim_start_ready':
      lv.prestart = false;
      lv.sessionId = null; // erzwingt Neuverbindung mit der neu gebauten Pipeline
      setTimeout(refreshStatus, 600);
      break;
    case 'sim_stopped':
    case 'pre_start_cancelled':
      lv.prestart = false;
      setTimeout(refreshStatus, 300);
      break;
    default:
      break;
  }
}

function subscribeUiEvents(attempt = 0) {
  // base.js wird als Modul nach diesem Skript geladen – kurz warten, bis orsUiWs existiert
  if (window.orsUiWs) { window.orsUiWs.on(onUiEvent); return; }
  if (attempt < 50) setTimeout(() => subscribeUiEvents(attempt + 1), 200);
}

// ---------------------------------------------------------------------------
// Verteiler-Status (externer RTMP-Ausgang), nur zur Info
// ---------------------------------------------------------------------------
async function refreshDistributor() {
  try {
    const stats = await authJsonFetch('/api/host/stats', { method: 'GET' });
    const d = stats?.rtmp;
    if (!d || !d.enabled) { distributorInfo.textContent = ''; return; }
    const label = { streaming: 'sendet', connecting: 'verbindet', error: 'Fehler' }[d.state] || d.state;
    distributorInfo.textContent = `Externer Verteiler: ${label}` + (d.state === 'error' && d.last_error ? ` (${d.last_error})` : '');
    distributorInfo.className = d.state === 'error' ? 'text-red-400' : '';
  } catch {
    distributorInfo.textContent = '';
  }
}

// ---------------------------------------------------------------------------
// Viewer-Bildschirme
// ---------------------------------------------------------------------------
const VIEWER_PATH = /^\/viewer\/(\d+)$/;

async function loadViewers() {
  const tbody = $('lvViewersBody');
  let rows;
  try {
    const res = await authJsonFetch('/api/viewers', { method: 'GET' });
    rows = Array.isArray(res?.viewers) ? res.viewers : [];
  } catch (e) {
    tbody.innerHTML = `<tr><td colspan="4" class="text-gray-500">${e?.status === 401 ? 'Bitte einloggen, um die Viewer-Liste zu sehen.' : 'Viewer-Liste nicht erreichbar.'}</td></tr>`;
    return [];
  }
  const screens = rows.filter((r) => VIEWER_PATH.test(r.url || ''))
    .sort((a, b) => Number(VIEWER_PATH.exec(a.url)[1]) - Number(VIEWER_PATH.exec(b.url)[1]));
  if (!screens.length) {
    tbody.innerHTML = '<tr><td colspan="4" class="text-gray-500">Noch kein Viewer-Bildschirm geöffnet.</td></tr>';
    return screens;
  }
  tbody.innerHTML = screens.map((r) => `
    <tr>
      <td>${r.online ? '<span class="badge badge-success badge-sm">online</span>' : '<span class="badge badge-ghost badge-sm">offline</span>'}</td>
      <td class="font-semibold">${escapeHtml(r.alias || r.name)}</td>
      <td class="font-mono text-xs">${escapeHtml(r.url)}</td>
      <td class="text-right whitespace-nowrap">
        <button class="btn btn-xs btn-outline" data-copy="${escapeHtml(r.url)}">Link kopieren</button>
        <button class="btn btn-xs btn-primary" data-open="${escapeHtml(VIEWER_PATH.exec(r.url)[1])}" title="In neuem Tab öffnen und dieses Gerät gleich koppeln">Öffnen</button>
      </td>
    </tr>`).join('');
  tbody.querySelectorAll('button[data-open]').forEach((btn) => {
    btn.addEventListener('click', () => openPairedViewer(Number(btn.dataset.open)));
  });
  tbody.querySelectorAll('button[data-copy]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const link = location.origin + btn.dataset.copy;
      try { await navigator.clipboard.writeText(link); btn.textContent = 'Kopiert ✓'; }
      catch { window.prompt('Link kopieren:', link); }
      setTimeout(() => { btn.textContent = 'Link kopieren'; }, 1500);
    });
  });
  return screens;
}

// Viewer in neuem Tab öffnen und dabei gleich koppeln (Einmal-Code im Link). Das Fenster
// wird sofort im Klick geöffnet (sonst blockt der Browser es) und erst danach umgeleitet.
async function openPairedViewer(no) {
  const win = window.open('about:blank', '_blank');
  let link = `/viewer/${no}`;
  try {
    const res = await authJsonFetch('/api/pairing/start', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ viewer_no: no }) });
    if (res?.code) link += `?pair=${encodeURIComponent(res.code)}`;
  } catch { /* ohne Code: Gerät fragt nach der PIN */ }
  if (win) { win.opener = null; win.location.href = link; } else { window.open(link, '_blank'); }
  setTimeout(loadViewers, 2500);
}

async function openNewViewer() {
  const win = window.open('about:blank', '_blank');
  const screens = await loadViewers();
  const used = screens.map((r) => Number(VIEWER_PATH.exec(r.url)[1]));
  const next = used.length ? Math.max(...used) + 1 : 1;
  let link = `/viewer/${next}`;
  try {
    const res = await authJsonFetch('/api/pairing/start', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ viewer_no: next }) });
    if (res?.code) link += `?pair=${encodeURIComponent(res.code)}`;
  } catch { /* ohne Code: Gerät fragt nach der PIN */ }
  if (win) { win.opener = null; win.location.href = link; } else { window.open(link, '_blank'); }
  setTimeout(loadViewers, 2500);
}

// ---------------------------------------------------------------------------
muteBtn.addEventListener('click', () => {
  video.muted = !video.muted;
  video.play().catch(() => {});
  updateMuteBtn();
});
fullscreenBtn.addEventListener('click', () => {
  (stage.requestFullscreen ? stage.requestFullscreen() : Promise.reject()).catch(() => {});
});
overlay.addEventListener('click', () => {
  if (video.srcObject) { video.play().then(hideOverlay).catch(() => {}); }
});
reconnectBtn.addEventListener('click', () => { lv.sessionId = null; refreshStatus(); });
$('lvViewersRefresh').addEventListener('click', loadViewers);
$('lvNewViewerBtn').addEventListener('click', openNewViewer);

subscribeUiEvents();
refreshStatus();
refreshDistributor();
loadViewers();
setInterval(() => { if (document.visibilityState === 'visible') refreshStatus(); }, STATUS_POLL_MS);
setInterval(() => { if (document.visibilityState === 'visible') refreshDistributor(); }, DISTRIBUTOR_POLL_MS);
