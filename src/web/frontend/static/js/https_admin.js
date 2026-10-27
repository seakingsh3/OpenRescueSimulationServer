// Systemseite → „🔒 HTTPS“ (nur Admin): Zustand, Wurzelzertifikat (Download + QR) und
// Einrichten/Erneuern über scripts/setup_https.sh (Server: src/https_admin.rs).
const $ = (id) => document.getElementById(id);
const headers = () => {
  const t = localStorage.getItem('access_token') || '';
  return { 'Content-Type': 'application/json', ...(t ? { Authorization: `Bearer ${t}` } : {}) };
};
const esc = (v) => String(v ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

let pollTimer = null;
let restartWait = 0;      // nach dem Neustart des Servers noch eine Weile weiter abfragen
let serverIp = '';

async function loadIp() {
  try { serverIp = ((await fetch('/api/host/ip').then((r) => r.json())).ip || '').trim(); } catch {}
}

// Download-Adresse für Geräte: immer per HTTP und mit Netzwerk-IP – über HTTPS würde das
// Gerät den Download ablehnen, solange es dem Zertifikat noch nicht vertraut.
function caUrlForDevices() {
  const host = serverIp && !['127.0.0.1', '::1', 'localhost'].includes(serverIp) ? serverIp : location.hostname;
  const port = location.protocol === 'https:' ? '8393' : (location.port || '80');
  return `http://${host}${port === '80' ? '' : ':' + port}/ca/ors-rootCA.crt`;
}

function render(st) {
  const sec = $('systemHttps');
  sec.classList.remove('hidden');
  const badge = $('httpsBadge');
  badge.className = 'badge ' + (st.running ? 'badge-warning' : st.tls_active ? 'badge-success' : 'badge-ghost');
  badge.textContent = st.running ? 'Einrichtung läuft …' : st.tls_active ? `aktiv · Port ${st.tls_port}` : 'nicht aktiv';

  const c = st.cert;
  if (c && c.not_after) {
    const days = c.expires ? Math.round((c.expires * 1000 - Date.now()) / 86400000) : null;
    const warn = days != null && days < 30;
    $('httpsCert').innerHTML = `Serverzertifikat gültig bis <b class="${warn ? 'text-error' : ''}">${esc(new Date(c.expires * 1000).toLocaleDateString('de-DE'))}</b>`
      + (days != null ? ` (noch ${days} Tage${warn ? ' – bitte erneuern' : ''})` : '')
      + (c.names?.length ? `<br><span class="text-gray-400">für: ${c.names.map(esc).join(', ')}</span>` : '');
  } else {
    $('httpsCert').textContent = 'Noch kein Serverzertifikat eingerichtet.';
  }

  const hasCa = !!st.root_ca_download;
  $('httpsCaBox').classList.toggle('hidden', !hasCa);
  $('httpsCaMissing').classList.toggle('hidden', hasCa);
  const qr = $('httpsCaQr');
  qr.classList.toggle('hidden', !hasCa);
  if (hasCa) {
    const url = caUrlForDevices();
    qr.src = `/api/qr?text=${encodeURIComponent(url)}`;
    qr.title = url;
  }

  const btn = $('httpsSetupBtn');
  btn.disabled = !st.can_run || st.running;
  $('httpsReason').textContent = st.can_run ? '' : (st.reason || '');
  const log = $('httpsLog');
  if (st.log) {
    log.classList.remove('hidden');
    const atEnd = log.scrollTop + log.clientHeight >= log.scrollHeight - 20;
    log.textContent = st.log;
    if (atEnd) log.scrollTop = log.scrollHeight;
  }
}

async function refresh() {
  try {
    const r = await fetch('/api/admin/https/status', { headers: headers(), cache: 'no-store' });
    if (r.status === 403 || r.status === 401) return null;   // kein Admin: Karte bleibt verborgen
    const st = await r.json();
    render(st);
    return st;
  } catch {
    // Server startet gerade neu
    $('httpsBadge').textContent = 'Server startet neu …';
    return { running: true };
  }
}

function poll() {
  clearTimeout(pollTimer);
  pollTimer = setTimeout(async () => {
    const st = await refresh();
    if (st && st.running) { restartWait = 0; poll(); }
    else if (restartWait > 0) { restartWait -= 1; poll(); }
  }, 2000);
}

$('httpsSetupBtn')?.addEventListener('click', async () => {
  const names = ($('httpsNames').value || '').split(/[,\s]+/).map((s) => s.trim()).filter(Boolean);
  if (!confirm('HTTPS jetzt einrichten bzw. das Zertifikat erneuern? Der Server startet danach neu, laufende Sessions werden unterbrochen.')) return;
  const r = await fetch('/api/admin/https/setup', { method: 'POST', headers: headers(), body: JSON.stringify({ names }) });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) { alert(j.error || `Fehler ${r.status}`); return; }
  restartWait = 30;   // bis zu ~60 s über den Neustart hinweg weiter abfragen
  $('httpsLog').classList.remove('hidden');
  await refresh();
  poll();
});

loadIp().then(refresh).then((st) => { if (st?.running) poll(); });
