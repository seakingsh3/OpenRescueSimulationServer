// -------- Toast ----------------------------------------------------
import { authFetch, withAuthHeaders } from '/static/js/http.js';

const $ = (s) => document.querySelector(s);
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

// ---------------- Auth / User Mgmt ----------------
async function fetchCurrentUser() {
  const token = localStorage.getItem('access_token') || localStorage.getItem('token');
  if (!token) {
    _currentUser = null;
    updateUserSectionVisibility();
    return null;
  }
  try {
    const me = await _fetchJson('/auth/me', { method: 'GET' });
    _currentUser = {
      id: me.user_id ?? me.id ?? null,
      username: me.username || '',
      role: me.role || 'user',
    };
  } catch (e) {
    console.warn('auth/me failed', e);
    _currentUser = null;
  }
  updateUserSectionVisibility();
  return _currentUser;
}

// ---------------- Invite Tokens ----------------
function bindInviteTokenModal() {
  const btn = document.getElementById('inviteTokenBtn');
  const value = document.getElementById('inviteTokenValue');
  const copy = document.getElementById('inviteTokenCopy');
  const status = document.getElementById('inviteTokenStatus');

  if (copy) {
    copy.addEventListener('click', async () => {
      if (!value || !value.value) return;
      try {
        await navigator.clipboard.writeText(value.value);
        showToast('Token kopiert ✅', true);
      } catch {
        showToast('Kopieren fehlgeschlagen ❌', false);
      }
    });
  }
  if (btn) {
    btn.addEventListener('click', async () => {
      if (status) status.textContent = 'Erzeuge Token…';
      try {
        const res = await _fetchJson('/api/admin/invite_tokens', { method: 'POST' });
        if (value) value.value = res.token || '';
        if (status) status.textContent = 'Token erzeugt';
        showToast('Token erzeugt ✅', true);
      } catch (e) {
        console.error('invite token failed', e);
        if (status) status.textContent = 'Fehler beim Erzeugen';
        showToast('Token konnte nicht erzeugt werden ❌', false);
      }
    });
  }
}

function bindSelfRegister() {
  const btn = document.getElementById('selfRegisterBtn');
  const tokenEl = document.getElementById('selfRegisterToken');
  const nameEl = document.getElementById('selfRegisterName');
  const passEl = document.getElementById('selfRegisterPass');
  const status = document.getElementById('selfRegisterStatus');
  if (!btn) return;
  btn.addEventListener('click', async () => {
    const token = (tokenEl?.value || '').trim();
    const username = (nameEl?.value || '').trim();
    const password = passEl?.value || '';
    if (!token || !username || !password) {
      if (status) status.textContent = 'Bitte alle Felder ausfüllen';
      return;
    }
    if (status) status.textContent = 'Registriere…';
    try {
      const res = await _fetchJson('/auth/register', {
        method: 'POST',
        body: { token, username, password },
      });
      localStorage.setItem('token', res.access_token);
      localStorage.setItem('access_token', res.access_token);
      if (res.username) localStorage.setItem('username', res.username);
      if (status) status.textContent = 'Registriert ✅';
      showToast('Registrierung erfolgreich ✅', true);
      if (tokenEl) tokenEl.value = '';
      if (nameEl) nameEl.value = '';
      if (passEl) passEl.value = '';
      fetchCurrentUser().catch(() => {});
    } catch (e) {
      console.error('register failed', e);
      if (status) status.textContent = 'Registrierung fehlgeschlagen';
      showToast('Registrierung fehlgeschlagen ❌', false);
    }
  });
}

function updateUserSectionVisibility() {
  const sec = document.getElementById('userSection');
  const adminSec = document.getElementById('adminSettingsSection');
  const sourcesSec = document.getElementById('sourcesSection');
  const sourcesNote = document.getElementById('sourcesAdminNote');
  const sourcesBtn = document.getElementById('openSourcesModalBtn');
  const isAdmin = _currentUser && (_currentUser.role || '').toLowerCase() === 'admin';

  if (sec) {
    if (isAdmin) {
      sec.classList.remove('hidden');
      loadUsers().catch(() => {});
    } else {
      sec.classList.add('hidden');
    }
  }

  if (adminSec) {
    if (isAdmin) {
      adminSec.classList.remove('hidden');
      loadAdminSettings().catch(() => {});
      loadUpdateFiles().catch(() => {});
      prepareVodModal().catch(() => {});
    } else {
      adminSec.classList.add('hidden');
    }
  }

  if (sourcesSec) {
    if (isAdmin) {
      if (sourcesBtn) sourcesBtn.disabled = false;
      if (sourcesNote) sourcesNote.classList.add('hidden');
      sourcesSec.classList.remove('opacity-60', 'pointer-events-none');
    } else {
      if (sourcesBtn) sourcesBtn.disabled = true;
      if (sourcesNote) sourcesNote.classList.remove('hidden');
      sourcesSec.classList.add('opacity-60', 'pointer-events-none');
    }
  }
}

// ---------------- VOD Downloads (Admin) ----------------
let _vodSessionsCache = [];

async function loadVodSessions() {
  const sel = document.getElementById('vodSessionSelect');
  if (!sel) return;
  sel.innerHTML = `<option value="">Lade Sessions…</option>`;
  try {
    const res = await _fetchJson('/api/sessions', { method: 'GET' });
    const list = Array.isArray(res?.sessions) ? res.sessions : [];
    _vodSessionsCache = list;
    if (!list.length) {
      sel.innerHTML = `<option value="">Keine Sessions</option>`;
      return;
    }
    sel.innerHTML = `<option value="">Session wählen…</option>` + list.map(s => {
      const name = s.name || `Session #${s.id}`;
      const stop = s.stop_time ? new Date(s.stop_time * 1000).toLocaleString() : 'aktiv/—';
      return `<option value="${s.id}">#${s.id} • ${escapeHtml(name)} • ${escapeHtml(stop)}</option>`;
    }).join('');
  } catch (e) {
    console.error('loadVodSessions failed', e);
    sel.innerHTML = `<option value="">Fehler beim Laden</option>`;
  }
}

function vodLinkFromItem(item) {
  if (item?.url && String(item.url).startsWith('/vod/')) {
    return item.url;
  }
  const fp = item?.file_path ? String(item.file_path) : '';
  const idx = fp.indexOf('/vod/');
  if (idx >= 0) {
    return fp.slice(idx);
  }
  return '';
}

async function loadVodListForSession(sessionId) {
  const listEl = document.getElementById('vodList');
  if (!listEl) return;
  if (!sessionId) {
    listEl.innerHTML = `<div class="text-gray-500 text-xs">Bitte Session wählen.</div>`;
    return;
  }
  listEl.innerHTML = `<div class="text-gray-500 text-xs">Lade VODs…</div>`;
  try {
    const res = await _fetchJson(`/api/sessions/${encodeURIComponent(sessionId)}/vod`, { method: 'GET' });
    const items = Array.isArray(res?.items) ? res.items : [];
    if (!items.length) {
      listEl.innerHTML = `<div class="text-gray-500 text-xs">Keine VODs gefunden.</div>`;
      return;
    }
    listEl.innerHTML = items.map(item => {
      const link = vodLinkFromItem(item);
      const label = item.label || item.kind || 'vod';
      const size = Number.isFinite(item.size_bytes) ? `${(item.size_bytes / (1024 * 1024)).toFixed(1)} MB` : '';
      const dur = Number.isFinite(item.duration_sec) ? `${Math.round(item.duration_sec)}s` : '';
      const meta = [size, dur].filter(Boolean).join(' • ');
      const linkHtml = link
        ? `<a href="${link}" class="btn btn-xs btn-outline" download>Download</a>`
        : `<span class="text-xs text-gray-500">Kein Download-Link</span>`;
      return `
        <div class="flex items-center justify-between gap-3 bg-gray-800 border border-gray-700 rounded px-3 py-2">
          <div>
            <div class="text-white text-sm font-medium">${escapeHtml(label)}</div>
            <div class="text-xs text-gray-400">${escapeHtml(meta)}</div>
          </div>
          ${linkHtml}
        </div>
      `;
    }).join('');
  } catch (e) {
    console.error('loadVodListForSession failed', e);
    listEl.innerHTML = `<div class="text-red-400 text-xs">VODs konnten nicht geladen werden.</div>`;
  }
}

async function prepareVodModal() {
  const openBtn = document.getElementById('openVodModalBtn');
  const closeBtn = document.getElementById('closeVodModalBtn');
  const modal = document.getElementById('vodModal');
  const sel = document.getElementById('vodSessionSelect');
  const refresh = document.getElementById('vodRefreshBtn');
  if (!openBtn || !closeBtn || !modal || !sel || !refresh) return;

  if (!openBtn.dataset.boundVodOpen) {
    openBtn.dataset.boundVodOpen = '1';
    openBtn.addEventListener('click', async () => {
      modal.classList.remove('hidden');
      document.body.classList.add('modal-open');
      await loadVodSessions();
    });
  }

  if (!closeBtn.dataset.boundVodClose) {
    closeBtn.dataset.boundVodClose = '1';
    closeBtn.addEventListener('click', () => {
      modal.classList.add('hidden');
      document.body.classList.remove('modal-open');
    });
  }

  if (!modal.dataset.boundVodBackdrop) {
    modal.dataset.boundVodBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) {
        modal.classList.add('hidden');
        document.body.classList.remove('modal-open');
      }
    });
  }

  if (!sel.dataset.boundVodChange) {
    sel.dataset.boundVodChange = '1';
    sel.addEventListener('change', () => {
      loadVodListForSession(sel.value);
    });
  }

  if (!refresh.dataset.boundVodRefresh) {
    refresh.dataset.boundVodRefresh = '1';
    refresh.addEventListener('click', () => {
      loadVodListForSession(sel.value);
    });
  }
}

async function loadUsers() {
  const body = document.getElementById('userTableBody');
  if (!body) return;
  try {
    const res = await _fetchJson('/api/users', { method: 'GET' });
    const list = Array.isArray(res?.users) ? res.users : [];
    if (!list.length) {
      body.innerHTML = `<tr><td colspan="5" class="px-3 py-3 text-center text-gray-500">Keine Benutzer</td></tr>`;
      return;
    }
    body.innerHTML = list.map(u => {
      const created = u.created_at ? new Date(u.created_at * 1000).toLocaleString() : '';
      const delBtn = _currentUser && _currentUser.id === u.id
        ? '<span class="text-xs text-gray-500">–</span>'
        : `<button data-action="delete-user" data-user-id="${u.id}" class="px-2 py-1 rounded bg-red-600 text-white text-[11px] hover:bg-red-500">Löschen</button>`;
      return `
        <tr>
          <td class="px-3 py-2 text-xs text-gray-400">${u.id}</td>
          <td class="px-3 py-2">${escapeHtml(u.username)}</td>
          <td class="px-3 py-2 text-xs text-gray-300">${escapeHtml(u.role || '')}</td>
          <td class="px-3 py-2 text-xs text-gray-400">${escapeHtml(created)}</td>
          <td class="px-3 py-2 text-right">${delBtn}</td>
        </tr>
      `;
    }).join('');
  } catch (e) {
    console.error('loadUsers failed', e);
    body.innerHTML = `<tr><td colspan="5" class="px-3 py-3 text-center text-red-400">Fehler beim Laden</td></tr>`;
  }
}

// ---------------- Admin Settings (ENV/DB) ----------------
async function loadAdminSettings() {
  const status = document.getElementById('adminSettingsStatus');
  const envBox = document.getElementById('adminEnvList');
  try {
    const res = await _fetchJson('/api/admin/settings', { method: 'GET' });
    const setVal = (id, v) => {
      const el = document.getElementById(id);
      if (!el) return;
      if (el.type === 'checkbox') {
        el.checked = Boolean(v);
      } else {
        el.value = v != null ? v : '';
      }
    };
    const setDefaultIfEmpty = (id, v) => {
      const el = document.getElementById(id);
      if (!el || el.type === 'checkbox') return;
      if (!el.value) el.value = v;
    };
    const setDefaultCheckbox = (id, value, resVal) => {
      const el = document.getElementById(id);
      if (!el || el.type !== 'checkbox') return;
      if (resVal == null) el.checked = value;
    };
    setVal('admOutputRtmp', res.output_rtmp || '');
    setVal('admAudioDevice', res.audio_device || '');
    setVal('admRecordEnabled', res.record_enabled);
    setVal('admRecordDir', res.record_dir || '');
    setVal('admRecordPrefix', res.record_prefix || '');
    setVal('admRecordFormat', res.record_format || '');
    setVal('admRecordSegment', res.record_segment_secs || '');
    setVal('admRecordMaxFiles', res.record_max_files || '');
    setVal('admLogRetentionDays', res.log_retention_days || '');

    setDefaultIfEmpty('admRecordPrefix', 'rec');
    setDefaultIfEmpty('admRecordFormat', 'mkv');
    setDefaultIfEmpty('admRecordSegment', '300');
    setDefaultIfEmpty('admRecordMaxFiles', '0');
    setDefaultIfEmpty('admLogRetentionDays', '7');
    setDefaultCheckbox('admRecordEnabled', true, res.record_enabled);
    if (envBox) {
      const env = Array.isArray(res.env) ? res.env : [];
      envBox.innerHTML = env.length
        ? env.map((e) => `<div class="flex justify-between gap-2"><span class="text-gray-400">${escapeHtml(e.key)}</span><span class="font-mono text-gray-200">${escapeHtml(e.value)}</span></div>`).join('')
        : '<div class="text-gray-500">Keine ENV gefunden.</div>';
    }
    if (status) status.textContent = 'Geladen';
  } catch (e) {
    console.error('loadAdminSettings failed', e);
    if (status) status.textContent = 'Fehler beim Laden';
    showToast('Admin-Settings laden fehlgeschlagen ❌', false);
  }
}

async function saveAdminSettings() {
  const status = document.getElementById('adminSettingsStatus');
  const numVal = (id) => {
    const el = document.getElementById(id);
    if (!el) return undefined;
    const n = Number(el.value);
    return Number.isFinite(n) ? n : undefined;
  };
  const strVal = (id) => {
    const el = document.getElementById(id);
    return el ? el.value.trim() : undefined;
  };
  const boolVal = (id) => {
    const el = document.getElementById(id);
    return el ? el.checked : false;
  };

  const payload = {
    output_rtmp: strVal('admOutputRtmp'),
    audio_device: strVal('admAudioDevice'),
    record_enabled: boolVal('admRecordEnabled'),
    record_dir: strVal('admRecordDir'),
    record_prefix: strVal('admRecordPrefix'),
    record_format: strVal('admRecordFormat'),
    record_segment_secs: numVal('admRecordSegment'),
    record_max_files: numVal('admRecordMaxFiles'),
    log_retention_days: numVal('admLogRetentionDays'),
  };

  try {
    await _fetchJson('/api/admin/settings', { method: 'POST', body: payload });
    if (status) status.textContent = 'Gespeichert – Pipeline wird neu geladen';
    showToast('Admin-Settings gespeichert ✅', true);
  } catch (e) {
    console.error('saveAdminSettings failed', e);
    if (status) status.textContent = 'Fehler beim Speichern';
    showToast('Admin-Settings speichern fehlgeschlagen ❌', false);
  }
}

async function cleanupLogFiles() {
  const daysEl = document.getElementById('admLogRetentionDays');
  const status = document.getElementById('adminLogsCleanupStatus');
  const days = Number(daysEl?.value ?? 7);
  if (!Number.isFinite(days) || days < 0) {
    if (status) status.textContent = 'Bitte gueltige Tage angeben.';
    return;
  }
  if (!confirm(`Logfiles aelter als ${days} Tage jetzt loeschen?`)) return;
  if (status) status.textContent = 'Loesche...';
  try {
    const res = await _fetchJson('/api/admin/logs/cleanup', {
      method: 'POST',
      body: { keep_days: Math.floor(days) },
    });
    const deleted = Number(res?.deleted_files || 0);
    const bytes = Number(res?.deleted_bytes || 0);
    const mb = (bytes / (1024 * 1024)).toFixed(1);
    if (status) status.textContent = `${deleted} Dateien geloescht (${mb} MB)`;
    showToast('Logfiles aufgeraeumt ✅', true);
  } catch (e) {
    console.error('cleanup logs failed', e);
    if (status) status.textContent = 'Aufraeumen fehlgeschlagen';
    showToast('Logfiles konnten nicht aufgeraeumt werden ❌', false);
  }
}

// ---------------- Admin Update (upload/install) ----------------
async function loadUpdateFiles() {
  const box = document.getElementById('adminUpdateListBox');
  const status = document.getElementById('adminUpdateStatus');
  if (box) box.textContent = 'Lade Dateien…';
  try {
    const res = await _fetchJson('/api/admin/update/list', { method: 'GET' });
    const files = Array.isArray(res.files) ? res.files : [];
    if (!box) return;
    const keyHint = Number(res.keys) > 0 ? ''
      : '<div class="text-amber-400 text-xs">Kein Update-Schlüssel auf dem Server hinterlegt – Installation aus der Oberfläche nicht möglich (siehe Hilfe → Update).</div>';
    if (!files.length) {
      box.innerHTML = keyHint + '<div class="text-gray-500 text-xs">Keine Updates hochgeladen.</div>';
    } else {
      box.innerHTML = files.map(f => {
        const sizeMb = (Number(f.size) || 0) / (1024 * 1024);
        return `<div class="flex items-center justify-between bg-gray-900/50 border border-gray-800 rounded-lg px-3 py-2">
          <div>
            <div class="font-mono text-xs">${escapeHtml(f.name)}</div>
            <div class="text-[11px] text-gray-400">${sizeMb.toFixed(1)} MB · ${f.signed ? '🔏 signiert' : '<span class="text-amber-400">Signatur fehlt (.sig hochladen)</span>'}</div>
          </div>
          <button class="btn btn-xs bg-green-600 text-white border-none" data-action="install-update" data-path="${escapeHtml(f.path)}" ${f.signed && Number(res.keys) > 0 ? '' : 'disabled'}>Installieren</button>
        </div>`;
      }).join('');
      box.innerHTML = keyHint + box.innerHTML;
    }
    if (status) status.textContent = '';
  } catch (e) {
    console.error('loadUpdateFiles failed', e);
    if (box) box.innerHTML = '<div class="text-red-400 text-xs">Fehler beim Laden.</div>';
    if (status) status.textContent = 'Fehler beim Laden';
  }
}

async function uploadUpdateFile() {
  const input = document.getElementById('adminUpdateFile');
  const status = document.getElementById('adminUpdateStatus');
  const files = input && input.files ? [...input.files] : [];
  if (!files.length) {
    if (status) status.textContent = 'Bitte .deb und .deb.sig auswählen.';
    return;
  }
  if (files.some((f) => !/\.deb(\.sig)?$/.test(f.name))) {
    if (status) status.textContent = 'Nur .deb und .deb.sig erlaubt.';
    return;
  }
  if (status) status.textContent = 'Lade hoch…';
  try {
    for (const file of files) {
      const fd = new FormData();
      fd.append('file', file);
      const res = await authFetch('/api/admin/update/upload', {
        method: 'POST',
        headers: withAuthHeaders({}),
        body: fd,
      });
      if (!res.ok) throw new Error('upload failed: ' + file.name);
    }
    if (status) status.textContent = 'Upload ok';
    showToast('Update hochgeladen ✅', true);
    input.value = '';
    loadUpdateFiles();
  } catch (e) {
    console.error('uploadUpdateFile failed', e);
    if (status) status.textContent = 'Fehler beim Upload';
    showToast('Upload fehlgeschlagen ❌', false);
  }
}

async function installUpdate(path) {
  const status = document.getElementById('adminUpdateStatus');
  if (!path) return;
  if (!confirm('Update installieren? Der Dienst startet dabei neu; eine laufende Session wird beendet.')) return;
  if (status) status.textContent = 'Starte Update…';
  try {
    const res = await _fetchJson('/api/admin/update/install', { method: 'POST', body: { path } });
    if (res.ok === false) {
      if (status) status.textContent = res.error || 'Fehler';
      showToast('Update fehlgeschlagen ❌', false);
      return;
    }
    pollUpdate();
  } catch (e) {
    console.error('installUpdate failed', e);
    if (status) status.textContent = 'Installationsfehler';
    showToast('Update fehlgeschlagen ❌', false);
  }
}

// Update läuft als eigene systemd-Unit; Log abfragen, auch über den Dienst-Neustart hinweg
let updatePollTimer = null;
function pollUpdate() {
  const status = document.getElementById('adminUpdateStatus');
  const logEl = document.getElementById('adminUpdateLog');
  const started = Date.now();
  let sawRunning = false;
  clearTimeout(updatePollTimer);
  const tick = async () => {
    let st = null;
    try { st = await _fetchJson('/api/admin/update/status'); } catch { /* Dienst startet neu */ }
    if (st) {
      if (logEl) {
        logEl.classList.remove('hidden');
        logEl.textContent = st.log || '';
        logEl.scrollTop = logEl.scrollHeight;
      }
      if (st.running) sawRunning = true;
      if (!st.running && (sawRunning || Date.now() - started > 5000)) {
        const failed = /^E: |dpkg: error|Fehler/m.test(st.log || '');
        if (status) status.textContent = failed ? 'Update fehlgeschlagen – siehe Log' : `Fertig – Version ${st.version}`;
        showToast(failed ? 'Update fehlgeschlagen ❌' : 'Update installiert ✅', !failed);
        loadUpdateFiles();
        return;
      }
      if (status) status.textContent = 'Update läuft…';
    } else if (status) {
      status.textContent = 'Dienst startet neu…';
    }
    if (Date.now() - started < 15 * 60 * 1000) updatePollTimer = setTimeout(tick, 2000);
  };
  tick();
}

async function createUser() {
  const nameEl = document.getElementById('userNewName');
  const passEl = document.getElementById('userNewPass');
  const roleEl = document.getElementById('userNewRole');
  const statusEl = document.getElementById('userStatus');
  if (!nameEl || !passEl || !roleEl) return;
  const username = nameEl.value.trim();
  const password = passEl.value;
  const role = (roleEl.value || 'user').trim() || 'user';
  if (!username || !password) {
    if (statusEl) statusEl.textContent = 'Bitte Benutzername/Passwort ausfüllen.';
    return;
  }
  try {
    await _fetchJson('/api/users', {
      method: 'POST',
      body: { username, password, role },
    });
    if (statusEl) statusEl.textContent = 'Benutzer angelegt.';
    nameEl.value = '';
    passEl.value = '';
    loadUsers();
    showToast('Benutzer angelegt ✅', true);
  } catch (e) {
    console.error('createUser failed', e);
    if (statusEl) statusEl.textContent = 'Fehler beim Anlegen.';
    showToast('Benutzer anlegen fehlgeschlagen ❌', false);
  }
}

async function deleteUser(id) {
  if (!id) return;
  const statusEl = document.getElementById('userStatus');
  try {
    await _fetchJson(`/api/users/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (statusEl) statusEl.textContent = 'Benutzer gelöscht.';
    loadUsers();
    showToast('Benutzer gelöscht ✅', true);
  } catch (e) {
    console.error('deleteUser failed', e);
    if (statusEl) statusEl.textContent = 'Fehler beim Löschen.';
    showToast('Benutzer löschen fehlgeschlagen ❌', false);
  }
}


// Rebuild
document.getElementById('rebuildBtn').addEventListener('click', async () => {
  try {
    await authFetch('/api/rebuild', { method: 'POST', headers: withAuthHeaders({}) });
  } catch (_) {}
});

function numOrNull(v){ const n = Number(v); return Number.isFinite(n) ? n : null; }

function escapeHtml(str) {
  if (str === null || str === undefined) return '';
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

async function _fetchJson(url, opts) {
  opts = opts || {};
  const method = opts.method || 'GET';
  const body   = opts.body !== undefined ? opts.body : undefined;

  const fetchOpts = { method, headers: withAuthHeaders({}) };

  if (body !== undefined) {
    fetchOpts.headers['Content-Type'] = 'application/json';
    fetchOpts.body = JSON.stringify(body);
  }

  const res = await authFetch(url, fetchOpts);

  if (!res.ok) {
    let text;
    try { text = await res.text(); } catch (_) { text = ''; }
    const err = new Error(`HTTP ${res.status} for ${url}`);
    err.status = res.status;
    err.body = text;
    throw err;
  }

  try {
    return await res.json();
  } catch {
    return {};
  }
}
 // ======== SOURCES / KAMERAS ======================================
// ======== NEW CAMERA DIALOG ==================================

(function () {
  const addBtn     = document.getElementById('addCameraBtn');
  const section    = document.getElementById('newCameraSection');
  const form       = document.getElementById('newCameraForm');
  const cancelBtn  = document.getElementById('cancelNewCameraBtn');
  const urlInput   = document.getElementById('sourceUrlInput');
  const mjpegInput = document.getElementById('sourceMjpegInput');

  if (!addBtn || !section || !form) return;

  if (urlInput) {
    urlInput.addEventListener('input', () => {
      updateMjpegPlaceholder(urlInput, mjpegInput);
    });
  }

  // --- Button öffnet Formular ---
  addBtn.addEventListener('click', () => {
    section.classList.remove('hidden');
    document.getElementById('sourceEditForm').classList.add('hidden');
    form.reset();
    document.getElementById('newCamEnabled').checked = true;
    document.getElementById('newCamPtzFields')?.classList.add('hidden');
  });

  // --- PTZ-Felder ein-/ausblenden ---
  document.getElementById('newCamPtz')?.addEventListener('change', (e) => {
    document.getElementById('newCamPtzFields')?.classList.toggle('hidden', !e.target.checked);
  });

  // --- Abbrechen ---
  cancelBtn.addEventListener('click', () => {
    section.classList.add('hidden');
    document.getElementById('sourceEditForm').classList.remove('hidden');
  });

  // --- Neue Kamera speichern ---
  form.addEventListener('submit', async (e) => {
    e.preventDefault();

    const name    = document.getElementById('newCamName').value.trim();
    const url     = document.getElementById('newCamUrl').value.trim();
    const mjpegUrl = document.getElementById('newCamMjpegUrl')?.value.trim() || '';
    const enabled = document.getElementById('newCamEnabled').checked;
    const brand   = document.getElementById('newCamBrand')?.value || 'generic';
    const ptzCapable = !!document.getElementById('newCamPtz')?.checked;
    const vapixUser = document.getElementById('newCamVapixUser')?.value.trim() || '';
    const vapixPass = document.getElementById('newCamVapixPass')?.value.trim() || '';
    const vapixPortRaw = document.getElementById('newCamVapixPort')?.value.trim() || '';
    const vapixPort = vapixPortRaw ? Number(vapixPortRaw) : null;

    if (!name || !url) {
      showToast("Name und URL sind Pflichtfelder.", false);
      return;
    }

    try {
      const created = await _fetchJson('/api/cameras', {
        method: 'POST',
        body: {
          name, url, mjpeg_url: mjpegUrl, enabled,
          brand, ptz_capable: ptzCapable,
          vapix_username: vapixUser || null,
          vapix_password: vapixPass || null,
          vapix_port: vapixPort,
        }
      });

      showToast("Kamera hinzugefügt ✅", true);

      // Zur Liste hinzufügen und Tabelle neu rendern
      _sourcesCache.push(created);
      renderSourcesTable(true);

      // Formular schließen
      section.classList.add('hidden');
      document.getElementById('sourceEditForm').classList.remove('hidden');

    } catch (err) {
      console.error("camera create failed", err);
      showToast("Kamera konnte nicht angelegt werden ❌", false);
    }
  });
})();

// MJPEG Placeholder für New Camera
(function () {
  const newUrlInput = document.getElementById('newCamUrl');
  const newMjpegInput = document.getElementById('newCamMjpegUrl');
  if (!newUrlInput || !newMjpegInput) return;
  newUrlInput.addEventListener('input', () => {
    updateMjpegPlaceholder(newUrlInput, newMjpegInput);
  });
})();


// Wir verwenden hier die Kamera-DB als Quelle.
let _sourcesCache = []; // { id, name, url, fov_deg, enabled, created_at }

async function loadSourcesSettings() {
  try {
    const res = await _fetchJson('/api/cameras', { method: 'GET' });
    const list = Array.isArray(res?.cameras) ? res.cameras : [];
    // kleine Normalisierung
    return list.map(cam => ({
      id: cam.id,
      name: cam.name || `Kamera #${cam.id}`,
      url: cam.url || '',
      mjpeg_url: cam.mjpeg_url || '',
      fov_deg: cam.fov_deg ?? null,
      enabled: cam.enabled ? !!cam.enabled : false,
      created_at: cam.created_at,
      brand: cam.brand || 'generic',
      ptz_capable: !!cam.ptz_capable,
      vapix_port: cam.vapix_port ?? null,
    }));
  } catch (e) {
    console.error('loadSourcesSettings failed', e);
    return [];
  }
}




function sourceRowHtml(cam) {
  const shortUrl = cam.url
    ? (cam.url.length > 80 ? cam.url.slice(0, 77) + '…' : cam.url)
    : 'keine URL (deaktiviert)';
  const status = cam.enabled
    ? '<span class="ors-pill ors-pill-ok">● aktiv</span>'
    : '<span class="ors-pill ors-pill-off">○ deaktiviert</span>';

  return `
    <tr class="${cam.enabled ? '' : 'opacity-60'}">
      <td class="px-3 py-2"><span class="ors-pill ors-pill-id">#${cam.id}</span></td>
      <td class="px-3 py-2 text-sm font-semibold text-gray-100">${escapeHtml(cam.name || '<ohne Namen>')}</td>
      <td class="px-3 py-2 ors-mono max-w-[16rem] truncate" title="${escapeHtml(cam.url || '')}">${escapeHtml(shortUrl)}</td>
      <td class="px-3 py-2">${status}</td>
      <td class="px-3 py-2 text-right">
        <button data-action="edit-source" data-id="${cam.id}" class="ors-btn-ghost">✏️ Bearbeiten</button>
      </td>
    </tr>
  `;
}

function deriveMjpegUrl(rawUrl) {
  if (!rawUrl) return '';
  try {
    const u = new URL(rawUrl);
    if (!u.hostname) return '';
    const auth = u.username
      ? `${u.username}${u.password ? ':' + u.password : ''}@`
      : '';
    const port = u.port && u.port !== '554' ? `:${u.port}` : '';
    return `http://${auth}${u.hostname}${port}/axis-cgi/mjpg/video.cgi`;
  } catch {
    return '';
  }
}

function updateMjpegPlaceholder(urlInput, mjpegInput) {
  if (!mjpegInput) return;
  const derived = deriveMjpegUrl(urlInput?.value?.trim() || '');
  if (derived) {
    mjpegInput.placeholder = derived;
  }
}

function updatePtzFieldsVisibility(checkboxEl, fieldsEl, hintEl) {
  const show = !!checkboxEl?.checked;
  fieldsEl?.classList.toggle('hidden', !show);
  hintEl?.classList.toggle('hidden', !show);
}

(function bindSourcePtzToggle() {
  const cb = document.getElementById('sourcePtzInput');
  if (!cb) return;
  cb.addEventListener('change', () => {
    updatePtzFieldsVisibility(cb, document.getElementById('sourcePtzFields'), document.getElementById('sourcePtzHint'));
  });
})();

function fillSourceForm(cam) {
  const idInput   = document.getElementById('sourceSlotInput');   // Kamera-ID (readonly)
  const urlInput  = document.getElementById('sourceUrlInput');
  const mjpegInput = document.getElementById('sourceMjpegInput');
  const nameInput = document.getElementById('sourceNameInput');
  const enInput   = document.getElementById('sourceEnabledInput');
  const brandInput = document.getElementById('sourceBrandInput');
  const ptzInput  = document.getElementById('sourcePtzInput');
  const portInput = document.getElementById('sourceVapixPortInput');
  const userInput = document.getElementById('sourceVapixUserInput');
  const passInput = document.getElementById('sourceVapixPassInput');
  resetCameraPreview();

  if (!idInput || !urlInput) return;

  idInput.value    = cam?.id != null ? String(cam.id) : '';
  urlInput.value   = cam?.url || '';
  if (mjpegInput) mjpegInput.value = cam?.mjpeg_url || '';
  if (nameInput)   nameInput.value   = cam?.name || '';
  if (enInput)     enInput.checked   = !!cam?.enabled;
  if (brandInput)  brandInput.value  = cam?.brand || 'generic';
  if (ptzInput)    ptzInput.checked  = !!cam?.ptz_capable;
  if (portInput)   portInput.value   = cam?.vapix_port ?? '';
  // Zugangsdaten liefert die API nie zurück (Sicherheit) — Felder bleiben leer,
  // leer lassen beim Speichern = unverändert.
  if (userInput)   userInput.value   = '';
  if (passInput)   passInput.value   = '';
  updatePtzFieldsVisibility(ptzInput, document.getElementById('sourcePtzFields'), document.getElementById('sourcePtzHint'));
  updateMjpegPlaceholder(urlInput, mjpegInput);
}

function getSourceFromCache(id) {
  return _sourcesCache.find(c => String(c.id) === String(id)) || { id, name: '', url: '', mjpeg_url: '', enabled: false, brand: 'generic', ptz_capable: false, vapix_port: null };
}

// ======== CAMERA SNAPSHOTS (Preview) =============================
let _currentPreviewObjectUrl = null;

function setPreviewState({ msg, loading = false, hasImg = false }) {
  const status = document.getElementById('cameraPreviewStatus');
  const placeholder = document.getElementById('cameraPreviewPlaceholder');
  const img = document.getElementById('cameraPreviewImg');

  if (status) status.textContent = msg || '';
  if (placeholder) placeholder.classList.toggle('hidden', loading || hasImg);
  if (img) img.classList.toggle('hidden', !hasImg);
}

async function loadCameraSnapshot(force = false) {
  const idStr = document.getElementById('sourceSlotInput')?.value;
  const id = Number(idStr);
  if (!id || Number.isNaN(id)) {
    setPreviewState({ msg: 'Keine Kamera ausgewählt', hasImg: false });
    const img = document.getElementById('cameraPreviewImg');
    if (img) img.src = '';
    return;
  }

  setPreviewState({ msg: force ? 'Lade Snapshot (erzwinge neu)…' : 'Lade Snapshot…', loading: true });

  try {
    const url = force
      ? `/api/cameras/${encodeURIComponent(id)}/snapshot?fresh=true`
      : `/api/cameras/${encodeURIComponent(id)}/snapshot`;

    const res = await authFetch(url, { method: 'GET' });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);

    const blob = await res.blob();
    if (_currentPreviewObjectUrl) {
      URL.revokeObjectURL(_currentPreviewObjectUrl);
    }
    _currentPreviewObjectUrl = URL.createObjectURL(blob);

    const img = document.getElementById('cameraPreviewImg');
    if (img) img.src = _currentPreviewObjectUrl;
    setPreviewState({ msg: 'Snapshot geladen', hasImg: true });
  } catch (e) {
    console.warn('loadCameraSnapshot failed', e);
    setPreviewState({ msg: 'Kein Snapshot verfügbar', hasImg: false });
    const img = document.getElementById('cameraPreviewImg');
    if (img) img.src = '';
  }
}

function resetCameraPreview() {
  if (_currentPreviewObjectUrl) {
    URL.revokeObjectURL(_currentPreviewObjectUrl);
    _currentPreviewObjectUrl = null;
  }
  setPreviewState({ msg: '', hasImg: false });
  const img = document.getElementById('cameraPreviewImg');
  if (img) img.src = '';
}

function openSourcesModal() {
  if (!_currentUser || (_currentUser.role || '').toLowerCase() !== 'admin') {
    if (window.showToast) showToast('Nur Admins dürfen Kameras verwalten.', false);
    return;
  }
  const m = document.querySelector('#sourcesModal');
  if (!m) {
    console.warn('sourcesModal element not found');
    return;
  }
  m.classList.remove('hidden');
  document.documentElement.classList.add('overflow-hidden');
   document.body.classList.add('modal-open');
  renderSourcesTable(true);
}

function closeSourcesModal() {
  const m = document.querySelector('#sourcesModal');
  if (!m) return;
  m.classList.add('hidden');
  document.documentElement.classList.remove('overflow-hidden');
  document.body.classList.remove('modal-open');
}

async function renderSourcesTable(force = false) {
  const tbody   = document.getElementById('sourcesTableBody');
  const summary = document.getElementById('sourcesSummary');
  if (!tbody) return;

  if (force || !_sourcesCache.length) {
    tbody.innerHTML = `<tr><td colspan="4" class="px-3 py-3 text-sm text-gray-400">Lade Quellen…</td></tr>`;
    try {
      _sourcesCache = await loadSourcesSettings();
    } catch (e) {
      console.error('loadSourcesSettings failed', e);
      tbody.innerHTML = `<tr><td colspan="4" class="px-3 py-3 text-sm text-red-400">Fehler beim Laden der Quellen.</td></tr>`;
      return;
    }
  }

  if (!_sourcesCache.length) {
    tbody.innerHTML = `<tr><td colspan="4" class="px-3 py-3 text-sm text-gray-400">Keine Quellen konfiguriert.</td></tr>`;
  } else {
    tbody.innerHTML = _sourcesCache.map(sourceRowHtml).join('');
  }

  if (summary) {
    const configured = _sourcesCache.filter(s => s.url && s.url.trim() !== '').length;
    summary.textContent = `${configured} von ${_sourcesCache.length} Slots konfiguriert.`;
  }
}

async function saveSourceFromForm() {
  const idStr     = document.getElementById('sourceSlotInput').value;
  const id        = Number(idStr);
  const url       = document.getElementById('sourceUrlInput').value.trim();
  const mjpegUrl  = document.getElementById('sourceMjpegInput')?.value.trim() || '';
  const nameEl    = document.getElementById('sourceNameInput');
  const enabledEl = document.getElementById('sourceEnabledInput');

  if (!id || Number.isNaN(id)) {
    showToast('Keine gültige Kamera-ID ausgewählt.', false);
    return;
  }

  const name    = nameEl ? nameEl.value.trim() : null;
  const enabled = enabledEl ? !!enabledEl.checked : true;
  const brand   = document.getElementById('sourceBrandInput')?.value || 'generic';
  const ptzCapable = !!document.getElementById('sourcePtzInput')?.checked;
  const vapixUser = document.getElementById('sourceVapixUserInput')?.value.trim() || '';
  const vapixPass = document.getElementById('sourceVapixPassInput')?.value.trim() || '';
  const vapixPortRaw = document.getElementById('sourceVapixPortInput')?.value.trim() || '';
  const vapixPort = vapixPortRaw ? Number(vapixPortRaw) : null;

  const body = {
    ...(name !== null ? { name } : {}),
    url,
    mjpeg_url: mjpegUrl,
    enabled,
    brand,
    ptz_capable: ptzCapable,
    vapix_port: vapixPort,
    // Leer = unverändert lassen (Backend behält vorhandene Zugangsdaten bei).
    ...(vapixUser ? { vapix_username: vapixUser } : {}),
    ...(vapixPass ? { vapix_password: vapixPass } : {}),
  };

  try {
    await _fetchJson(`/api/cameras/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body,
    });

    const idx = _sourcesCache.findIndex(c => c.id === id);
    if (idx >= 0) {
      _sourcesCache[idx] = { ..._sourcesCache[idx], ...body };
    }

    if (window.showToast) showToast('Kamera gespeichert ✅', true);
    renderSourcesTable(false);
    await loadCameraSnapshot(false);
  } catch (e) {
    console.error('saveSourceFromForm failed', e);
    if (window.showToast) showToast('Speichern fehlgeschlagen ❌', false);
  }
}



async function clearSourceSlot() {
  const idStr = document.getElementById('sourceSlotInput').value;
  const id = Number(idStr);
  if (!id || Number.isNaN(id)) return;

  if (!confirm('Diese Kamera wirklich deaktivieren?')) return;

  try {
    await _fetchJson(`/api/cameras/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: { enabled: false, url: '' },
    });

    const idx = _sourcesCache.findIndex(c => c.id === id);
    if (idx >= 0) {
      _sourcesCache[idx].enabled = false;
      _sourcesCache[idx].url = '';
      _sourcesCache[idx].mjpeg_url = '';
    }

    if (window.showToast) showToast('Kamera deaktiviert ✅', true);
    renderSourcesTable(false);
    const cam = _sourcesCache.find(c => c.id === id) || { id, name: '', url: '', mjpeg_url: '', enabled: false };
    fillSourceForm(cam);
  } catch (e) {
    console.error('clearSourceSlot failed', e);
    if (window.showToast) showToast('Deaktivieren fehlgeschlagen ❌', false);
  }
}



// ======== MARKER TYPES: API ======================================

let _markerTypesCache = [];

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
  document.body.classList.add('modal-open');
  renderMarkerTypesList(true);
}

function closeMarkerTypesModal() {
  const m = document.querySelector('#markerTypesModal');
  if (!m) return;
  m.classList.add('hidden');
  document.documentElement.classList.remove('overflow-hidden');
  document.body.classList.remove('modal-open');
}

function markerTypeRowHtml(t) {
  return `
    <div class="p-4 flex items-center gap-3">
      <div class="w-6 h-6 rounded border border-gray-700"
           style="background:${t.color || '#4b5563'}"></div>
      <div class="flex-1">
        <div class="text-white font-medium">${escapeHtml(t.name)}</div>
        <div class="text-xs text-gray-400">ID: ${t.id} • ${escapeHtml(t.color)}</div>
      </div>
      <div class="flex gap-2">
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

  if (!_markerTypesCache.length) {
    list.innerHTML = `<div class="p-4 text-gray-400">Keine Marker-Typen definiert.</div>`;
  } else {
    list.innerHTML = _markerTypesCache.map(markerTypeRowHtml).join('');
  }

  list.querySelectorAll('[data-action="edit-marker-type"]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.id;
      const t = _markerTypesCache.find(x => String(x.id) === String(id));
      if (!t) return;
      const name = prompt('Name des Marker-Typs:', t.name || '');
      if (name == null) return;
      const color = prompt('Farbe (CSS, z.B. #3b82f6):', t.color || '#3b82f6');
      if (color == null) return;
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

// Bindings

(function bindMarkerTypesUIOnce() {
  const btn = document.querySelector('#markerTypesBtn');
  const close = document.querySelector('#closeMarkerTypesModal');
  const addBtn = document.querySelector('#addMarkerTypeBtn');
  const modal = document.querySelector('#markerTypesModal');

  if (btn && !btn.dataset.boundMarkerTypesOpen) {
    btn.dataset.boundMarkerTypesOpen = '1';
    btn.addEventListener('click', (e) => { e.preventDefault(); openMarkerTypesModal(); });
  }
  if (close && !close.dataset.boundMarkerTypesClose) {
    close.dataset.boundMarkerTypesClose = '1';
    close.addEventListener('click', (e) => { e.preventDefault(); closeMarkerTypesModal(); });
  }
  if (addBtn && !addBtn.dataset.boundMarkerTypesAdd) {
    addBtn.dataset.boundMarkerTypesAdd = '1';
    addBtn.addEventListener('click', async () => {
      const name = prompt('Name des neuen Marker-Typs:', 'Neuer Typ');
      if (!name) return;
      const color = prompt('Farbe (CSS, z.B. #3b82f6):', '#3b82f6') || '#3b82f6';
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

  if (modal && !modal.dataset.boundMarkerTypesBackdrop) {
    modal.dataset.boundMarkerTypesBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) closeMarkerTypesModal();
    });
  }

  window.addEventListener('keydown', (ev) => {
    const m = document.querySelector('#markerTypesModal');
    if (!m || m.classList.contains('hidden')) return;
    if (ev.key === 'Escape') closeMarkerTypesModal();
  });

  // ======== SOURCES: EVENT BINDING =================================

(function () {
  const openBtn  = document.getElementById('openSourcesModalBtn');
  const closeBtn = document.getElementById('closeSourcesModalBtn');
  const modal    = document.getElementById('sourcesModal');
  const tbody    = document.getElementById('sourcesTableBody');
  const form     = document.getElementById('sourceEditForm');
  const delBtn   = document.getElementById('sourceDeleteBtn');

  if (openBtn && !openBtn.dataset.boundSourcesOpen) {
    openBtn.dataset.boundSourcesOpen = '1';
    openBtn.addEventListener('click', (e) => { e.preventDefault(); openSourcesModal(); });
  }

  if (closeBtn && !closeBtn.dataset.boundSourcesClose) {
    closeBtn.dataset.boundSourcesClose = '1';
    closeBtn.addEventListener('click', (e) => { e.preventDefault(); closeSourcesModal(); });
  }

  if (modal && !modal.dataset.boundSourcesBackdrop) {
    modal.dataset.boundSourcesBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) closeSourcesModal();
    });
  }

  if (tbody && !tbody.dataset.boundSourcesTable) {
  tbody.dataset.boundSourcesTable = '1';
  tbody.addEventListener('click', (e) => {
    const btn = e.target.closest('button[data-action="edit-source"]');
    if (!btn) return;
    const id = Number(btn.dataset.id || '0');
    if (!id) return;
    const cam = getSourceFromCache(id);
    fillSourceForm(cam);
    loadCameraSnapshot(false);
  });
}



  if (form && !form.dataset.boundSourcesForm) {
    form.dataset.boundSourcesForm = '1';
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      await saveSourceFromForm();
    });
  }

  if (delBtn && !delBtn.dataset.boundSourcesDelete) {
    delBtn.dataset.boundSourcesDelete = '1';
    delBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      await clearSourceSlot();
    });
  }

  const previewBtn = document.getElementById('cameraPreviewRefresh');
  if (previewBtn && !previewBtn.dataset.boundPreviewRefresh) {
    previewBtn.dataset.boundPreviewRefresh = '1';
    previewBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      await loadCameraSnapshot(true);
    });
  }

  // ESC im Modal
  window.addEventListener('keydown', (ev) => {
    const m = document.querySelector('#sourcesModal');
    if (!m || m.classList.contains('hidden')) return;
    if (ev.key === 'Escape') closeSourcesModal();
  });
})();

})();

// ======== ROOMS: LAYOUT-PRESETS & STATE ==========================

const ROOM_LAYOUT_PRESETS = [
  { code: '1',     label: 'Single (1)',                   rows: 1, cols: 1 },
  { code: '1+3',   label: '1 groß + 3 Thumbs',            rows: 2, cols: 2 },
  { code: '1+5',   label: '1 groß + 5 klein',             rows: 2, cols: 3 },
  { code: '3pip',  label: '3 (PIP rechts unten)',         rows: 2, cols: 2 },
  { code: '3diag', label: '3 diagonal/überlappend',       rows: 3, cols: 1 },
  { code: '2x2',   label: '2 × 2',                        rows: 2, cols: 2 },
  { code: '3x3',   label: '3 × 3',                        rows: 3, cols: 3 },
  { code: '4x4',   label: '4 × 4',                        rows: 4, cols: 4 },
];

function roomLayoutByCode(code) {
  return ROOM_LAYOUT_PRESETS.find(p => p.code === code) || ROOM_LAYOUT_PRESETS[0];
}

let _roomsCache = [];
let _currentRoomSources = new Array(64).fill(null); // index = source.N
let _currentRoomAudioInputs = []; // [{device,label,gain}]
let _audioDefsCache = []; // globale Liste aus /api/audio/inputs
let _audioDevicesCache = [];
let _currentUser = null;

function normalizeAudioDeviceId(val) {
  const v = (val || '').trim();
  if (!v) return '';
  if (/^\\d+$/.test(v)) {
    return `hw:${v},0`;
  }
  const m = v.match(/^(hw|plughw):(\\d+)(?:,(\\d+))?$/i);
  if (m) {
    const card = m[2];
    const dev = m[3] || '0';
    return `${m[1].toLowerCase()}:${card},${dev}`;
  }
  return v;
}

// Kanalauswahl „1,2“ / „3-6“ / leer → Array 1-basierter Kanäle oder null (= alle)
function parseChannelsInput(str) {
  const t = String(str || '').trim();
  if (!t) return null;
  const out = new Set();
  t.split(/[,\s;]+/).forEach(part => {
    if (!part) return;
    const m = part.match(/^(\d+)\s*-\s*(\d+)$/);
    if (m) {
      const a = Number(m[1]), b = Number(m[2]);
      for (let i = Math.min(a, b); i <= Math.max(a, b); i++) if (i >= 1 && i <= 64) out.add(i);
    } else if (/^\d+$/.test(part)) {
      const n = Number(part);
      if (n >= 1 && n <= 64) out.add(n);
    }
  });
  return out.size ? Array.from(out) : null;
}
function formatChannels(arr) {
  return Array.isArray(arr) && arr.length ? arr.join(',') : '';
}
function deviceChannelCount(deviceId) {
  const d = (_audioDevicesCache || []).find(x => x.id === deviceId);
  return d && Number.isFinite(d.channels) ? d.channels : null;
}
function channelsHint(deviceId, channels) {
  const n = deviceChannelCount(deviceId);
  const sel = Array.isArray(channels) && channels.length ? channels : null;
  const mode = sel ? (sel.length === 2 ? 'Stereo' : 'Mono-Mix') : 'alle → Mono-Mix';
  return `${n ? n + ' Kanäle' : 'Kanäle ?'} · ${mode}`;
}

// ======== ROOMS: API =============================================

// Lädt die verfügbaren Audio-Input-Geräte vom Host-API
async function loadAudioInputDevices() {
  try {
    const res = await _fetchJson('/api/host/audio_inputs', { method: 'GET' });
    _audioDevicesCache = Array.isArray(res?.devices) ? res.devices : [];
    updateRoomAudioInputOptions();
  } catch (e) {
    console.warn('loadAudioInputDevices failed', e);
  }
}

// Aktualisiert die Datalist für Audio-Geräte (zur Autovervollständigung)
function updateRoomAudioInputOptions() {
  const listEl  = document.getElementById('audioDevicesList');
  if (!listEl) return;

  // Datalist leeren
  listEl.innerHTML = '';

  const optDefault = document.createElement('option');
  optDefault.value = '';
  optDefault.label = 'Default (globales audio.device)';
  listEl.appendChild(optDefault);

  _audioDevicesCache.forEach(dev => {
    const opt = document.createElement('option');
    opt.value = dev.id;
    const ch = Number.isFinite(dev.channels) ? ` · ${dev.channels} Kanäle` : '';
    opt.label = (dev.name ? `${dev.name} (${dev.id})` : dev.id) + ch;
    listEl.appendChild(opt);
  });
}

// Audio-Input-Definitionen (global)
async function loadAudioInputDefs() {
  try {
    const res = await _fetchJson('/api/audio/inputs', { method: 'GET' });
    _audioDefsCache = Array.isArray(res?.audio_inputs)
      ? res.audio_inputs.map(a => ({ ...a, device: normalizeAudioDeviceId(a.device || '') }))
      : [];
  } catch (e) {
    console.error('loadAudioInputDefs failed', e);
    _audioDefsCache = [];
  }
}

async function saveAudioInputDefs(list) {
  return await _fetchJson('/api/audio/inputs', {
    method: 'POST',
    body: { audio_inputs: list },
  });
}

function renderAudioDefsModal() {
  const list = document.getElementById('audioList');
  if (!list) return;
  if (!_audioDefsCache || !_audioDefsCache.length) {
    list.innerHTML = `<div class="text-sm text-gray-400">Keine Audio-Inputs definiert.</div>`;
    return;
  }
  list.innerHTML = _audioDefsCache
    .map((a, idx) => {
      const devVal = normalizeAudioDeviceId(a.device || '');
      return `
      <div class="flex items-center gap-2 text-xs" data-audio-def-idx="${idx}">
        <input type="text" list="audioDevicesList" value="${escapeHtml(devVal)}"
               class="flex-1 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
               placeholder="plughw:1,0" data-field="device" />
        <input type="text" value="${escapeHtml(a.label || '')}"
               class="w-32 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
               placeholder="Label" data-field="label" />
        <input type="number" step="0.1" min="0" value="${Number(a.gain ?? 1).toFixed(1)}"
               class="w-20 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
               placeholder="Gain" data-field="gain" />
        <input type="text" value="${escapeHtml(formatChannels(a.channels))}"
               class="w-24 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
               placeholder="Kanäle: alle" title="Hardware-Kanäle, z.B. 1,2 (Stereo) oder 3 (Mono) oder 1-8 (Mix). Leer = alle." data-field="channels" />
        <span class="text-[10px] text-gray-500 whitespace-nowrap">${escapeHtml(channelsHint(devVal, a.channels))}</span>
        <button type="button" data-action="remove-audio-def"
                class="px-2 py-1 rounded bg-red-600 text-white text-[11px] hover:bg-red-500">
          ✕
        </button>
      </div>
    `;
    })
    .join('');
}

function addAudioDef() {
  _audioDefsCache = _audioDefsCache || [];
  const nextId = (_audioDefsCache.map(a => a.id).reduce((m, v) => Math.max(m, Number(v) || 0), 0) || 0) + 1;
  _audioDefsCache.push({ id: nextId, device: '', label: '', gain: 1.0 });
  renderAudioDefsModal();
}

function removeAudioDef(idx) {
  if (!Array.isArray(_audioDefsCache)) return;
  _audioDefsCache.splice(idx, 1);
  renderAudioDefsModal();
}

function updateAudioDef(idx, field, value) {
  if (!Array.isArray(_audioDefsCache)) return;
  if (!_audioDefsCache[idx]) return;
  const def = _audioDefsCache[idx];
  const prevKey = `${def.device}|${def.label || ''}`;
  if (field === 'gain') {
    const num = Number(value);
    def.gain = Number.isFinite(num) ? num : 1.0;
  } else if (field === 'device') {
    def.device = normalizeAudioDeviceId(value);
  } else if (field === 'label') {
    def.label = value;
  } else if (field === 'channels') {
    def.channels = parseChannelsInput(value);
  }
  // Bereits im Raum ausgewählte Kopien dieses Eingangs mitziehen
  (_currentRoomAudioInputs || []).forEach(sel => {
    if (`${sel.device}|${sel.label || ''}` === prevKey) {
      sel.device = def.device;
      sel.label = def.label || '';
      sel.gain = def.gain != null ? def.gain : 1.0;
      sel.channels = def.channels || null;
    }
  });
  if (field === 'gain' || field === 'device') renderRoomAudioModal();
  renderRoomAudioInputs();
}

// --- Audio-Eingänge (Room) --------------------------------------
function parseRoomAudioString(str) {
  if (!str || typeof str !== 'string') return [];
  const t = str.trim();
  if (!t) return [];
  try {
    const arr = JSON.parse(t);
    if (Array.isArray(arr)) {
      return arr
        .filter(item => item && typeof item === 'object' && item.device)
        .map(item => ({
          device: normalizeAudioDeviceId(String(item.device || '')),
          label: item.label ? String(item.label) : '',
          gain: Number.isFinite(item.gain) ? Number(item.gain) : 1.0,
          channels: Array.isArray(item.channels) && item.channels.length ? item.channels.map(Number).filter(n => n >= 1) : null,
        }));
    }
  } catch (_) {
    // ignore
  }
  // Fallback: CSV/semicolon string
  return t.split(/[;,]/)
    .map(s => s.trim())
    .filter(Boolean)
    .map(device => ({ device: normalizeAudioDeviceId(device), label: '', gain: 1.0 }));
}

function renderRoomAudioInputs() {
  const list = document.getElementById('roomAudioList');
  const hidden = document.getElementById('roomAudioInput');
  if (!list) return;
  if (!Array.isArray(_currentRoomAudioInputs)) _currentRoomAudioInputs = [];
  if (!Array.isArray(_audioDefsCache)) _audioDefsCache = [];

  if (!_audioDefsCache.length) {
    list.innerHTML = `<div class="text-[11px] text-gray-500">Keine globalen Audio-Inputs definiert. Bitte Audio-Modal nutzen.</div>`;
  } else {
    list.innerHTML = _currentRoomAudioInputs.map(sel => {
      const label = sel.label || sel.device;
      const gain = sel.gain != null ? sel.gain : 1.0;
      const ch = Array.isArray(sel.channels) && sel.channels.length ? ` • Kanäle ${sel.channels.join(',')}` : ' • alle Kanäle';
      return `<div class="text-[11px] text-gray-300">${escapeHtml(label)} (${escapeHtml(sel.device)}) • Gain ${gain}${ch}</div>`;
    }).join('') || `<div class="text-[11px] text-gray-500">Keine Audio-Eingänge ausgewählt.</div>`;
  }

  if (hidden) {
    hidden.value = _currentRoomAudioInputs.length
      ? JSON.stringify(_currentRoomAudioInputs.map(sel => {
          const o = { device: sel.device, label: sel.label || '', gain: sel.gain != null ? sel.gain : 1.0 };
          if (Array.isArray(sel.channels) && sel.channels.length) o.channels = sel.channels;
          return o;
        }))
      : '';
  }
}

function renderRoomAudioModal() {
  const audioDefsList = document.getElementById('roomAudioDefsList');
  if (!audioDefsList) return;
  if (!Array.isArray(_audioDefsCache)) _audioDefsCache = [];
  const rows = _audioDefsCache.length
    ? _audioDefsCache.map((def, idx) => {
        const checked = _currentRoomAudioInputs.some(sel => sel.device === def.device && (sel.label || '') === (def.label || ''));
        const gain = def.gain != null ? def.gain : 1.0;
        const options = (_audioDevicesCache || []).map(dev => {
          const sel = dev.id === def.device ? 'selected' : '';
          const label = (dev.name ? `${dev.name} (${dev.id})` : dev.id) + (Number.isFinite(dev.channels) ? ` · ${dev.channels} Kan.` : '');
          return `<option value="${escapeHtml(dev.id)}" ${sel}>${escapeHtml(label)}</option>`;
        }).join('');
        return `
          <div class="flex items-center gap-2 text-xs bg-gray-800/60 border border-gray-700 rounded-lg px-3 py-2" data-audio-def-idx="${idx}">
            <input type="checkbox" data-audio-def-id="${def.id}" class="h-4 w-4" ${checked ? 'checked' : ''}>
            <select class="flex-1 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
                    data-field="device">
              <option value="">(kein Gerät)</option>
              ${options}
            </select>
            <input type="text" value="${escapeHtml(def.label || '')}"
                   class="w-32 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
                   placeholder="Label" data-field="label" />
            <input type="number" step="0.1" min="0" value="${Number(gain).toFixed(1)}"
                   class="w-20 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
                   placeholder="Gain" data-field="gain" />
            <input type="text" value="${escapeHtml(formatChannels(def.channels))}"
                   class="w-24 px-2 py-1 rounded bg-gray-800 border border-gray-700 text-gray-100"
                   placeholder="Kanäle: alle" title="Hardware-Kanäle, z.B. 1,2 (Stereo) oder 3 (Mono) oder 1-8 (Mix). Leer = alle." data-field="channels" />
            <span class="text-[10px] text-gray-500 whitespace-nowrap">${escapeHtml(channelsHint(def.device, def.channels))}</span>
            <button type="button" data-action="remove-audio-def"
                    class="px-2 py-1 rounded bg-red-600 text-white text-[11px] hover:bg-red-500">
              ✕
            </button>
          </div>
        `;
      }).join('')
    : `<div class="text-sm text-gray-400">Keine Audio-Inputs definiert.</div>`;
  audioDefsList.innerHTML = rows;
}

function addRoomAudioInput() {
  _currentRoomAudioInputs = _currentRoomAudioInputs || [];
  _currentRoomAudioInputs.push({ device: '', label: '', gain: 1.0 });
  renderRoomAudioInputs();
}

function removeRoomAudioInput(idx) {
  if (!Array.isArray(_currentRoomAudioInputs)) return;
  _currentRoomAudioInputs.splice(idx, 1);
  renderRoomAudioInputs();
}

function updateRoomAudioInput(idx, field, value) {
  if (!Array.isArray(_currentRoomAudioInputs)) return;
  if (!_currentRoomAudioInputs[idx]) return;
  if (field === 'gain') {
    const num = Number(value);
    _currentRoomAudioInputs[idx].gain = Number.isFinite(num) ? num : 1.0;
  } else if (field === 'device') {
    _currentRoomAudioInputs[idx].device = normalizeAudioDeviceId(value);
  } else if (field === 'label') {
    _currentRoomAudioInputs[idx].label = value;
  }
  renderRoomAudioInputs();
}

function toggleRoomAudioSelection(defId, checked) {
  if (!Array.isArray(_audioDefsCache)) return;
  const def = _audioDefsCache.find(d => Number(d.id) === Number(defId));
  if (!def) return;

  if (checked) {
    // hinzufügen, falls nicht vorhanden
    const exists = _currentRoomAudioInputs.some(sel => sel.device === def.device && (sel.label || '') === (def.label || ''));
    if (!exists) {
      _currentRoomAudioInputs.push({
        device: normalizeAudioDeviceId(def.device || ''),
        label: def.label || '',
        gain: def.gain != null ? def.gain : 1.0,
        channels: def.channels || null,
      });
    }
  } else {
    _currentRoomAudioInputs = _currentRoomAudioInputs.filter(sel => !(sel.device === def.device && (sel.label || '') === (def.label || '')));
  }
  renderRoomAudioInputs();
}


async function listRooms() {
  const r = await _fetchJson('/api/rooms', { method: 'GET' });
  return Array.isArray(r?.rooms) ? r.rooms : [];
}

async function createRoom(body) {
  return await _fetchJson('/api/rooms', {
    method: 'POST',
    body,
  });
}

async function updateRoom(id, body) {
  return await _fetchJson(`/api/rooms/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body,
  });
}

async function deleteRoom(id) {
  return await _fetchJson(`/api/rooms/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

function roomRowHtml(r) {
  return `
    <tr>
      <td class="px-3 py-2 min-w-0">
        <div class="text-sm font-semibold text-gray-100 truncate" title="${escapeHtml(r.name || '')}">${escapeHtml(r.name || '')}</div>
        <div class="flex flex-wrap items-center gap-1 mt-1">
          <span class="ors-pill ors-pill-id">#${r.id}</span>
          <span class="ors-pill ors-pill-teal" title="Layout">${escapeHtml(r.layout || '')}</span>
          ${r.viewer_id != null ? `<span class="ors-pill ors-pill-id" title="Standard-Viewer">📺 #${r.viewer_id}</span>` : ''}
          <span class="ors-pill ${r.audio_device ? 'ors-pill-ok' : 'ors-pill-off'}" title="${escapeHtml(r.audio_device || 'globales Audiogerät')}">🎚️ ${r.audio_device ? 'eigene Eingänge' : 'Default'}</span>
          ${parseRoomSpeakers(r.speakers).size ? `<span class="ors-pill ors-pill-ok" title="Lautsprecher im Raum">🔊 ${parseRoomSpeakers(r.speakers).size}</span>` : ''}
        </div>
      </td>
      <td class="px-3 py-2 text-right whitespace-nowrap align-middle">
        <button data-action="edit-room" data-id="${r.id}" class="ors-btn-ghost" title="Bearbeiten">✏️ Bearbeiten</button>
        <button data-action="delete-room" data-id="${r.id}" class="ors-btn-danger ml-1" title="Löschen">🗑</button>
      </td>
    </tr>
  `;
}

async function renderRoomsTable(force = false) {
  const tbody = document.getElementById('roomsTableBody');
  const stats = document.getElementById('roomsStats');
  if (!tbody) return;

  if (force || !_roomsCache.length) {
    tbody.innerHTML = `<tr><td colspan="6" class="px-4 py-3 text-sm text-gray-400">Lade Räume…</td></tr>`;
    try {
      _roomsCache = await listRooms();
    } catch (e) {
      console.error('listRooms failed', e);
      tbody.innerHTML = `<tr><td colspan="6" class="px-4 py-3 text-sm text-red-400">Fehler beim Laden der Räume.</td></tr>`;
      return;
    }
  }

  if (!_roomsCache.length) {
    tbody.innerHTML = `<tr><td colspan="6" class="px-4 py-3 text-sm text-gray-400">Keine Räume definiert.</td></tr>`;
  } else {
    tbody.innerHTML = _roomsCache.map(roomRowHtml).join('');
  }

  if (stats) {
    stats.textContent = `${_roomsCache.length} Raum/Räume definiert.`;
  }
}

// Bekannte Lautsprecher (/api/voice/speakers) – genutzt von Raumlautsprecher-Tabelle und
// Raum-Editor; hier oben deklariert, weil der Raum-Editor früher im Modul läuft.
let _speakers = [];
// Lautsprecher des Raums (speakers.id), siehe Abschnitt „4 Lautsprecher“
let _currentRoomSpeakers = new Set();

async function renderRoomSpeakers(reload = false) {
  const box = document.getElementById('roomSpeakersList');
  if (!box) return;
  let list = _speakers;
  if (reload || !list.length) {
    try {
      const res = await _fetchJson('/api/voice/speakers', { method: 'GET' });
      list = Array.isArray(res?.speakers) ? res.speakers : [];
      _speakers = list;
    } catch (e) {
      box.innerHTML = '<div class="text-red-400">Lautsprecherliste nicht erreichbar.</div>';
      return;
    }
  }
  // Zugeordnete, aber inzwischen gelöschte Geräte trotzdem zeigen, damit man sie abwählen kann
  const known = new Set(list.map(sp => sp.id));
  const missing = [..._currentRoomSpeakers].filter(id => !known.has(id));
  if (!list.length && !missing.length) {
    box.innerHTML = '<div class="text-gray-500 sm:col-span-2">Noch keine Lautsprecher angemeldet (siehe Raumlautsprecher unten).</div>';
    return;
  }
  box.innerHTML = list.map(sp => `
      <label class="flex items-center gap-2 cursor-pointer">
        <input type="checkbox" class="checkbox checkbox-xs checkbox-primary" style="border: 1px solid rgba(255,255,255,0.5)" data-speaker-id="${sp.id}" ${_currentRoomSpeakers.has(sp.id) ? 'checked' : ''}>
        <span class="truncate">${escapeHtml(sp.name)}</span>
        <span class="text-gray-500">· ${escapeHtml(sp.group)}${sp.online ? '' : ' · offline'}</span>
      </label>`).join('')
    + missing.map(id => `
      <label class="flex items-center gap-2 cursor-pointer text-gray-500">
        <input type="checkbox" class="checkbox checkbox-xs checkbox-primary" style="border: 1px solid rgba(255,255,255,0.5)" data-speaker-id="${id}" checked>
        <span>#${id} (nicht mehr angemeldet)</span>
      </label>`).join('');
  box.querySelectorAll('input[data-speaker-id]').forEach(cb => {
    cb.addEventListener('change', () => {
      const id = Number(cb.dataset.speakerId);
      if (cb.checked) _currentRoomSpeakers.add(id); else _currentRoomSpeakers.delete(id);
    });
  });
}

function parseRoomSpeakers(raw) {
  try {
    const arr = JSON.parse(raw || '[]');
    return new Set(Array.isArray(arr) ? arr.filter(Number.isFinite) : []);
  } catch { return new Set(); }
}

function resetRoomForm() {
  const idInput     = document.getElementById('roomIdInput');
  const nameInput   = document.getElementById('roomNameInput');
  const layoutSel   = document.getElementById('roomLayoutSelect');
  const viewerInput = document.getElementById('roomViewerInput');
  const sourcesInput= document.getElementById('roomSourcesInput');
  const delBtn      = document.getElementById('roomDeleteBtn');
  const saveStatus  = document.getElementById('roomSaveStatus');

  if (!idInput) return;

  idInput.value = '';
  nameInput.value = '';
  viewerInput.value = '';
  _currentRoomSources = new Array(64).fill(null);
  _currentRoomAudioInputs = [];
  _currentRoomSpeakers = new Set();
  renderRoomSpeakers();

  // Layout default
  renderRoomLayoutSelectOptions();
  layoutSel.value = ROOM_LAYOUT_PRESETS[0].code;

  renderRoomAudioInputs();

  updateRoomSourcesTextareaFromState();
  renderRoomLayoutGrid();
  renderRoomSourcesPalette();

  if (delBtn) delBtn.classList.add('hidden');
  if (saveStatus) saveStatus.textContent = '';
}


function fillRoomForm(room) {
  const idInput     = document.getElementById('roomIdInput');
  const nameInput   = document.getElementById('roomNameInput');
  const layoutSel   = document.getElementById('roomLayoutSelect');
  const viewerInput = document.getElementById('roomViewerInput');
  const sourcesInput= document.getElementById('roomSourcesInput');
  const delBtn      = document.getElementById('roomDeleteBtn');
  const saveStatus  = document.getElementById('roomSaveStatus');

  if (!idInput) return;

  idInput.value       = room.id;
  nameInput.value     = room.name || '';
  viewerInput.value   = room.viewer_id != null ? String(room.viewer_id) : '';

  renderRoomLayoutSelectOptions();
  layoutSel.value     = room.layout || ROOM_LAYOUT_PRESETS[0].code;

  // Audio-Eingänge aus dem Raum übernehmen
  _currentRoomAudioInputs = parseRoomAudioString(room.audio_device || '');
  renderRoomAudioInputs();

  _currentRoomSpeakers = parseRoomSpeakers(room.speakers);
  renderRoomSpeakers();

  // Sources: JSON -> _currentRoomSources (Kamera-IDs)
  _currentRoomSources = new Array(64).fill(null);
  if (room.sources) {
    try {
      const arr = JSON.parse(room.sources);
      if (Array.isArray(arr)) {
        arr.forEach((v, idx) => {
          if (idx < _currentRoomSources.length) {
            if (v === null || v === undefined) {
              _currentRoomSources[idx] = null;
            } else if (typeof v === 'number') {
              _currentRoomSources[idx] = v;
            }
          }
        });
      }
    } catch (e) {
      console.warn('parse room.sources failed', e);
    }
  }

  updateRoomSourcesTextareaFromState();
  renderRoomLayoutGrid();
  renderRoomSourcesPalette();

  if (delBtn) delBtn.classList.remove('hidden');
  if (saveStatus) saveStatus.textContent = `Bearbeite Raum #${room.id}`;
}


function renderRoomLayoutSelectOptions() {
  const layoutSel = document.getElementById('roomLayoutSelect');
  if (!layoutSel || layoutSel.dataset.filled === '1') return;

  layoutSel.innerHTML = ROOM_LAYOUT_PRESETS
    .map(p => `<option value="${p.code}">${escapeHtml(p.label)} (${escapeHtml(p.code)})</option>`)
    .join('');

  layoutSel.dataset.filled = '1';
}

function updateRoomSourcesTextareaFromState() {
  const sourcesInput = document.getElementById('roomSourcesInput');
  if (!sourcesInput) return;

  // auf letzte nicht-leere Kamera-ID kürzen
  let lastNonEmpty = -1;
  for (let i = 0; i < _currentRoomSources.length; i++) {
    const v = _currentRoomSources[i];
    if (v !== null && v !== undefined) {
      lastNonEmpty = i;
    }
  }
  const arr = lastNonEmpty >= 0
    ? _currentRoomSources.slice(0, lastNonEmpty + 1)
    : [];

  // arr ist jetzt z.B. [1, null, 3] (Kamera-IDs)
  sourcesInput.value = JSON.stringify(arr, null, 2);
}


function syncRoomStateFromTextarea() {
  const sourcesInput = document.getElementById('roomSourcesInput');
  if (!sourcesInput) return;
  const raw = sourcesInput.value.trim();
  if (!raw) {
    _currentRoomSources = new Array(64).fill(null);
    return;
  }

  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) throw new Error('not array');

    const next = new Array(64).fill(null);
    parsed.forEach((v, idx) => {
      if (idx >= next.length) return;
      if (v === null || v === undefined) {
        next[idx] = null;
      } else if (typeof v === 'number') {
        next[idx] = v;
      } else {
        // Strings, Objekte etc. ignorieren wir
        console.warn('Ungültiger Kamera-ID-Wert in room.sources:', v);
      }
    });

    _currentRoomSources = next;
  } catch (e) {
    console.warn('syncRoomStateFromTextarea parse failed', e);
    showToast('Sources-JSON ist ungültig. Es müssen Arrays von Kamera-IDs (Zahlen) oder null sein, z.B. [1,null,3]. Änderungen wurden nicht übernommen.', false);
  }
}


// Kamera-Palette (Links)
async function renderRoomSourcesPalette() {
  const palette = document.getElementById('roomSourcesPalette');
  if (!palette) return;

  // Kameras laden, falls noch nicht vorhanden
  if (!_sourcesCache.length) {
    _sourcesCache = await loadSourcesSettings();
  }

  const cameras = _sourcesCache.filter(cam => cam.enabled && cam.url && cam.url.trim() !== '');

  if (!cameras.length) {
    palette.innerHTML = `
      <div class="col-span-full text-xs text-gray-500">
        Noch keine Kameras definiert oder aktiv. Lege Kameras im Kamera-Tab an.
      </div>
    `;
    return;
  }

  palette.innerHTML = cameras.map(cam => {
    const shortUrl = cam.url.length > 60 ? cam.url.slice(0, 57) + '…' : cam.url;
    return `
      <div class="room-source-card"
           draggable="true"
           data-camera-id="${cam.id}"
           title="${escapeHtml(cam.url || '')}">
        <div class="flex items-center justify-between gap-2">
          <span class="ors-pill ors-pill-id">#${cam.id}</span>
          <span class="text-[10px] text-gray-500">⠿ ziehen</span>
        </div>
        <div class="ors-thumb">
          <!-- Preview – Backend: /api/camera_preview/<id> -->
          <img src="/api/camera_preview/${cam.id}"
               alt=""
               onerror="this.style.display='none'; this.parentElement.textContent='keine Vorschau';" />
        </div>
        <div class="text-[11px] text-gray-100 font-semibold truncate" title="${escapeHtml(cam.name || '')}">
          ${escapeHtml(cam.name || '')}
        </div>
        <div class="ors-mono truncate">${escapeHtml(shortUrl)}</div>
      </div>
    `;
  }).join('');

  // Drag-Events für Karten
  palette.querySelectorAll('.room-source-card').forEach(card => {
    if (card.dataset.boundDrag === '1') return;
    card.dataset.boundDrag = '1';
    card.addEventListener('dragstart', (e) => {
      const camId = Number(card.dataset.cameraId || '0');
      if (!camId) return;
      const payload = JSON.stringify({ camera_id: camId });
      e.dataTransfer.effectAllowed = 'copyMove';
      e.dataTransfer.setData('application/json', payload);
      e.dataTransfer.setData('text/plain', payload);
      card.classList.add('opacity-50');
    });
    card.addEventListener('dragend', () => {
      card.classList.remove('opacity-50');
    });
  });

  // Palette fertig → Layout noch mal rendern, damit evtl. Namen angezeigt werden
  renderRoomLayoutGrid();
}


// Layout-Grid (Rechts)
function renderRoomLayoutGrid() {
  const grid = document.getElementById('roomLayoutGrid');
  const layoutSel = document.getElementById('roomLayoutSelect');
  const hint = document.getElementById('roomLayoutHint');
  if (!grid || !layoutSel) return;

  const code = layoutSel.value || ROOM_LAYOUT_PRESETS[0].code;
  const preset = roomLayoutByCode(code);
  const rows = preset.rows;
  const cols = preset.cols;
  const tilesCount = rows * cols;

  grid.innerHTML = `
    <div class="grid gap-2 w-full h-full"
         style="grid-template-columns: repeat(${cols}, minmax(0, 1fr)); grid-auto-rows: 1fr;">
      ${
        Array.from({ length: tilesCount }, (_, i) => {
          const camId = _currentRoomSources[i];
          let cam = null;
          if (camId != null && _sourcesCache.length) {
            cam = _sourcesCache.find(c => String(c.id) === String(camId)) || null;
          }

          const label = cam
            ? cam.name || `Kamera #${cam.id}`
            : (camId != null ? `Kamera #${camId}` : 'keine Kamera');

          const hasCam = !!cam;
          const previewSrc = cam ? `/api/camera_preview/${cam.id}` : '';

          return `
            <div class="room-layout-tile group relative text-[11px] text-gray-300 ${camId != null ? 'is-filled' : ''}"
                 data-slot-index="${i}">
              <div class="flex items-center justify-between gap-2">
                <span class="ors-pill ors-pill-id">Slot ${i}</span>
                <span class="text-[10px] text-gray-500 group-hover:text-teal-300">${camId != null ? 'Klick entfernt' : 'Kamera hierher'}</span>
              </div>
              <div class="ors-tile-img">
                ${
                  hasCam
                    ? `<img src="${previewSrc}"
                             alt=""
                             onerror="this.style.display='none'; this.parentElement.textContent='keine Vorschau';" />`
                    : `<span>Testbild / leer</span>`
                }
              </div>
              <div class="text-[10px] ${camId != null ? 'text-teal-200 font-semibold' : 'text-gray-500'} truncate min-h-[1.2em]" title="${escapeHtml(label)}">
                ${camId != null ? escapeHtml(label) : 'keine Kamera'}
              </div>
            </div>
          `;
        }).join('')
      }
    </div>
  `;

  if (hint) {
    hint.textContent = `${rows} × ${cols} = ${tilesCount} Kacheln (Slots 0–${tilesCount - 1})`;
  }

  // DnD-Events für Kacheln
  grid.querySelectorAll('.room-layout-tile').forEach(tile => {
    tile.addEventListener('dragover', (e) => {
      e.preventDefault();
      tile.classList.add('ring');
    });
    tile.addEventListener('dragleave', () => {
      tile.classList.remove('ring');
    });
    tile.addEventListener('drop', (e) => {
      e.preventDefault();
      tile.classList.remove('ring');
      const data = e.dataTransfer.getData('application/json') || e.dataTransfer.getData('text/plain');
      if (!data) return;
      try {
        const parsed = JSON.parse(data);
        const camId = parsed.camera_id;
        const slotIdx = Number(tile.dataset.slotIndex || '0');
        if (typeof camId !== 'number') return;
        setRoomSlotSource(slotIdx, camId);
      } catch (err) {
        console.warn('drop parse failed', err);
      }
    });
  });
}

function setRoomSlotSource(slotIdx, cameraId) {
  if (slotIdx < 0 || slotIdx >= _currentRoomSources.length) return;
  _currentRoomSources[slotIdx] = cameraId != null ? Number(cameraId) : null;
  updateRoomSourcesTextareaFromState();
  renderRoomLayoutGrid();
}

async function saveRoomFromForm() {
  const idInput     = document.getElementById('roomIdInput');
  const nameInput   = document.getElementById('roomNameInput');
  const layoutSel   = document.getElementById('roomLayoutSelect');
  const viewerInput = document.getElementById('roomViewerInput');
  const sourcesInput= document.getElementById('roomSourcesInput');
  const saveStatus  = document.getElementById('roomSaveStatus');

  const id          = idInput.value ? Number(idInput.value) : null;
  const name        = nameInput.value.trim();
  const layout      = layoutSel.value.trim();
  const viewer_id   = viewerInput.value.trim() === '' ? null : Number(viewerInput.value.trim());

  // JSON aus Textarea übernehmen (falls der User manuell editiert hat)
  syncRoomStateFromTextarea();

  // Kürzen und JSON generieren
  let lastNonEmpty = -1;
  for (let i = 0; i < _currentRoomSources.length; i++) {
    const v = _currentRoomSources[i];
    if (v != null && String(v).trim() !== '') lastNonEmpty = i;
  }
  const arr = lastNonEmpty >= 0
    ? _currentRoomSources.slice(0, lastNonEmpty + 1)
    : [];
  const sources = JSON.stringify(arr);

  // Audio-Inputs säubern
  const audio_clean = (_currentRoomAudioInputs || [])
    .filter(a => a && a.device && a.device.trim() !== '')
    .map(a => ({
      device: a.device.trim(),
      label: a.label ? a.label.toString() : '',
      gain: Number.isFinite(Number(a.gain)) ? Number(a.gain) : 1.0,
    }));
  const audio_device = audio_clean.length ? JSON.stringify(audio_clean) : null;
  const hiddenAudioInput = document.getElementById('roomAudioInput');
  if (hiddenAudioInput) hiddenAudioInput.value = audio_device || '';

  if (!name || !layout) {
    showToast('Name und Layout sind Pflichtfelder.', false);
    return;
  }

  const speakers = JSON.stringify([..._currentRoomSpeakers].sort((a, b) => a - b));
  const body = { name, layout, audio_device, viewer_id, sources, speakers };

  try {
    if (id == null) {
      const created = await createRoom(body);
      _roomsCache.push(created);
      showToast('Raum angelegt ✅', true);
      if (saveStatus) saveStatus.textContent = `Raum #${created.id} gespeichert`;
      fillRoomForm(created);
    } else {
      await updateRoom(id, body);
      const idx = _roomsCache.findIndex(r => r.id === id);
      if (idx >= 0) {
        _roomsCache[idx] = { ..._roomsCache[idx], ...body };
      }
      showToast('Raum aktualisiert ✅', true);
      if (saveStatus) saveStatus.textContent = `Raum #${id} gespeichert`;
      const room = _roomsCache.find(r => r.id === id);
      if (room) fillRoomForm(room);
    }
    renderRoomsTable(false);
  } catch (e) {
    console.error('saveRoomFromForm failed', e);
    showToast('Speichern fehlgeschlagen ❌', false);
  }
}

async function deleteRoomFromForm() {
  const idInput = document.getElementById('roomIdInput');
  const id = idInput.value ? Number(idInput.value) : null;
  if (!id) return;

  if (!confirm('Diesen Raum wirklich löschen?')) return;

  try {
    await deleteRoom(id);
    _roomsCache = _roomsCache.filter(r => r.id !== id);
    showToast('Raum gelöscht ✅', true);
    renderRoomsTable(false);
    resetRoomForm();
  } catch (e) {
    console.error('deleteRoomFromForm failed', e);
    showToast('Löschen fehlgeschlagen ❌', false);
  }
}

// ======== ROOMS: EVENT BINDING ===================================

(function () {
  const openBtn   = document.getElementById('openRoomsModalBtn');
  const closeBtn  = document.getElementById('closeRoomsModalBtn');
  const modal     = document.getElementById('roomsModal');
  const tbody     = document.getElementById('roomsTableBody');
  const form      = document.getElementById('roomEditForm');
  const addBtn    = document.getElementById('addRoomBtn');
  const delBtn    = document.getElementById('roomDeleteBtn');
  const layoutSel = document.getElementById('roomLayoutSelect');
  const sourcesInput = document.getElementById('roomSourcesInput');
  const audioModalBtn = document.getElementById('roomAudioModalBtn');
  const audioModal = document.getElementById('roomAudioModal');
  const audioModalClose = document.getElementById('closeRoomAudioModalBtn');
  const audioModalSave = document.getElementById('saveRoomAudioBtn');
  const audioDefsList = document.getElementById('roomAudioDefsList');
  const audioAddBtn = document.getElementById('roomAudioAddBtn');
  document.getElementById('roomSpeakersRefreshBtn')?.addEventListener('click', () => renderRoomSpeakers(true));
  const refreshPaletteBtn = document.getElementById('refreshSourcesPaletteBtn');

  function openRoomsModal() {
    if (!modal) return;

    // Audio-Geräte-Liste laden/aktualisieren, sobald das Modal geöffnet wird
    loadAudioInputDevices();
    loadAudioInputDefs().then(() => {
      renderRoomAudioInputs();
    });

    modal.classList.remove('hidden');
    document.documentElement.classList.add('overflow-hidden');
    document.body.classList.add('modal-open');
    renderRoomsTable(true);
    if (!document.getElementById('roomIdInput').value) {
      resetRoomForm();
    }
  }

  function closeRoomsModal() {
    if (!modal) return;
    modal.classList.add('hidden');
    document.documentElement.classList.remove('overflow-hidden');
    document.body.classList.remove('modal-open');
  }

  if (openBtn && !openBtn.dataset.boundRoomsOpen) {
    openBtn.dataset.boundRoomsOpen = '1';
    openBtn.addEventListener('click', (e) => {
      e.preventDefault();
      openRoomsModal();
    });
  }

  if (closeBtn && !closeBtn.dataset.boundRoomsClose) {
    closeBtn.dataset.boundRoomsClose = '1';
    closeBtn.addEventListener('click', (e) => {
      e.preventDefault();
      closeRoomsModal();
    });
  }

  if (modal && !modal.dataset.boundRoomsBackdrop) {
    modal.dataset.boundRoomsBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) closeRoomsModal();
    });
  }

  if (addBtn && !addBtn.dataset.boundRoomsAdd) {
    addBtn.dataset.boundRoomsAdd = '1';
    addBtn.addEventListener('click', (e) => {
      e.preventDefault();
      resetRoomForm();
    });
  }

  if (tbody && !tbody.dataset.boundRoomsTable) {
    tbody.dataset.boundRoomsTable = '1';
    tbody.addEventListener('click', async (e) => {
      const editBtn = e.target.closest('button[data-action="edit-room"]');
      const delBtnInline = e.target.closest('button[data-action="delete-room"]');
      if (editBtn) {
        const id = Number(editBtn.dataset.id);
        const room = _roomsCache.find(r => r.id === id);
        if (room) fillRoomForm(room);
      } else if (delBtnInline) {
        const id = Number(delBtnInline.dataset.id);
        if (!confirm('Diesen Raum wirklich löschen?')) return;
        await deleteRoom(id);
        _roomsCache = _roomsCache.filter(r => r.id !== id);
        renderRoomsTable(false);
        showToast('Raum gelöscht ✅', true);
        resetRoomForm();
      }
    });
  }

  if (form && !form.dataset.boundRoomsForm) {
    form.dataset.boundRoomsForm = '1';
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      await saveRoomFromForm();
    });
  }

  if (delBtn && !delBtn.dataset.boundRoomsDelete) {
    delBtn.dataset.boundRoomsDelete = '1';
    delBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      await deleteRoomFromForm();
    });
  }

  if (layoutSel && !layoutSel.dataset.boundRoomsLayout) {
    layoutSel.dataset.boundRoomsLayout = '1';
    layoutSel.addEventListener('change', () => {
      renderRoomLayoutGrid();
    });
  }

  if (sourcesInput && !sourcesInput.dataset.boundRoomsSources) {
    sourcesInput.dataset.boundRoomsSources = '1';
    sourcesInput.addEventListener('blur', () => {
      syncRoomStateFromTextarea();
      renderRoomLayoutGrid();
      renderRoomSourcesPalette();
    });
  }

  if (refreshPaletteBtn && !refreshPaletteBtn.dataset.boundRoomsRefresh) {
    refreshPaletteBtn.dataset.boundRoomsRefresh = '1';
    refreshPaletteBtn.addEventListener('click', (e) => {
      e.preventDefault();
      renderRoomSourcesPalette();
    });
  }

  const openAudioModal = async () => {
    if (!audioModal) return;
    await loadAudioInputDevices();
    await loadAudioInputDefs();
    renderRoomAudioModal();
    audioModal.classList.remove('hidden');
    document.documentElement.classList.add('overflow-hidden');
  };
  const closeAudioModal = () => {
    if (!audioModal) return;
    audioModal.classList.add('hidden');
    document.documentElement.classList.remove('overflow-hidden');
  };
  if (audioModalBtn && !audioModalBtn.dataset.boundAudioModalBtn) {
    audioModalBtn.dataset.boundAudioModalBtn = '1';
    audioModalBtn.addEventListener('click', async (e) => { e.preventDefault(); await openAudioModal(); });
  }
  if (audioModalClose && !audioModalClose.dataset.boundAudioModalClose) {
    audioModalClose.dataset.boundAudioModalClose = '1';
    audioModalClose.addEventListener('click', (e) => { e.preventDefault(); closeAudioModal(); });
  }
  if (audioModalSave && !audioModalSave.dataset.boundAudioModalSave) {
    audioModalSave.dataset.boundAudioModalSave = '1';
    audioModalSave.addEventListener('click', async (e) => {
      e.preventDefault();
      try {
        const clean = (_audioDefsCache || [])
          .filter(a => a && a.device && a.device.trim() !== '')
          .map(a => ({
            ...a,
            device: normalizeAudioDeviceId(a.device),
          }));
        const saved = await saveAudioInputDefs(clean);
        _audioDefsCache = Array.isArray(saved?.audio_inputs)
          ? saved.audio_inputs.map(a => ({ ...a, device: normalizeAudioDeviceId(a.device || '') }))
          : clean;
        renderRoomAudioModal();
        renderRoomAudioInputs();
        if (window.showToast) showToast('Audio gespeichert ✅', true);
      } catch (err) {
        console.error('save audio defs failed', err);
        if (window.showToast) showToast('Audio speichern fehlgeschlagen ❌', false);
      } finally {
        closeAudioModal();
      }
    });
  }
  if (audioModal && !audioModal.dataset.boundAudioModalBackdrop) {
    audioModal.dataset.boundAudioModalBackdrop = '1';
    audioModal.addEventListener('click', (e) => {
      if (e.target === audioModal) closeAudioModal();
    });
  }
  if (audioDefsList && !audioDefsList.dataset.boundRoomsAudioDefs) {
    audioDefsList.dataset.boundRoomsAudioDefs = '1';
    audioDefsList.addEventListener('input', (e) => {
      const row = e.target.closest('[data-audio-def-idx]');
      if (!row) return;
      const idx = Number(row.dataset.audioDefIdx || '0');
      const field = e.target.dataset.field;
      if (e.target.dataset.audioDefId) {
        toggleRoomAudioSelection(e.target.dataset.audioDefId, e.target.checked);
        renderRoomAudioInputs();
        return;
      }
      if (!field) return;
      updateAudioDef(idx, field, e.target.value);
    });
    audioDefsList.addEventListener('click', (e) => {
      const btn = e.target.closest('button[data-action="remove-audio-def"]');
      if (!btn) return;
      const row = btn.closest('[data-audio-def-idx]');
      if (!row) return;
      const idx = Number(row.dataset.audioDefIdx || '0');
      removeAudioDef(idx);
      _currentRoomAudioInputs = (_currentRoomAudioInputs || []).filter(sel => {
        const def = _audioDefsCache[idx];
        return !(def && sel.device === def.device && (sel.label || '') === (def.label || ''));
      });
      renderRoomAudioModal();
      renderRoomAudioInputs();
    });
  }
  if (audioAddBtn && !audioAddBtn.dataset.boundRoomsAudioAdd) {
    audioAddBtn.dataset.boundRoomsAudioAdd = '1';
    audioAddBtn.addEventListener('click', (e) => {
      e.preventDefault();
      addAudioDef();
      renderRoomAudioModal();
    });
  }
  // ESC schließt Modal
  window.addEventListener('keydown', (ev) => {
    if (ev.key !== 'Escape') return;
    const m = document.querySelector('#roomsModal');
    if (!m || m.classList.contains('hidden')) return;
    closeRoomsModal();
  });
})();

// ======== AUDIO MODAL (GLOBAL INPUTS) ==============================
(function () {
  const openBtn = document.getElementById('openAudioModalBtn');
  const closeBtn = document.getElementById('closeAudioModalBtn');
  const modal = document.getElementById('audioModal');
  const list = document.getElementById('audioList');
  const addBtn = document.getElementById('audioAddBtn');
  const saveBtn = document.getElementById('saveAudioInputsBtn');

  async function openAudioModal() {
    if (!modal) return;
    await loadAudioInputDevices();
    await loadAudioInputDefs();
    renderAudioDefsModal();
    modal.classList.remove('hidden');
    document.documentElement.classList.add('overflow-hidden');
    window.__openAudioModal = openAudioModal;
  }

  function closeAudioModal() {
    if (!modal) return;
    modal.classList.add('hidden');
    document.documentElement.classList.remove('overflow-hidden');
  }

  if (openBtn && !openBtn.dataset.boundAudioOpen) {
    openBtn.dataset.boundAudioOpen = '1';
    openBtn.addEventListener('click', (e) => {
      e.preventDefault();
      openAudioModal();
    });
  }
  // Fallback: global helper für Inline-Handler
  window.__openAudioModal = openAudioModal;
  window.__closeAudioModal = closeAudioModal;
  if (closeBtn && !closeBtn.dataset.boundAudioClose) {
    closeBtn.dataset.boundAudioClose = '1';
    closeBtn.addEventListener('click', (e) => {
      e.preventDefault();
      closeAudioModal();
    });
  }
  if (modal && !modal.dataset.boundAudioBackdrop) {
    modal.dataset.boundAudioBackdrop = '1';
    modal.addEventListener('click', (e) => {
      if (e.target === modal) closeAudioModal();
    });
  }
  if (addBtn && !addBtn.dataset.boundAudioAdd) {
    addBtn.dataset.boundAudioAdd = '1';
    addBtn.addEventListener('click', (e) => {
      e.preventDefault();
      addAudioDef();
    });
  }
  if (saveBtn && !saveBtn.dataset.boundAudioSave) {
    saveBtn.dataset.boundAudioSave = '1';
    saveBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      try {
        const clean = (_audioDefsCache || [])
          .filter(a => a && a.device && a.device.trim() !== '')
          .map(a => ({ ...a, device: normalizeAudioDeviceId(a.device) }));
        const saved = await saveAudioInputDefs(clean);
        _audioDefsCache = Array.isArray(saved?.audio_inputs)
          ? saved.audio_inputs.map(a => ({ ...a, device: normalizeAudioDeviceId(a.device || '') }))
          : clean;
        showToast('Audio-Inputs gespeichert ✅', true);
        closeAudioModal();
        renderRoomAudioInputs(); // aktualisiere Auswahl im Room-Modal
      } catch (err) {
        console.error('save audio inputs failed', err);
        showToast('Audio-Inputs speichern fehlgeschlagen ❌', false);
      }
    });
  }
  if (list && !list.dataset.boundAudioList) {
    list.dataset.boundAudioList = '1';
    list.addEventListener('input', (e) => {
      const row = e.target.closest('[data-audio-def-idx]');
      if (!row) return;
      const idx = Number(row.dataset.audioDefIdx || '0');
      const field = e.target.dataset.field;
      if (!field) return;
      updateAudioDef(idx, field, e.target.value);
    });
    list.addEventListener('click', (e) => {
      const btn = e.target.closest('button[data-action="remove-audio-def"]');
      if (!btn) return;
      const row = btn.closest('[data-audio-def-idx]');
      if (!row) return;
      const idx = Number(row.dataset.audioDefIdx || '0');
      removeAudioDef(idx);
    });
  }
})();

// ===== Viewer Overlay (Ticker & Trivia) ================================
(function initOverlaySettings() {
  const txtTicker = document.getElementById('tickerMessagesInput');
  const btnSaveTicker = document.getElementById('overlaySaveTickerBtn');
  const btnReload = document.getElementById('overlayReloadBtn');
  const uploadBtn = document.getElementById('triviaUploadBtn');
  const uploadInput = document.getElementById('triviaUploadInput');
  const uploadTitle = document.getElementById('triviaTitleInput');
  const listBox = document.getElementById('triviaList');
  const statusEl = document.getElementById('overlayStatus');

  async function loadOverlay() {
    if (statusEl) statusEl.textContent = 'Lade…';
    try {
      const res = await _fetchJson('/api/ui/content', { method: 'GET' });
      const msgs = Array.isArray(res.ticker_messages) ? res.ticker_messages : [];
      if (txtTicker) {
        txtTicker.value = msgs.join('\n');
      }
      renderTrivia(Array.isArray(res.trivia_images) ? res.trivia_images : []);
      if (statusEl) statusEl.textContent = '';
    } catch (e) {
      console.error('Overlay laden fehlgeschlagen', e);
      if (statusEl) statusEl.textContent = 'Fehler beim Laden.';
    }
  }

  function renderTrivia(list) {
    if (!listBox) return;
    listBox.innerHTML = '';
    if (!list || list.length === 0) {
      listBox.innerHTML = '<div class="text-gray-500 text-xs">Keine Trivia-Bilder hinterlegt.</div>';
      return;
    }
    list.forEach((item) => {
      const row = document.createElement('div');
      row.className = 'flex items-center gap-3 border border-gray-700 rounded-lg p-2';
      const img = document.createElement('img');
      img.src = item.url || item.file || '';
      img.alt = item.title || '';
      img.className = 'w-16 h-10 object-cover rounded';
      const title = document.createElement('div');
      title.className = 'flex-1 text-sm text-gray-100';
      title.textContent = item.title || item.file || '';
      const del = document.createElement('button');
      del.className = 'btn btn-xs bg-red-600 text-white border-none';
      del.textContent = 'Löschen';
      del.addEventListener('click', () => {
        deleteTrivia(item.file);
      });
      row.appendChild(img);
      row.appendChild(title);
      row.appendChild(del);
      listBox.appendChild(row);
    });
  }

  async function saveTicker() {
    const val = (txtTicker?.value || '').split(/\r?\n/).map((s) => s.trim()).filter((s) => s.length > 0);
    try {
      await _fetchJson('/api/ticker', { method: 'POST', body: { messages: val } });
      showToast('Ticker gespeichert', true);
    } catch (e) {
      console.error('Ticker speichern fehlgeschlagen', e);
      showToast('Ticker konnte nicht gespeichert werden', false);
    }
  }

  async function fetchAuthMultipart(url, formData) {
    const res = await authFetch(url, { method: 'POST', headers: withAuthHeaders({}), body: formData });
    if (!res.ok) {
      const txt = await res.text().catch(() => '');
      const err = new Error(`HTTP ${res.status}`);
      err.body = txt;
      throw err;
    }
    return res.json().catch(() => ({}));
  }

  async function uploadTrivia() {
    const file = uploadInput?.files?.[0];
    if (!file) {
      showToast('Bitte eine Bilddatei wählen', false);
      return;
    }
    const fd = new FormData();
    fd.append('file', file);
    if (uploadTitle && uploadTitle.value.trim()) {
      fd.append('title', uploadTitle.value.trim());
    }
    try {
      await fetchAuthMultipart('/api/trivia', fd);
      if (uploadInput) uploadInput.value = '';
      if (uploadTitle) uploadTitle.value = '';
      showToast('Trivia hochgeladen', true);
      loadOverlay();
    } catch (e) {
      console.error('Trivia-Upload fehlgeschlagen', e);
      showToast('Upload fehlgeschlagen', false);
    }
  }

  async function deleteTrivia(file) {
    if (!file) return;
    if (!confirm('Trivia-Bild wirklich löschen?')) return;
    try {
      await _fetchJson(`/api/trivia/${encodeURIComponent(file)}`, { method: 'DELETE' });
      showToast('Trivia gelöscht', true);
      loadOverlay();
    } catch (e) {
      console.error('Trivia löschen fehlgeschlagen', e);
      showToast('Löschen fehlgeschlagen', false);
    }
  }

  if (btnSaveTicker) btnSaveTicker.addEventListener('click', (e) => { e.preventDefault(); saveTicker(); });
  if (btnReload) btnReload.addEventListener('click', (e) => { e.preventDefault(); loadOverlay(); });
  if (uploadBtn) uploadBtn.addEventListener('click', (e) => { e.preventDefault(); uploadTrivia(); });

  loadOverlay();
})();


// ======== USER MANAGEMENT (Admin) ================================
(function initUserMgmt() {
  const refreshBtn = document.getElementById('userRefreshBtn');
  const createBtn = document.getElementById('userCreateBtn');
  const table = document.getElementById('userTableBody');

  if (refreshBtn) {
    refreshBtn.addEventListener('click', (e) => {
      e.preventDefault();
      loadUsers();
    });
  }
  if (createBtn) {
    createBtn.addEventListener('click', (e) => {
      e.preventDefault();
      createUser();
    });
  }
  if (table) {
    table.addEventListener('click', (e) => {
      const btn = e.target.closest('button[data-action="delete-user"]');
      if (!btn) return;
      const id = Number(btn.dataset.userId);
      if (!Number.isFinite(id)) return;
      if (confirm('Benutzer wirklich löschen?')) {
        deleteUser(id);
      }
    });
  }

  // initial load -> checks token & role and toggles visibility
  fetchCurrentUser();
})();

// ======== ADMIN SETTINGS (ENV/DB) ================================
(function initAdminSettings() {
  const btnSave = document.getElementById('adminSettingsSave');
  const btnRefresh = document.getElementById('adminSettingsRefresh');
  const btnUpload = document.getElementById('adminUpdateUpload');
  const btnList = document.getElementById('adminUpdateList');
  const btnLogsCleanup = document.getElementById('adminLogsCleanupBtn');
  const listBox = document.getElementById('adminUpdateListBox');
  if (btnSave) {
    btnSave.addEventListener('click', (e) => {
      e.preventDefault();
      saveAdminSettings();
    });
  }
  if (btnRefresh) {
    btnRefresh.addEventListener('click', (e) => {
      e.preventDefault();
      loadAdminSettings();
    });
  }
  if (btnUpload) {
    btnUpload.addEventListener('click', (e) => {
      e.preventDefault();
      uploadUpdateFile();
    });
  }
  if (btnList) {
    btnList.addEventListener('click', (e) => {
      e.preventDefault();
      loadUpdateFiles();
    });
  }
  if (btnLogsCleanup) {
    btnLogsCleanup.addEventListener('click', (e) => {
      e.preventDefault();
      cleanupLogFiles();
    });
  }
  if (listBox && !listBox.dataset.boundInstall) {
    listBox.dataset.boundInstall = '1';
    listBox.addEventListener('click', (e) => {
      const btn = e.target.closest('button[data-action="install-update"]');
      if (!btn) return;
      installUpdate(btn.dataset.path);
    });
  }
})();

bindInviteTokenModal();
bindSelfRegister();

// ======== RAUMLAUTSPRECHER (Voice of Room) ==========================
let _speakersTimer = null;

// Freigabe-Zustand: ESP-Lautsprecher mit neuer Firmware müssen freigegeben werden,
// alte Firmware/Pi-Client laufen übergangsweise ungesichert weiter (siehe pairing.rs).
function speakerAuthBadge(sp) {
  if (sp.auth === 'pending') return ' <span class="badge badge-error badge-sm" title="Bekommt keinen Ton, bis es freigegeben wird">nicht freigegeben</span>';
  if (sp.auth === 'legacy') return ' <span class="badge badge-ghost badge-sm" title="Alte Firmware ohne Schlüssel – übergangsweise ohne Freigabe beschallt">alt · ungesichert</span>';
  if (sp.auth === 'approved') return ' <span class="badge badge-ghost badge-sm" title="Mit Schlüssel freigegeben">🔒</span>';
  return '';
}

function speakerStatusBadge(sp) {
  if (!sp.online) return `<span class="badge badge-ghost badge-sm">offline</span>`;
  if (sp.receiving) return `<span class="badge badge-success badge-sm">empfängt</span>`;
  return `<span class="badge badge-info badge-sm">online</span>`;
}

function renderSpeakersTable() {
  const tbody = document.getElementById('speakersTableBody');
  if (!tbody) return;
  const dl = document.getElementById('speakerGroupsList');
  if (dl) {
    const groups = Array.from(new Set(_speakers.map(s => s.group).filter(Boolean)));
    dl.innerHTML = groups.map(g => `<option value="${escapeHtml(g)}"></option>`).join('');
  }
  if (!_speakers.length) {
    tbody.innerHTML = `<tr><td colspan="6" class="px-3 py-4 text-center text-gray-500">Noch kein Lautsprecher angemeldet. Sobald ein Gerät mit dem Client startet, erscheint es hier.</td></tr>`;
    return;
  }
  tbody.innerHTML = _speakers.map((sp) => `
    <tr data-id="${sp.id}" class="${sp.online ? '' : 'opacity-60'}">
      <td class="px-3 py-2 whitespace-nowrap">${speakerStatusBadge(sp)}${sp.muted ? ' <span class="badge badge-warning badge-sm">stumm</span>' : ''}${speakerAuthBadge(sp)}</td>
      <td class="px-3 py-2"><input data-field="name" class="input input-xs input-bordered bg-gray-800/70 w-full" value="${escapeHtml(sp.name)}" title="Umbenennen: Feld verlassen speichert"></td>
      <td class="px-3 py-2"><input data-field="group" list="speakerGroupsList" class="input input-xs input-bordered bg-gray-800/70 w-32" value="${escapeHtml(sp.group)}" title="Gruppe: Feld verlassen speichert"></td>
      <td class="px-3 py-2 text-xs font-mono text-gray-300">${escapeHtml(sp.ip || '–')}${sp.rtp_port ? ':' + sp.rtp_port : ''}
        <div class="text-[10px] text-gray-500">${sp.online ? (sp.codec || '') + (sp.version ? ' · v' + escapeHtml(sp.version) : '') : (sp.last_seen ? 'zuletzt ' + new Date(sp.last_seen * 1000).toLocaleString('de-DE') : '')}</div></td>
      <td class="px-3 py-2">
        <div class="flex items-center gap-2">
          <input data-field="volume" type="range" min="0" max="100" value="${Number(sp.volume) || 0}" class="range range-xs range-primary w-24">
          <span data-volume-label class="text-xs w-8 text-right">${Number(sp.volume) || 0}</span>
        </div>
      </td>
      <td class="px-3 py-2 text-right">
        <div class="flex gap-1 justify-end flex-wrap">
          <button class="btn btn-xs btn-outline" data-action="test" title="Testton abspielen" ${sp.online ? '' : 'disabled'}>🔔 Test</button>
          <button class="btn btn-xs btn-outline" data-action="identify" title="Gerät blinken/piepen lassen" ${sp.online ? '' : 'disabled'}>📍</button>
          <button class="btn btn-xs btn-outline" data-action="mute" title="${sp.muted ? 'Ton an' : 'Stumm'}" ${sp.online ? '' : 'disabled'}>${sp.muted ? '🔈' : '🔇'}</button>
          <button class="btn btn-xs btn-outline" data-action="reboot" title="Gerät neu starten" ${sp.online ? '' : 'disabled'}>⟳</button>
          ${sp.auth === 'pending' ? `<button class="btn btn-xs btn-success" data-action="approve" title="Gerät freigeben – vorher mit 📍 prüfen, welches Gerät piept" ${sp.online ? '' : 'disabled'}>✅ Freigeben</button>` : ''}
          ${sp.auth === 'approved' ? '<button class="btn btn-xs btn-outline" data-action="revoke" title="Freigabe entziehen – Gerät bekommt keinen Ton mehr">🔒</button>' : ''}
          <button class="btn btn-xs btn-error" data-action="delete" title="Aus der Liste entfernen">✕</button>
        </div>
      </td>
    </tr>`).join('');

  tbody.querySelectorAll('tr[data-id]').forEach((row) => {
    const id = Number(row.dataset.id);
    const sp = _speakers.find(s => s.id === id);
    if (!sp) return;
    const patch = async (body) => {
      try {
        await _fetchJson(`/api/voice/speakers/${id}`, { method: 'PATCH', body });
        Object.assign(sp, body);
      } catch (e) {
        console.error('speaker patch failed', e);
        showToast('Lautsprecher speichern fehlgeschlagen ❌', false);
      }
    };
    row.querySelector('input[data-field="name"]')?.addEventListener('change', (ev) => {
      const v = ev.target.value.trim(); if (v && v !== sp.name) patch({ name: v });
    });
    row.querySelector('input[data-field="group"]')?.addEventListener('change', (ev) => {
      const v = ev.target.value.trim(); if (v && v !== sp.group) patch({ group: v }).then(loadSpeakers);
    });
    const vol = row.querySelector('input[data-field="volume"]');
    const volLabel = row.querySelector('[data-volume-label]');
    if (vol) {
      vol.addEventListener('input', () => { if (volLabel) volLabel.textContent = vol.value; });
      vol.addEventListener('change', () => patch({ volume: Number(vol.value) }));
    }
    row.querySelectorAll('button[data-action]').forEach((btn) => {
      btn.addEventListener('click', async () => {
        const action = btn.dataset.action;
        try {
          if (action === 'delete') {
            if (!confirm(`Lautsprecher „${sp.name}“ aus der Liste entfernen? Ein laufendes Gerät meldet sich danach neu an.`)) return;
            await _fetchJson(`/api/voice/speakers/${id}`, { method: 'DELETE' });
            showToast('Lautsprecher entfernt ✅', true);
            return loadSpeakers();
          }
          if (action === 'mute') {
            await patch({ muted: !sp.muted });
            return loadSpeakers();
          }
          if (action === 'approve') {
            if (!confirm(`Lautsprecher „${sp.name}“ freigeben? Tipp: vorher mit 📍 prüfen, ob das richtige Gerät piept.`)) return;
            const r = await _fetchJson(`/api/voice/speakers/${id}/approve`, { method: 'POST', body: {} });
            showToast(r?.ok === false ? (r.error || 'Freigabe fehlgeschlagen ❌') : 'Lautsprecher freigegeben ✅', r?.ok !== false);
            return loadSpeakers();
          }
          if (action === 'revoke') {
            if (!confirm(`Freigabe für „${sp.name}“ entziehen? Das Gerät bekommt danach keinen Ton mehr, bis es neu freigegeben wird.`)) return;
            await _fetchJson(`/api/voice/speakers/${id}/revoke`, { method: 'POST', body: {} });
            showToast('Freigabe entzogen', true);
            return loadSpeakers();
          }
          if (action === 'reboot' && !confirm(`Lautsprecher „${sp.name}“ neu starten? Er ist danach einige Sekunden offline.`)) return;
          const cmd = { test: 'test_tone', identify: 'identify', reboot: 'reboot' }[action];
          const res = await _fetchJson(`/api/voice/speakers/${id}/cmd`, { method: 'POST', body: { cmd } });
          if (res?.ok === false) showToast(res.error || 'Gerät offline ❌', false);
          else showToast({ test: 'Testton gesendet ✅', identify: 'Gerät blinkt ✅', reboot: 'Neustart ausgelöst ✅' }[action], true);
        } catch (e) {
          console.error('speaker action failed', e);
          showToast('Aktion fehlgeschlagen ❌', false);
        }
      });
    });
  });
}

// Steht der Cursor in einem Feld der Tabelle (Name, Gruppe, Lautstärke), darf die
// 5-s-Aktualisierung die Zeilen nicht neu aufbauen – sonst gehen Eingaben verloren.
function speakersTableBusy() {
  const tbody = document.getElementById('speakersTableBody');
  const active = document.activeElement;
  return !!(tbody && active && active.tagName === 'INPUT' && tbody.contains(active));
}

async function loadSpeakers() {
  const info = document.getElementById('speakersActiveInfo');
  try {
    const res = await _fetchJson('/api/voice/speakers', { method: 'GET' });
    const online = (Array.isArray(res?.speakers) ? res.speakers : []).filter(s => s.online).length;
    if (info) info.textContent = `${online}/${res?.speakers?.length ?? 0} online` + (res?.active_group ? ` · Durchsage läuft (${res.active_group})` : '');
    if (speakersTableBusy()) return;
    _speakers = Array.isArray(res?.speakers) ? res.speakers : [];
    renderSpeakersTable();
  } catch (e) {
    if (e?.status !== 401) console.error('speakers load failed', e);
    if (info) info.textContent = e?.status === 401 ? 'Login erforderlich' : 'nicht erreichbar';
  }
}

(function bindSpeakersOnce() {
  const section = document.getElementById('speakersSection');
  if (!section || section.dataset.bound === '1') return;
  section.dataset.bound = '1';
  document.getElementById('speakersRefreshBtn')?.addEventListener('click', loadSpeakers);
  loadSpeakers();
  _speakersTimer = setInterval(() => { if (document.visibilityState === 'visible') loadSpeakers(); }, 5000);
})();
