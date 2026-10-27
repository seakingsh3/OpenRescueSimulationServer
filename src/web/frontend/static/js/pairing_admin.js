// Einstellungen → Aktive Viewer: Geräte koppeln (PIN) und gekoppelte Geräte verwalten.
// Server: src/pairing.rs, Geräteseite: device_pairing.js.
const authHeaders = () => {
  const t = localStorage.getItem('access_token') || '';
  return { 'Content-Type': 'application/json', ...(t ? { Authorization: `Bearer ${t}` } : {}) };
};
const esc = (v) => String(v ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const fmt = (ts) => (ts ? new Date(ts * 1000).toLocaleString('de-DE', { dateStyle: 'short', timeStyle: 'short' }) : '–');

// Grobe Gerätebeschreibung aus dem User-Agent (Label beim Koppeln)
function deviceName(ua) {
  if (!ua) return 'Gerät';
  const os = /iPad/.test(ua) ? 'iPad' : /iPhone/.test(ua) ? 'iPhone' : /Android/.test(ua) ? 'Android'
    : /Windows/.test(ua) ? 'Windows' : /Mac OS X/.test(ua) ? 'Mac' : /Linux/.test(ua) ? 'Linux' : '';
  const br = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : '';
  return [os, br].filter(Boolean).join(' · ') || 'Gerät';
}

async function loadPairings() {
  const box = document.getElementById('pairingsList');
  if (!box) return;
  try {
    const r = await fetch('/api/pairings', { headers: authHeaders(), cache: 'no-store' });
    if (!r.ok) throw new Error(r.status);
    const list = (await r.json()).pairings || [];
    if (!list.length) {
      box.innerHTML = '<div class="text-gray-500">Noch keine Geräte gekoppelt.</div>';
      return;
    }
    box.innerHTML = `<div class="overflow-x-auto"><table class="table table-sm">
      <thead><tr><th>Nr.</th><th>Gerät</th><th>Gekoppelt</th><th>Zuletzt verbunden</th><th></th></tr></thead>
      <tbody>${list.map((p) => `<tr>
        <td class="font-mono">${esc(p.viewer_no)}</td>
        <td title="${esc(p.label)}">${esc(deviceName(p.label))}</td>
        <td>${fmt(p.created_at)}${p.created_by ? ` · ${esc(p.created_by)}` : ''}</td>
        <td>${fmt(p.last_seen)}</td>
        <td class="text-right"><button class="btn btn-xs btn-error btn-outline" data-unpair="${esc(p.id)}" data-no="${esc(p.viewer_no)}">Entkoppeln</button></td>
      </tr>`).join('')}</tbody></table></div>`;
    box.querySelectorAll('[data-unpair]').forEach((btn) => btn.addEventListener('click', async () => {
      if (!confirm(`Gerät für Bildschirm Nr. ${btn.dataset.no} entkoppeln? Es bekommt danach kein Bild und keine Durchsagen mehr, bis es neu gekoppelt wird.`)) return;
      await fetch(`/api/pairings/${encodeURIComponent(btn.dataset.unpair)}`, { method: 'DELETE', headers: authHeaders() });
      loadPairings();
    }));
  } catch {
    box.innerHTML = '<div class="text-red-400">Gekoppelte Geräte konnten nicht geladen werden.</div>';
  }
}

let countdownTimer = null;
async function newPin() {
  const pinEl = document.getElementById('pairPin');
  const cd = document.getElementById('pairCountdown');
  const res = document.getElementById('pairResult');
  res.textContent = '';
  pinEl.textContent = '…';
  try {
    const r = await fetch('/api/pairing/start', { method: 'POST', headers: authHeaders(), body: '{}' });
    if (!r.ok) throw new Error(r.status);
    const j = await r.json();
    pinEl.textContent = j.pin;
    let left = j.expires_in || 120;
    clearInterval(countdownTimer);
    const tick = () => {
      cd.textContent = left > 0 ? `noch ${left} s gültig` : 'abgelaufen – „Neue PIN“ tippen';
      if (left <= 0) { pinEl.textContent = '––––––'; clearInterval(countdownTimer); }
      left -= 1;
    };
    tick();
    countdownTimer = setInterval(tick, 1000);
  } catch {
    pinEl.textContent = '––––––';
    cd.textContent = 'PIN konnte nicht erzeugt werden (angemeldet?)';
  }
}

function openPairModal() {
  const m = document.getElementById('pairModal');
  if (!m) return;
  m.classList.remove('hidden');
  document.body.classList.add('modal-open');
  newPin();
}

function closePairModal() {
  const m = document.getElementById('pairModal');
  if (!m) return;
  m.classList.add('hidden');
  document.body.classList.remove('modal-open');
  clearInterval(countdownTimer);
}

document.getElementById('pairDeviceBtn')?.addEventListener('click', openPairModal);
document.getElementById('closePairModalBtn')?.addEventListener('click', closePairModal);
document.getElementById('pairNewPinBtn')?.addEventListener('click', newPin);

// Gerät hat gekoppelt → Rückmeldung im Dialog, Liste aktualisieren
(function subscribe(attempt = 0) {
  if (window.orsUiWs && typeof window.orsUiWs.on === 'function') {
    window.orsUiWs.on((msg) => {
      if (msg?.type !== 'device_paired') return;
      const m = document.getElementById('pairModal');
      if (m && !m.classList.contains('hidden')) {
        document.getElementById('pairResult').textContent = `✅ Bildschirm Nr. ${msg.viewer_no} gekoppelt`;
        clearInterval(countdownTimer);
        document.getElementById('pairPin').textContent = '✓';
        document.getElementById('pairCountdown').textContent = '';
        setTimeout(closePairModal, 2000);
      }
      loadPairings();
    });
    return;
  }
  if (attempt < 50) setTimeout(() => subscribe(attempt + 1), 200);
})();

loadPairings();
setInterval(() => { if (!document.hidden) loadPairings(); }, 15000);
