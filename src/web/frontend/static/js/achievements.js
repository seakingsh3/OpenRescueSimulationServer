const statusEl = document.getElementById('achievementsStatus');
const gridEl = document.getElementById('achievementsGrid');
const refreshBtn = document.getElementById('achievementsRefresh');

function escapeHtml(s) {
  return String(s || '').replace(/[&<>"]/g, (c) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
  }[c]));
}

function fmtTs(ts) {
  if (!ts) return '—';
  const d = new Date(ts * 1000);
  if (Number.isNaN(d.getTime())) return '—';
  return d.toLocaleString('de-DE');
}

async function apiGet(url) {
  const headers = {};
  const token = localStorage.getItem('access_token') || localStorage.getItem('token');
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(url, { headers, cache: 'no-store' });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    const err = new Error(`${res.status} ${res.statusText}`);
    err.status = res.status;
    err.payload = text;
    throw err;
  }
  return res.json();
}

function renderAchievements(items) {
  if (!Array.isArray(items) || !items.length) {
    gridEl.innerHTML = `
      <div class="card bg-base-200 shadow-lg border border-base-300">
        <div class="card-body text-gray-500">Noch keine Achievements erreicht.</div>
      </div>
    `;
    return;
  }
  gridEl.innerHTML = items.map((a) => `
    <div class="card bg-gradient-to-br from-slate-900/80 via-slate-800/50 to-slate-900/80 shadow-lg border border-slate-700/60">
      <div class="card-body space-y-2">
        <div class="flex items-center justify-between">
          <span class="text-xs uppercase text-sky-300 tracking-wide">Achievement</span>
          <span class="text-xs text-gray-400">${fmtTs(a.unlocked_at)}</span>
        </div>
        <div class="text-xl font-bold text-white">🏆 ${escapeHtml(a.name || a.key)}</div>
        <div class="text-sm text-gray-300">${escapeHtml(a.description || '')}</div>
        <div class="text-xs text-gray-400">+${a.points ?? 0} XP</div>
      </div>
    </div>
  `).join('');
}

async function loadAchievements() {
  statusEl.textContent = 'Lade Achievements…';
  try {
    const me = await apiGet('/auth/me');
    const res = await apiGet(`/api/users/${encodeURIComponent(me.user_id ?? me.id)}/achievements`);
    const achieved = (res?.achievements || []).filter((a) => a.unlocked_at);
    renderAchievements(achieved);
    statusEl.textContent = `Aktualisiert: ${new Date().toLocaleTimeString('de-DE')}`;
  } catch (e) {
    if (e.status === 401 || e.status === 403) {
      statusEl.textContent = 'Nicht angemeldet oder keine Berechtigung.';
      gridEl.innerHTML = `
        <div class="card bg-base-200 shadow-lg border border-base-300">
          <div class="card-body text-gray-500">Nicht berechtigt.</div>
        </div>
      `;
      return;
    }
    console.error('loadAchievements failed', e);
    statusEl.textContent = 'Fehler beim Laden der Achievements.';
  }
}

if (refreshBtn) {
  refreshBtn.addEventListener('click', (e) => {
    e.preventDefault();
    loadAchievements();
  });
}

loadAchievements();
