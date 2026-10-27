// === participants.js ===============================================
// Ausbilder-Seite: Teilnehmer-Notizblätter anlegen, Links/QR verteilen, verwalten.
import { authJsonFetch } from '/static/js/http.js';

const $ = (s) => document.querySelector(s);
const bodyEl = $('#tnBody');
const statusEl = $('#tnStatus');

let sheets = [];
let serverBaseCache = null;
let refreshTimer = null;

function toast(msg, ok = true) {
  if (typeof window.showUiToast === 'function') window.showUiToast(msg, ok);
  else console.log(msg);
}
function escapeHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}
function fmtTs(ts) {
  if (!ts) return '—';
  try { return new Date(ts * 1000).toLocaleString('de-DE'); } catch { return '—'; }
}

// Basis-URL, die Teilnehmergeräte erreichen: bei localhost die Server-IP verwenden
async function serverBase() {
  if (serverBaseCache) return serverBaseCache;
  const loc = window.location;
  let host = loc.hostname;
  if (/^(localhost|127\.|0\.0\.0\.0|\[::1\]|::1)/.test(host)) {
    try {
      const r = await fetch('/api/host/ip');
      const j = r.ok ? await r.json() : null;
      const ip = (j?.ip || '').trim();
      if (ip) host = ip;
    } catch {}
  }
  serverBaseCache = `${loc.protocol}//${host}${loc.port ? ':' + loc.port : ''}`;
  return serverBaseCache;
}
async function sheetUrl(s) {
  return `${await serverBase()}${s.path}`;
}

async function load() {
  try {
    const res = await authJsonFetch('/api/participants');
    sheets = Array.isArray(res?.sheets) ? res.sheets : [];
    if (statusEl) statusEl.textContent = '';
    render();
  } catch (e) {
    if (statusEl) {
      statusEl.textContent = e.status === 401
        ? 'Bitte einloggen, um Notizblätter zu verwalten.'
        : 'Notizblätter konnten nicht geladen werden.';
    }
    if (bodyEl) bodyEl.innerHTML = `<tr><td colspan="5" class="text-gray-500">—</td></tr>`;
  }
  try {
    const st = await authJsonFetch('/api/sim/status');
    const el = $('#tnActiveSession');
    if (el) {
      el.textContent = st?.active
        ? `Aktive Session: „${st.name || st.session_id}“ (#${st.session_id})`
        : 'Aktive Session: keine';
    }
  } catch {}
}

function linkBadge(s) {
  if (s.session_id && s.session_active) {
    return `<span class="badge badge-success badge-sm">läuft: ${escapeHtml(s.session_name || ('#' + s.session_id))}</span>`;
  }
  if (s.session_id) {
    return `<span class="badge badge-ghost badge-sm">beendet: ${escapeHtml(s.session_name || ('#' + s.session_id))}</span>`;
  }
  return `<span class="badge badge-outline badge-sm">nicht verknüpft</span>`;
}

function render() {
  if (!bodyEl) return;
  if (!sheets.length) {
    bodyEl.innerHTML = `<tr><td colspan="5" class="text-gray-500">Noch keine Notizblätter. Oben eines anlegen.</td></tr>`;
    return;
  }
  bodyEl.innerHTML = sheets.map((s) => `
    <tr data-id="${s.id}">
      <td class="font-semibold text-white">${escapeHtml(s.name)}</td>
      <td class="whitespace-nowrap">${fmtTs(s.created_at)}</td>
      <td>${s.field_count || 0}${s.updated_at ? `<div class="text-[10px] text-gray-500">Stand ${fmtTs(s.updated_at)}</div>` : ''}</td>
      <td>${linkBadge(s)}</td>
      <td class="text-right">
        <div class="flex gap-1 justify-end flex-wrap">
          <button class="btn btn-xs btn-outline" data-action="qr" data-id="${s.id}">📲 QR</button>
          <button class="btn btn-xs btn-outline" data-action="copy" data-id="${s.id}">Link kopieren</button>
          <a class="btn btn-xs btn-outline" href="${escapeHtml(s.path)}" target="_blank" rel="noopener">Öffnen</a>
          <a class="btn btn-xs btn-outline" href="${escapeHtml(s.path)}/download" target="_blank" rel="noopener">Ansicht/Druck</a>
          ${s.session_id ? `<button class="btn btn-xs btn-outline btn-warning" data-action="unlink" data-id="${s.id}">Lösen</button>` : ''}
          <button class="btn btn-xs btn-error" data-action="delete" data-id="${s.id}">Löschen</button>
        </div>
      </td>
    </tr>
  `).join('');

  bodyEl.querySelectorAll('[data-action]').forEach((btn) => {
    btn.addEventListener('click', () => onAction(btn.dataset.action, Number(btn.dataset.id)));
  });
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand('copy');
      ta.remove();
      return ok;
    } catch { return false; }
  }
}

async function onAction(action, id) {
  const s = sheets.find((x) => x.id === id);
  if (!s) return;
  if (action === 'qr') {
    const url = await sheetUrl(s);
    $('#tnQrTitle').textContent = `QR-Code: ${s.name}`;
    $('#tnQrImg').src = `/api/qr?text=${encodeURIComponent(url)}`;
    $('#tnQrLink').textContent = url;
    $('#tnQrOpen').href = url;
    $('#tnQrCopy').onclick = async () => toast((await copyText(url)) ? 'Link kopiert ✅' : 'Kopieren fehlgeschlagen ❌', true);
    $('#tnQrModal').classList.remove('hidden');
    return;
  }
  if (action === 'copy') {
    const url = await sheetUrl(s);
    toast((await copyText(url)) ? `Link kopiert ✅ ${url}` : 'Kopieren fehlgeschlagen ❌', true);
    return;
  }
  if (action === 'unlink') {
    try {
      await authJsonFetch(`/api/participants/${id}/unlink`, { method: 'POST' });
      toast('Verknüpfung gelöst ✅', true);
      await load();
    } catch { toast('Lösen fehlgeschlagen ❌', false); }
    return;
  }
  if (action === 'delete') {
    if (!confirm(`Notizblatt „${s.name}“ wirklich löschen? Der Link wird ungültig, bereits gesetzte Marker bleiben.`)) return;
    try {
      await authJsonFetch(`/api/participants/${id}`, { method: 'DELETE' });
      toast('Notizblatt gelöscht ✅', true);
      await load();
    } catch { toast('Löschen fehlgeschlagen ❌', false); }
  }
}

async function create() {
  const input = $('#tnNewName');
  const name = (input?.value || '').trim();
  if (!name) { toast('Bitte einen Namen eingeben.', false); input?.focus(); return; }
  const btn = $('#tnCreate');
  if (btn) btn.disabled = true;
  try {
    const created = await authJsonFetch('/api/participants', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    if (input) input.value = '';
    toast(`Notizblatt „${created.name}“ angelegt ✅`, true);
    await load();
    // QR direkt anbieten
    const s = sheets.find((x) => x.id === created.id);
    if (s) onAction('qr', s.id);
  } catch (e) {
    toast(e.status === 401 ? 'Bitte einloggen ❌' : 'Anlegen fehlgeschlagen ❌', false);
  } finally {
    if (btn) btn.disabled = false;
  }
}

$('#tnCreate')?.addEventListener('click', create);
$('#tnNewName')?.addEventListener('keydown', (ev) => { if (ev.key === 'Enter') create(); });
$('#tnRefresh')?.addEventListener('click', load);
$('#tnQrClose')?.addEventListener('click', () => $('#tnQrModal').classList.add('hidden'));
$('#tnQrModal')?.addEventListener('click', (e) => { if (e.target === e.currentTarget) e.currentTarget.classList.add('hidden'); });

// Live-Updates: Marker/Session-Events der zentralen UI-WS nutzen, sonst Intervall
if (window.orsUiWs?.on) {
  window.orsUiWs.on((msg) => {
    if (['sim_started', 'sim_stopped', 'room_switched'].includes(msg?.type)) load();
  });
}
refreshTimer = setInterval(load, 15000);
load();
