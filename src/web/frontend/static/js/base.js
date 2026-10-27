// base.js – globale Features (PTT „Voice of Room“, Hotkeys, UI-WS, Toasts)

// --------------------------------------------------------------
// PTT / Voice of Room: Browser-Mikrofon → Server → Raumlautsprecher (+ Viewer)
// --------------------------------------------------------------
const pttBtn = document.getElementById('pttBtn');
const PTT_MODE_KEY = 'ors_ptt_mode';   // 'hold' | 'toggle'
const PTT_GROUP_KEY = 'ors_ptt_group';

const pttState = {
  pc: null,
  ws: null,
  stream: null,
  sending: false,
  audioCtx: null,
  analyser: null,
  meterTimer: null,
  mode: 'hold',
};

function pttAuthHeaders() {
  const t = localStorage.getItem('access_token') || localStorage.getItem('token') || '';
  return t ? { Authorization: `Bearer ${t}` } : {};
}

function pttSetStatus(text, kind = 'info') {
  const el = document.getElementById('pttStatus');
  if (!el) return;
  el.textContent = text;
  el.classList.remove('text-gray-200', 'text-emerald-300', 'text-red-300', 'text-amber-300');
  el.classList.add(kind === 'ok' ? 'text-emerald-300' : kind === 'error' ? 'text-red-300' : kind === 'warn' ? 'text-amber-300' : 'text-gray-200');
}

function setPttUi(active) {
  if (pttBtn) {
    pttBtn.classList.toggle('btn-success', active);
    pttBtn.classList.toggle('btn-info', !active);
    pttBtn.dataset.tip = active ? 'Durchsage läuft' : 'PTT';
  }
  const big = document.getElementById('pttModalStart');
  if (big) {
    big.style.background = active ? '#16a34a' : '#2563eb';
    big.style.borderColor = active ? '#86efac' : '#60a5fa';
    const label = big.querySelector('span');
    if (label) {
      label.innerHTML = active
        ? 'Spricht…<br><span class="text-sm font-normal">' + (pttState.mode === 'toggle' ? 'zum Beenden tippen' : 'loslassen zum Beenden') + '</span>'
        : (pttState.mode === 'toggle' ? 'Tippen<br>zum Sprechen' : 'Halten<br>zum Sprechen');
    }
  }
}

function pttStopMeter() {
  if (pttState.meterTimer) { clearInterval(pttState.meterTimer); pttState.meterTimer = null; }
  if (pttState.audioCtx) { try { pttState.audioCtx.close(); } catch {} }
  pttState.audioCtx = null;
  pttState.analyser = null;
  const bar = document.getElementById('pttMeterBar');
  if (bar) bar.style.width = '0%';
}

function pttStartMeter(stream) {
  try {
    const Ctx = window.AudioContext || window.webkitAudioContext;
    if (!Ctx) return;
    const ctx = new Ctx();
    const src = ctx.createMediaStreamSource(stream);
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 512;
    src.connect(analyser);
    const data = new Uint8Array(analyser.frequencyBinCount);
    pttState.audioCtx = ctx;
    pttState.analyser = analyser;
    const bar = document.getElementById('pttMeterBar');
    pttState.meterTimer = setInterval(() => {
      analyser.getByteTimeDomainData(data);
      let sum = 0;
      for (let i = 0; i < data.length; i++) { const v = (data[i] - 128) / 128; sum += v * v; }
      const rms = Math.sqrt(sum / data.length);
      const pct = Math.min(100, Math.round(rms * 300));
      if (bar) {
        bar.style.width = pct + '%';
        bar.classList.toggle('bg-emerald-500', pct < 80);
        bar.classList.toggle('bg-red-500', pct >= 80);
      }
    }, 80);
  } catch (e) {
    console.warn('PTT meter failed', e);
  }
}

// Läuft eine Session in einem Raum mit zugeordneten Lautsprechern, ist der Raum ein
// eigenes Durchsage-Ziel (value "room:<id>"), unabhängig von den Lautsprecher-Gruppen.
async function pttRoomTarget() {
  try {
    const st = await fetch('/api/sim/status', { cache: 'no-store' }).then((r) => r.json());
    if (!st?.active || st.room_id == null) return null;
    const [room, sp] = await Promise.all([
      fetch(`/api/rooms/${st.room_id}`, { headers: pttAuthHeaders(), cache: 'no-store' }).then((r) => (r.ok ? r.json() : null)),
      fetch('/api/voice/speakers', { headers: pttAuthHeaders(), cache: 'no-store' }).then((r) => (r.ok ? r.json() : { speakers: [] })),
    ]);
    let ids = [];
    try { ids = JSON.parse(room?.speakers || '[]'); } catch {}
    if (!Array.isArray(ids) || !ids.length) return null;
    const online = (sp.speakers || []).filter((s) => ids.includes(s.id) && s.online).length;
    return { group: `room:${st.room_id}`, label: `🏠 Raum ${room.name}`, total: ids.length, online };
  } catch {
    return null;
  }
}

async function pttLoadGroups() {
  const sel = document.getElementById('pttGroupSelect');
  const info = document.getElementById('pttGroupInfo');
  if (!sel) return;
  const remembered = localStorage.getItem(PTT_GROUP_KEY) || 'default';
  try {
    const r = await fetch('/api/voice/groups', { headers: pttAuthHeaders(), cache: 'no-store' });
    if (r.status === 401) {
      if (info) info.textContent = 'Bitte einloggen, um Lautsprecher zu sehen.';
      return;
    }
    const j = r.ok ? await r.json() : { groups: [] };
    const groups = Array.isArray(j.groups) ? j.groups : [];
    if (!groups.some((g) => g.group === 'default')) groups.unshift({ group: 'default', total: 0, online: 0 });
    const room = await pttRoomTarget();
    const options = groups.map((g) => ({ ...g, label: g.group }));
    if (room) options.unshift(room);
    const esc = (v) => String(v).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
    sel.innerHTML = options.map((g) =>
      `<option value="${esc(g.group)}">${esc(g.label)} (${g.online}/${g.total} online)</option>`
    ).join('');
    // Raum der laufenden Session vorauswählen, außer es wurde bewusst eine Gruppe gewählt
    let explicit = false;
    try { explicit = sessionStorage.getItem('ors_ptt_group_explicit') === '1'; } catch {}
    const preferRoom = room && (remembered.startsWith('room:') ? remembered === room.group || !explicit : !explicit);
    sel.value = preferRoom ? room.group
      : options.some((g) => g.group === remembered) ? remembered : options[0].group;
    pttUpdateGroupInfo(options);
    sel.onchange = () => pttUpdateGroupInfo(options);
  } catch (e) {
    if (info) info.textContent = 'Lautsprecherliste nicht erreichbar.';
  }
}

function pttUpdateGroupInfo(options) {
  const sel = document.getElementById('pttGroupSelect');
  const info = document.getElementById('pttGroupInfo');
  if (!sel || !info) return;
  const cur = options.find((g) => g.group === sel.value);
  const isRoom = sel.value.startsWith('room:');
  info.textContent = cur && cur.total
    ? `${cur.online} von ${cur.total} Lautsprecher(n) ${isRoom ? 'im Raum ' : ''}online`
    : 'Keine Lautsprecher in dieser Gruppe. Geräte melden sich unter Einstellungen → Kameras & Räume → Raumlautsprecher.';
}

async function stopPtt() {
  pttState.sending = false;
  pttStopMeter();
  if (pttState.pc) {
    try { pttState.pc.getSenders().forEach((s) => s.track && s.track.stop()); pttState.pc.close(); } catch {}
  }
  if (pttState.ws) {
    try { pttState.ws.send(JSON.stringify({ type: 'bye' })); } catch {}
    try { pttState.ws.onclose = null; pttState.ws.close(); } catch {}
  }
  if (pttState.stream) {
    try { pttState.stream.getTracks().forEach((t) => t.stop()); } catch {}
  }
  pttState.pc = null;
  pttState.ws = null;
  pttState.stream = null;
  setPttUi(false);
  pttSetStatus('Bereit');
}

async function startPtt(opts = {}) {
  if (pttState.sending) return;
  const forwardViewers = !!opts.forwardViewers;
  const group = (opts.group || 'default').trim() || 'default';
  try {
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      const msg = 'Kein Mikrofonzugriff: Seite muss über HTTPS oder localhost geöffnet sein.';
      pttSetStatus(msg, 'error');
      if (typeof window.showUiToast === 'function') window.showUiToast(msg, false);
      return;
    }
    pttState.sending = true;
    pttSetStatus('Mikrofon wird geöffnet…');
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      video: false,
    });
    // Nur Host-Kandidaten: im Übungsnetz gibt es keinen STUN-Server
    const pc = new RTCPeerConnection({ iceServers: [] });
    stream.getTracks().forEach((t) => pc.addTrack(t, stream));

    const query = [`group=${encodeURIComponent(group)}`];
    if (forwardViewers) query.push('viewers=1');
    // Durchsagen nur mit Login: Token als Parameter (WebSockets können keinen Header setzen)
    const pttToken = localStorage.getItem('access_token') || localStorage.getItem('token') || '';
    if (pttToken) query.push(`token=${encodeURIComponent(pttToken)}`);
    const wsUrl = (location.protocol === 'https:' ? 'wss' : 'ws') + '://' + location.host + '/ws/voice/tx?' + query.join('&');
    const ws = new WebSocket(wsUrl);

    ws.onmessage = async (ev) => {
      let msg;
      try { msg = JSON.parse(ev.data); } catch { return; }
      try {
        if (msg.type === 'answer' && msg.sdp) {
          await pc.setRemoteDescription({ type: 'answer', sdp: msg.sdp });
        } else if (msg.type === 'ice' && msg.candidate != null) {
          await pc.addIceCandidate({ candidate: msg.candidate, sdpMLineIndex: msg.sdpMLineIndex ?? 0 });
        } else if (msg.type === 'status') {
          const n = msg.speakers_online || 0, r = msg.speakers_receiving || 0;
          if (n === 0) pttSetStatus(`Sendet · keine Lautsprecher in „${msg.group}“ online`, 'warn');
          else pttSetStatus(`Sendet · ${r} von ${n} Lautsprecher(n) empfangen${forwardViewers ? ' · Viewer' : ''}`, r > 0 ? 'ok' : 'warn');
        } else if (msg.type === 'error') {
          pttSetStatus('Fehler: ' + (msg.message || 'unbekannt'), 'error');
          if (typeof window.showUiToast === 'function') window.showUiToast('Durchsage: ' + msg.message, false);
          stopPtt();
        }
      } catch (e) {
        console.warn('PTT message handling failed', e);
      }
    };
    ws.onerror = () => { pttSetStatus('Verbindung fehlgeschlagen', 'error'); stopPtt(); };
    ws.onclose = () => stopPtt();

    pc.onicecandidate = (ev) => {
      if (ev.candidate && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'ice', candidate: ev.candidate.candidate, sdpMLineIndex: ev.candidate.sdpMLineIndex }));
      }
    };
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'connected') pttSetStatus('Verbunden, sendet…', 'ok');
      if (pc.connectionState === 'failed') { pttSetStatus('WebRTC-Verbindung fehlgeschlagen', 'error'); stopPtt(); }
    };

    const offer = await pc.createOffer({ offerToReceiveAudio: false, offerToReceiveVideo: false });
    await pc.setLocalDescription(offer);

    const sendOffer = () => {
      ws.send(JSON.stringify({ type: 'config', viewers: forwardViewers, group }));
      ws.send(JSON.stringify({ type: 'offer', sdp: offer.sdp }));
      pttSetStatus('Verbinde…');
      setPttUi(true);
    };
    // Der WebSocket kann schon offen sein, während oben noch das Angebot erzeugt wurde –
    // dann käme onopen nie mehr, das Angebot bliebe ungesendet und die Durchsage stumm,
    // obwohl die Statuszeile „Sendet …“ zeigt.
    if (ws.readyState === WebSocket.OPEN) sendOffer();
    else ws.onopen = sendOffer;

    pttState.pc = pc;
    pttState.ws = ws;
    pttState.stream = stream;
    pttStartMeter(stream);
    try { localStorage.setItem(PTT_GROUP_KEY, group); } catch {}
  } catch (e) {
    console.error('PTT start failed', e);
    pttSetStatus(e && e.name === 'NotAllowedError' ? 'Mikrofon verweigert' : 'Start fehlgeschlagen', 'error');
    await stopPtt();
  }
}

// --- Modal ---
const pttModal = document.getElementById('pttModal');
const pttModalClose = document.getElementById('pttModalClose');
const pttModalCancel = document.getElementById('pttModalCancel');
const pttModalStart = document.getElementById('pttModalStart');
const pttModeToggle = document.getElementById('pttModeToggle');
const pttGroupSelect = document.getElementById('pttGroupSelect');

function pttApplyMode(mode) {
  pttState.mode = mode === 'toggle' ? 'toggle' : 'hold';
  if (pttModeToggle) pttModeToggle.checked = pttState.mode === 'toggle';
  try { localStorage.setItem(PTT_MODE_KEY, pttState.mode); } catch {}
  setPttUi(pttState.sending);
}

function openPttModal() {
  if (!pttModal) return;
  pttModal.classList.remove('hidden');
  document.body.classList.add('modal-open');
  pttApplyMode(localStorage.getItem(PTT_MODE_KEY) || 'hold');
  pttLoadGroups();
}
function closePttModal() {
  if (pttModal) pttModal.classList.add('hidden');
  document.body.classList.remove('modal-open');
}

function pttCurrentOpts() {
  return {
    forwardViewers: !!document.getElementById('pttForwardViewers')?.checked,
    group: pttGroupSelect?.value || 'default',
  };
}

if (pttBtn) pttBtn.addEventListener('click', openPttModal);
if (pttModalClose) pttModalClose.addEventListener('click', closePttModal);
if (pttModalCancel) pttModalCancel.addEventListener('click', closePttModal);
if (pttModeToggle) pttModeToggle.addEventListener('change', () => pttApplyMode(pttModeToggle.checked ? 'toggle' : 'hold'));
if (pttGroupSelect) pttGroupSelect.addEventListener('change', () => {
  try {
    localStorage.setItem(PTT_GROUP_KEY, pttGroupSelect.value);
    // Bewusste Wahl in dieser Sitzung: den Raum nicht mehr automatisch vorauswählen
    sessionStorage.setItem('ors_ptt_group_explicit', '1');
  } catch {}
});

if (pttModalStart) {
  // Halten: pointerdown startet, loslassen stoppt. Umschalten: Tippen startet/stoppt.
  let holdActive = false;
  pttModalStart.addEventListener('contextmenu', (e) => e.preventDefault());
  pttModalStart.addEventListener('pointerdown', (e) => {
    if (pttState.mode !== 'hold') return;
    e.preventDefault();
    holdActive = true;
    try { pttModalStart.setPointerCapture(e.pointerId); } catch {}
    startPtt(pttCurrentOpts());
  });
  const endHold = () => {
    if (pttState.mode !== 'hold' || !holdActive) return;
    holdActive = false;
    stopPtt();
  };
  pttModalStart.addEventListener('pointerup', endHold);
  pttModalStart.addEventListener('pointercancel', endHold);
  pttModalStart.addEventListener('lostpointercapture', endHold);
  pttModalStart.addEventListener('click', (e) => {
    if (pttState.mode !== 'toggle') return;
    e.preventDefault();
    if (pttState.sending) stopPtt();
    else startPtt(pttCurrentOpts());
  });
  // Leertaste im offenen Modal: wie Taste halten
  window.addEventListener('keydown', (ev) => {
    if (!pttModal || pttModal.classList.contains('hidden')) return;
    if (ev.code !== 'Space' || ev.repeat) return;
    const tag = (ev.target?.tagName || '').toLowerCase();
    if (tag === 'input' || tag === 'select' || tag === 'textarea') return;
    ev.preventDefault();
    if (pttState.mode === 'toggle') { if (pttState.sending) stopPtt(); else startPtt(pttCurrentOpts()); }
    else if (!pttState.sending) startPtt(pttCurrentOpts());
  });
  window.addEventListener('keyup', (ev) => {
    if (!pttModal || pttModal.classList.contains('hidden')) return;
    if (ev.code !== 'Space' || pttState.mode !== 'hold') return;
    stopPtt();
  });
}

// Cleanup on page unload
window.addEventListener('beforeunload', () => stopPtt());

// --------------------------------------------------------------
// Global Hotkeys
// --------------------------------------------------------------
(function bindGlobalHotkeysOnce() {
  if (window.__orsGlobalHotkeysBound) return;
  window.__orsGlobalHotkeysBound = true;

  function isTypingTarget(el) {
    if (!el) return false;
    const tag = (el.tagName || '').toLowerCase();
    return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable;
  }

  window.addEventListener('keydown', (ev) => {
    if (!ev.ctrlKey || ev.altKey || ev.metaKey) return;
    if (isTypingTarget(ev.target)) return;
    const key = ev.key.toLowerCase();
    if (key === 'd') {
      ev.preventDefault();
      window.location.href = '/ui/debrief';
    }
  });
})();

// --------------------------------------------------------------
// Achievement Toasts + UI-WS (centralized)
// --------------------------------------------------------------
const toastHost = document.getElementById('achievementToastHost');
let uiWs = null;
let uiWsRetry = 0;
const uiWsListeners = new Set();

function escapeHtml(s) {
  return String(s || '').replace(/[&<>"]/g, (c) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
  }[c]));
}

function removeToast(el) {
  if (!el) return;
  el.classList.add('out');
  setTimeout(() => el.remove(), 320);
}

function showUiToast({ icon = 'ℹ️', title = 'Hinweis', name = '', meta = '', desc = '', durationMs = 6500 }) {
  if (!toastHost) return;
  const el = document.createElement('div');
  el.className = 'achievement-toast';
  el.style.setProperty('--toast-duration', `${durationMs}ms`);
  el.innerHTML = `
    <div class="flex items-start gap-3 relative z-10">
      <div class="text-2xl">${icon}</div>
      <div class="flex-1">
        <div class="toast-title">${escapeHtml(title)}</div>
        <div class="toast-name">${escapeHtml(name)}</div>
        <div class="toast-meta">${escapeHtml(meta)}</div>
        <div class="toast-desc">${escapeHtml(desc)}</div>
        <div class="toast-bar"></div>
      </div>
    </div>
  `;
  el.addEventListener('click', () => removeToast(el));
  toastHost.prepend(el);
  const maxToasts = 5;
  const toasts = toastHost.querySelectorAll('.achievement-toast');
  if (toasts.length > maxToasts) {
    for (let i = maxToasts; i < toasts.length; i += 1) {
      removeToast(toasts[i]);
    }
  }
  setTimeout(() => removeToast(el), durationMs + 400);
}

function showAchievementToast(username, achievement, durationMs = 6500) {
  if (!achievement) return;
  const userLabel = username ? `von ${username}` : 'von Unbekannt';
  const points = Number.isFinite(achievement.points) ? ` • ${achievement.points} XP` : '';
  showUiToast({
    icon: '🏆',
    title: 'Achievement freigeschaltet',
    name: achievement.name || achievement.key || 'Unbekannt',
    meta: `${userLabel}${points}`,
    desc: achievement.description || '',
    durationMs,
  });
}

function enqueueAchievementToasts(username, achievements = []) {
  const items = Array.isArray(achievements) ? achievements : [];
  items.forEach((ach, idx) => {
    const delay = idx * 350;
    setTimeout(() => showAchievementToast(username, ach), delay);
  });
}

function emitUiWsMessage(raw) {
  let msg = null;
  try { msg = JSON.parse(raw); } catch { return; }
  uiWsListeners.forEach((cb) => {
    try { cb(msg); } catch (e) { console.warn('ui ws listener failed', e); }
  });
  if (msg?.type === 'achievement_unlocked') {
    const username = msg.username || msg.user_name || msg.user || '';
    const achievements = msg.achievements || [];
    enqueueAchievementToasts(username, achievements);
  }
  if (msg?.type === 'record_failed') {
    const element = msg.element ? `Element: ${msg.element}` : 'Recording-Branch';
    const reason = msg.message || 'Aufnahme wurde unterbrochen.';
    showUiToast({
      icon: '⛔',
      title: 'Recording abgebrochen',
      name: 'Aufnahmefehler erkannt',
      meta: element,
      desc: reason,
      durationMs: 9000,
    });
  }
  if (msg?.type === 'distributor_failed') {
    showUiToast({
      icon: '📡',
      title: 'Externer Verteiler nicht erreichbar',
      name: 'Übung läuft weiter',
      meta: 'Nur die Weitergabe an den Streaming-Server ist ausgefallen.',
      desc: msg.message || '',
      durationMs: 9000,
    });
  }
}

function connectUiWs() {
  if (!toastHost) return;
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const url = `${proto}://${location.host}/ws`;
  uiWs = new WebSocket(url);
  uiWs.onmessage = (ev) => emitUiWsMessage(ev.data);
  uiWs.onopen = () => {
    uiWsRetry = 0;
  };
  uiWs.onclose = () => {
    uiWsRetry = Math.min(uiWsRetry + 1, 6);
    const wait = Math.min(8000, 800 + uiWsRetry * 900);
    setTimeout(connectUiWs, wait);
  };
  uiWs.onerror = () => {
    try { uiWs.close(); } catch {}
  };
}

connectUiWs();

window.orsUiWs = {
  on: (cb) => {
    if (typeof cb === 'function') uiWsListeners.add(cb);
    return () => uiWsListeners.delete(cb);
  },
  off: (cb) => uiWsListeners.delete(cb),
};
