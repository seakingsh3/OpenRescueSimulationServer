// camera_control.js — Pan/Tilt/Zoom-Steuerung für PTZ-fähige Kameras.
// Für jeden eingeloggten Nutzer gedacht (nicht nur Admin); Einrichten/Löschen
// der Kamera bleibt weiterhin ausschließlich im Admin-Kameras-Modal (options.js).
import { authJsonFetch, authFetch, getAuthToken } from '/static/js/http.js';

const $ = (sel) => document.querySelector(sel);

const loginNotice = $('#ccLoginNotice');
const listView = $('#ccListView');
const controlView = $('#ccControlView');
const camGrid = $('#ccCamGrid');
const emptyMsg = $('#ccEmpty');
const camNameEl = $('#ccCamName');
const statusEl = $('#ccStatus');
const backBtn = $('#ccBack');
const stopBtn = $('#ccStop');
const refreshBtn = $('#ccRefresh');
const zoomInBtn = $('#ccZoomIn');
const zoomOutBtn = $('#ccZoomOut');

let activeCameraId = null;
let moveTimer = null;

function setStatus(msg, isError) {
  if (!statusEl) return;
  statusEl.textContent = msg || '';
  statusEl.classList.toggle('text-red-400', !!isError);
  statusEl.classList.toggle('text-gray-400', !isError);
}

async function checkLogin() {
  const token = getAuthToken();
  if (!token) {
    loginNotice?.classList.remove('hidden');
    return;
  }
  try {
    const res = await authFetch('/auth/me', { method: 'GET' });
    if (!res.ok) throw new Error();
    loginNotice?.classList.add('hidden');
  } catch {
    loginNotice?.classList.remove('hidden');
  }
}

async function loadCameraThumbnail(imgEl, id) {
  try {
    const res = await authFetch(`/api/cameras/${id}/snapshot`, { method: 'GET' });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const blob = await res.blob();
    imgEl.src = URL.createObjectURL(blob);
  } catch {
    imgEl.style.visibility = 'hidden';
  }
}

async function loadCameras() {
  camGrid.innerHTML = '<div class="text-sm text-gray-500">Lade Kameras…</div>';
  emptyMsg.classList.add('hidden');
  try {
    const data = await authJsonFetch('/api/cameras', { method: 'GET' });
    const cams = (Array.isArray(data.cameras) ? data.cameras : []).filter((c) => !!c.ptz_capable);
    camGrid.innerHTML = '';
    if (!cams.length) {
      emptyMsg.classList.remove('hidden');
      return;
    }
    for (const cam of cams) {
      const card = document.createElement('div');
      card.className = 'card bg-base-200 shadow border border-base-300 cc-cam-card hover:border-primary transition-colors';
      card.innerHTML = `
        <figure class="bg-black/30">
          <img alt="${escapeHtml(cam.name)}" loading="lazy">
        </figure>
        <div class="card-body p-3">
          <div class="font-semibold text-white text-sm">${escapeHtml(cam.name)}</div>
          <div class="text-xs text-gray-500">🎮 PTZ</div>
        </div>
      `;
      card.addEventListener('click', () => openControl(cam));
      camGrid.appendChild(card);
      loadCameraThumbnail(card.querySelector('img'), cam.id);
    }
  } catch (e) {
    const status = e?.status === 401
      ? 'Bitte einloggen, um Kameras zu sehen.'
      : 'Kameras konnten nicht geladen werden.';
    camGrid.innerHTML = `<div class="text-sm text-red-400">${status}</div>`;
  }
}

function escapeHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function openControl(cam) {
  activeCameraId = cam.id;
  camNameEl.textContent = cam.name;
  setStatus('');
  listView.classList.add('hidden');
  controlView.classList.remove('hidden');
}

function closeControl() {
  sendStop();
  activeCameraId = null;
  controlView.classList.add('hidden');
  listView.classList.remove('hidden');
}

async function sendPtz(pan, tilt, zoom) {
  if (!activeCameraId) return;
  try {
    await authJsonFetch(`/api/cameras/${activeCameraId}/ptz`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pan, tilt, zoom }),
    });
    setStatus('');
  } catch (e) {
    const msg = e?.payload?.message || `Fehler (${e?.status ?? '?'})`;
    setStatus(msg, true);
  }
}

function sendStop() {
  if (moveTimer) { clearTimeout(moveTimer); moveTimer = null; }
  if (!activeCameraId) return;
  sendPtz(0, 0, 0);
}

function bindHoldButton(el, getVector) {
  if (!el) return;
  const start = (ev) => {
    ev.preventDefault();
    el.classList.add('cc-active');
    const { pan, tilt, zoom } = getVector();
    sendPtz(pan, tilt, zoom);
  };
  const stop = (ev) => {
    if (ev) ev.preventDefault();
    el.classList.remove('cc-active');
    sendStop();
  };
  el.addEventListener('pointerdown', start);
  el.addEventListener('pointerup', stop);
  el.addEventListener('pointercancel', stop);
  el.addEventListener('pointerleave', stop);
  // Fallback für Umgebungen ohne verlässliche Pointer-Events
  el.addEventListener('contextmenu', (ev) => ev.preventDefault());
}

document.querySelectorAll('.cc-pad-btn[data-pan]').forEach((btn) => {
  const pan = parseInt(btn.dataset.pan, 10) || 0;
  const tilt = parseInt(btn.dataset.tilt, 10) || 0;
  bindHoldButton(btn, () => ({ pan, tilt, zoom: 0 }));
});
bindHoldButton(zoomInBtn, () => ({ pan: 0, tilt: 0, zoom: 70 }));
bindHoldButton(zoomOutBtn, () => ({ pan: 0, tilt: 0, zoom: -70 }));

stopBtn?.addEventListener('click', sendStop);
backBtn?.addEventListener('click', closeControl);
refreshBtn?.addEventListener('click', () => { loadCameras(); checkLogin(); });

checkLogin();
loadCameras();
