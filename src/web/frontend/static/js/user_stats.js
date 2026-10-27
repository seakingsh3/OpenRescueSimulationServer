const statusEl = document.getElementById('userStatsStatus');
const bodyEl = document.getElementById('userStatsBody');
const refreshBtn = document.getElementById('userStatsRefresh');
const adminSection = document.getElementById('adminUserStatsSection');
const identityEl = document.getElementById('userStatsIdentity');

const statMarkers = document.getElementById('statMarkers');
const statMarkersSub = document.getElementById('statMarkersSub');
const statSims = document.getElementById('statSims');
const statSimsSub = document.getElementById('statSimsSub');
const statDebriefs = document.getElementById('statDebriefs');
const statDebriefsSub = document.getElementById('statDebriefsSub');
const statLastActive = document.getElementById('statLastActive');
const statLastActiveSub = document.getElementById('statLastActiveSub');
const progMarkers = document.getElementById('progMarkers');
const progMarkersLabel = document.getElementById('progMarkersLabel');
const progSims = document.getElementById('progSims');
const progSimsLabel = document.getElementById('progSimsLabel');
const progDebrief = document.getElementById('progDebrief');
const progDebriefLabel = document.getElementById('progDebriefLabel');
const achievementsGrid = document.getElementById('userAchievementsGrid');
const adminStatsMeta = document.getElementById('adminStatsMeta');
const adminStatsPrev = document.getElementById('adminStatsPrev');
const adminStatsNext = document.getElementById('adminStatsNext');

const COUNT_THRESHOLDS = [25, 50, 100, 200, 300, 400, 500, 1000];
let adminPage = 1;
let adminTotalPages = 1;

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

function latestActivity(stats) {
  const candidates = [
    stats?.last_marker_at,
    stats?.last_sim_start_at,
    stats?.last_sim_stop_at,
    stats?.last_debrief_at,
  ].filter(Boolean);
  if (!candidates.length) return '—';
  return fmtTs(Math.max(...candidates));
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

function nextThreshold(value) {
  for (const t of COUNT_THRESHOLDS) {
    if (value < t) return t;
  }
  return COUNT_THRESHOLDS[COUNT_THRESHOLDS.length - 1];
}

function setProgress(el, labelEl, value, suffix = '') {
  if (!el || !labelEl) return;
  const next = nextThreshold(value);
  const current = Math.min(value, next);
  const pct = next ? Math.round((current / next) * 100) : 0;
  el.value = pct;
  labelEl.textContent = `${current}/${next}${suffix ? ` ${suffix}` : ''}`;
}

function renderUserStats(user, stats) {
  if (!stats) return;
  if (identityEl) {
    const role = user?.role ? user.role.toUpperCase() : 'USER';
    identityEl.textContent = `${user?.username || 'Unbekannt'} • ${role}`;
  }
  if (statMarkers) statMarkers.textContent = stats.markers_total ?? 0;
  if (statMarkersSub) statMarkersSub.textContent = `${stats.markers_text_total ?? 0} mit Text`;
  if (statSims) statSims.textContent = stats.sim_starts_total ?? 0;
  if (statSimsSub) statSimsSub.textContent = 'Starts gesamt';
  if (statDebriefs) statDebriefs.textContent = stats.debriefs_total ?? 0;
  if (statDebriefsSub) statDebriefsSub.textContent = 'Debriefings gesamt';

  const last = latestActivity(stats);
  if (statLastActive) statLastActive.textContent = last;
  if (statLastActiveSub) statLastActiveSub.textContent = 'Zuletzt aktiv';

  setProgress(progMarkers, progMarkersLabel, stats.markers_total ?? 0, 'Marker');
  setProgress(progSims, progSimsLabel, stats.sim_starts_total ?? 0, 'Starts');
  setProgress(progDebrief, progDebriefLabel, stats.debriefs_total ?? 0, 'Debriefs');
}

function renderUserAchievements(items) {
  if (!achievementsGrid) return;
  if (!Array.isArray(items) || !items.length) {
    achievementsGrid.innerHTML = `<div class="text-sm text-gray-500">Noch keine Achievements erreicht.</div>`;
    return;
  }
  achievementsGrid.innerHTML = items.map((a) => `
    <div class="card bg-base-200/80 border border-base-300 shadow-md">
      <div class="card-body space-y-1">
        <div class="text-xs uppercase text-sky-300 tracking-wide">Achievement</div>
        <div class="text-lg font-bold text-white">🏆 ${escapeHtml(a.name || a.key)}</div>
        <div class="text-xs text-gray-400">${escapeHtml(a.description || '')}</div>
        <div class="text-xs text-gray-500">+${a.points ?? 0} XP • ${escapeHtml(fmtTs(a.unlocked_at))}</div>
      </div>
    </div>
  `).join('');
}

function renderRows(users) {
  if (!Array.isArray(users) || !users.length) {
    bodyEl.innerHTML = `<tr><td colspan="7" class="text-gray-500">Keine Daten vorhanden.</td></tr>`;
    return;
  }
  bodyEl.innerHTML = users.map((u) => {
    const stats = u.stats || {};
    return `
      <tr>
        <td class="font-semibold">${escapeHtml(u.username || `#${u.user_id}`)}</td>
        <td class="text-xs uppercase">${escapeHtml(u.role || 'user')}</td>
        <td>${stats.markers_total ?? 0}</td>
        <td>${stats.markers_text_total ?? 0}</td>
        <td>${stats.sim_starts_total ?? 0}</td>
        <td>${stats.debriefs_total ?? 0}</td>
        <td class="text-xs">${escapeHtml(latestActivity(stats))}</td>
      </tr>
    `;
  }).join('');
}

async function loadMyStats() {
  const me = await apiGet('/auth/me');
  const statsRes = await apiGet(`/api/users/${encodeURIComponent(me.user_id ?? me.id)}/stats`);
  renderUserStats(me, statsRes?.stats || {});
  const achRes = await apiGet(`/api/users/${encodeURIComponent(me.user_id ?? me.id)}/achievements`);
  const achieved = (achRes?.achievements || []).filter((a) => a.unlocked_at);
  renderUserAchievements(achieved);
  return me;
}

async function loadStats() {
  if (!statusEl || !bodyEl) return;
  statusEl.textContent = 'Lade Nutzerstatistiken…';
  try {
    const me = await loadMyStats();
    const isAdmin = (me?.role || '').toLowerCase() === 'admin';
    if (isAdmin) {
      if (adminSection) adminSection.classList.remove('hidden');
      const data = await apiGet(`/api/admin/users/stats?page=${adminPage}&page_size=25`);
      renderRows(data?.users || []);
      adminTotalPages = Math.max(1, Number(data?.total_pages || 1));
      if (adminStatsMeta) {
        const total = Number(data?.total || 0);
        adminStatsMeta.textContent = `Seite ${data?.page || adminPage} von ${adminTotalPages} • ${total} Nutzer`;
      }
      if (adminStatsPrev) adminStatsPrev.disabled = adminPage <= 1;
      if (adminStatsNext) adminStatsNext.disabled = adminPage >= adminTotalPages;
    } else {
      if (adminSection) adminSection.classList.add('hidden');
    }
    statusEl.textContent = `Aktualisiert: ${new Date().toLocaleTimeString('de-DE')}`;
  } catch (e) {
    if (e.status === 401 || e.status === 403) {
      statusEl.textContent = 'Nicht angemeldet oder keine Berechtigung.';
      bodyEl.innerHTML = `<tr><td colspan="7" class="text-gray-500">Nicht berechtigt.</td></tr>`;
      return;
    }
    console.error('loadStats failed', e);
    statusEl.textContent = 'Fehler beim Laden der Statistiken.';
  }
}

if (refreshBtn) {
  refreshBtn.addEventListener('click', (e) => {
    e.preventDefault();
    loadStats();
  });
}

if (adminStatsPrev) {
  adminStatsPrev.addEventListener('click', (e) => {
    e.preventDefault();
    if (adminPage > 1) {
      adminPage -= 1;
      loadStats();
    }
  });
}

if (adminStatsNext) {
  adminStatsNext.addEventListener('click', (e) => {
    e.preventDefault();
    if (adminPage < adminTotalPages) {
      adminPage += 1;
      loadStats();
    }
  });
}

loadStats();
