import { authFetch, withAuthHeaders } from '/static/js/http.js';
const fmtPercent = (n) => `${Number(n ?? 0).toFixed(0)}%`;
const fmtBytes = (b) => {
  if (b == null || isNaN(b)) return '-';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  let val = b;
  while (val >= 1024 && i < units.length - 1) { val /= 1024; i++; }
  return `${val.toFixed(1)} ${units[i]}`;
};
const escapeHtml = (s) => String(s || '').replace(/[&<>"]/g, (c) => ({
  '&': '&amp;',
  '<': '&lt;',
  '>': '&gt;',
  '"': '&quot;',
}[c]));
// Legacy helper kept for existing call sites; implementation comes from shared http.js.
const fmtDateTime = (sec) => {
  if (!Number.isFinite(Number(sec))) return '-';
  return new Date(Number(sec) * 1000).toLocaleString();
};

async function fetchStats() {
  const res = await authFetch('/api/host/stats', {
    cache: 'no-store',
    headers: withAuthHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function fetchMe() {
  const res = await authFetch('/auth/me', {
    cache: 'no-store',
    headers: withAuthHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function fetchLogFiles() {
  const res = await authFetch('/api/admin/logs/files', {
    cache: 'no-store',
    headers: withAuthHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function fetchNutzerinfo() {
  const res = await authFetch('/api/ui/nutzerinfo', {
    cache: 'no-store',
    headers: withAuthHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function fetchAdminSettings() {
  const res = await authFetch('/api/admin/settings', {
    cache: 'no-store',
    headers: withAuthHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function fetchUiLogMailTo() {
  const res = await authFetch('/api/ui/log_mail_to', {
    cache: 'no-store',
    headers: withAuthHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

function setBar(el, value) {
  if (!el) return;
  const v = Math.max(0, Math.min(100, Number(value) || 0));
  el.value = v;
}

function setText(id, val) {
  const el = document.getElementById(id);
  if (el) el.textContent = val;
}

function setHtml(id, val) {
  const el = document.getElementById(id);
  if (el) el.innerHTML = val;
}

const fmtUptime = (secRaw) => {
  const s = Math.max(0, Math.floor(secRaw || 0));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  return `${d}d ${h}h ${m}m`;
};

const overlay = document.getElementById('systemReconnectOverlay');
const overlayReason = document.getElementById('systemReconnectReason');
const overlayCountdown = document.getElementById('systemReconnectCountdown');
let connectionLost = false;
let restartCountdownTimer = null;
let restartCountdownLeft = 0;
let restartForcingOverlay = false;

function showOverlay(reason, countdownSec = null) {
  if (overlay) overlay.classList.remove('hidden');
  if (overlayReason) overlayReason.textContent = reason || 'Bitte warten, die Verbindung wird wiederhergestellt…';
  if (overlayCountdown) {
    if (countdownSec != null) {
      overlayCountdown.classList.remove('hidden');
      overlayCountdown.textContent = `${countdownSec}`;
    } else {
      overlayCountdown.classList.add('hidden');
      overlayCountdown.textContent = '';
    }
  }
}

function hideOverlay() {
  if (restartForcingOverlay) return;
  if (overlay) overlay.classList.add('hidden');
  if (overlayCountdown) overlayCountdown.classList.add('hidden');
}

function setConnectionLost(on) {
  connectionLost = on;
  if (on) {
    showOverlay('Verbindung zum Server verloren. Bitte warten…');
  } else if (!restartForcingOverlay) {
    hideOverlay();
  }
}

function startRestartCountdown(seconds) {
  if (restartCountdownTimer) clearInterval(restartCountdownTimer);
  restartCountdownLeft = seconds;
  restartForcingOverlay = true;
  showOverlay('Systemdienst wird neu gestartet…', restartCountdownLeft);
  restartCountdownTimer = setInterval(() => {
    restartCountdownLeft = Math.max(0, restartCountdownLeft - 1);
    showOverlay('Systemdienst wird neu gestartet…', restartCountdownLeft);
    if (restartCountdownLeft <= 0) {
      clearInterval(restartCountdownTimer);
      restartCountdownTimer = null;
      restartForcingOverlay = false;
      if (!connectionLost) hideOverlay();
    }
  }, 1000);
}

function updateUI(data) {
  const err = document.getElementById('systemError');
  if (err) err.classList.add('hidden');
  setConnectionLost(false);

  setText('serverVersion', data.server_version || '-');
  setText('cpuUsage', fmtPercent(data.cpu_avg));
  setText('cpuUsageDetail', fmtPercent(data.cpu_avg));
  setBar(document.getElementById('cpuBar'), data.cpu_avg);
  const loadAvgStr = [data.load_one, data.load_five, data.load_fifteen]
    .map(v => (v != null ? Number(v).toFixed(2) : '-'))
    .join(' / ');
  setText('loadAvg', loadAvgStr);

  const memTotal = data.mem_total ?? 0;
  const memUsed = data.mem_used ?? 0;
  const memFree = Math.max(memTotal - memUsed, 0);
  const memPct = memTotal ? (memUsed / memTotal) * 100 : 0;
  setText('memUsage', fmtPercent(memPct));
  setText('memUsageDetail', fmtPercent(memPct));
  setText('memFree', `${fmtBytes(memFree)} frei von ${fmtBytes(memTotal)}`);
  setBar(document.getElementById('memBar'), memPct);

  const disks = Array.isArray(data.disks) ? data.disks : [];
  const disk =
    disks.find((d) => d.mount === '/') ||
    disks.find((d) => d.total > 0) ||
    disks[0];
  if (disk) {
    const used = (disk.total ?? 0) - (disk.available ?? 0);
    const pct = disk.total ? (used / disk.total) * 100 : 0;
    setText('diskUsage', fmtPercent(pct));
    setText('diskUsageDetail', fmtPercent(pct));
    setText('diskPath', disk.mount ?? disk.name ?? '/');
    setBar(document.getElementById('diskBar'), pct);
  } else {
    setText('diskUsage', '-');
    setText('diskUsageDetail', '-');
    setText('diskPath', '-');
    setBar(document.getElementById('diskBar'), 0);
  }

  setText('uptime', fmtUptime(data.uptime_s));
  setText('threads', data.thread_count ?? '-');

  const nets = Array.isArray(data.net) ? data.net : [];
  const sum = nets.reduce(
    (acc, n) => ({
      rx: acc.rx + (n.rx_bytes ?? 0),
      tx: acc.tx + (n.tx_bytes ?? 0),
    }),
    { rx: 0, tx: 0 }
  );
  setText('netTx', fmtBytes(sum.tx));
  setText('netRx', fmtBytes(sum.rx));

  const list = nets
    .map((n) => {
      const rx = fmtBytes(n.rx_bytes);
      const tx = fmtBytes(n.tx_bytes);
      return `<div class="flex justify-between bg-base-300 rounded px-3 py-2">
        <span class="font-semibold">${n.name || 'iface'}</span>
        <span class="text-xs text-gray-500">Tx ${tx} / Rx ${rx}</span>
      </div>`;
    })
    .join('');
  setHtml('netList', list || '<div class="text-gray-500 text-sm">Keine Interfaces gefunden.</div>');

  const rtmp = data.rtmp || {};
  const state = rtmp.enabled ? (rtmp.state || 'connecting') : 'disabled';
  const stateLabel = {
    disabled: 'aus',
    connecting: 'verbindet…',
    streaming: 'sendet',
    error: 'Fehler',
  }[state] || state;
  setText('rtmpState', stateLabel);
  const pill = document.getElementById('rtmpState');
  if (pill) {
    pill.classList.toggle('ors-pill-ok', state === 'streaming');
    pill.classList.toggle('ors-pill-off', state !== 'streaming');
    pill.classList.toggle('text-error', state === 'error');
  }
  setText('rtmpUrl', rtmp.enabled ? (rtmp.url || '-') : 'kein Verteiler aktiv');
  const updated =
    rtmp.updated_at ? new Date(Number(rtmp.updated_at) * 1000).toLocaleString() : '-';
  setText('rtmpUpdated', updated);
  const errEl = document.getElementById('rtmpError');
  if (errEl) {
    if (rtmp.last_error) {
      errEl.textContent = rtmp.last_error;
      errEl.classList.remove('hidden');
    } else {
      errEl.textContent = '';
      errEl.classList.add('hidden');
    }
  }
}

async function loadStats() {
  try {
    const data = await fetchStats();
    updateUI(data || {});
  } catch (e) {
    console.error('System stats failed', e);
    const err = document.getElementById('systemError');
    if (String(e?.message || '').includes('401')) {
      if (err) {
        err.classList.remove('hidden');
        err.textContent = 'Bitte einloggen, um Systemdaten zu sehen.';
      }
      setConnectionLost(false);
      return;
    }
    if (err) err.classList.remove('hidden');
    setConnectionLost(true);
  }
}

const refreshBtn = document.getElementById('refreshSystemBtn');
if (refreshBtn) refreshBtn.addEventListener('click', () => {
  loadStats();
  reconnectLogs(true);
});

async function systemAction(url, label) {
  if (!confirm(`${label}?`)) return;
  const msg = document.getElementById('systemMessage');
  const show = (text, ok) => {
    if (!msg) return;
    msg.classList.remove('hidden', 'alert-success', 'alert-error');
    msg.classList.add(ok ? 'alert-success' : 'alert-error');
    msg.textContent = text;
  };
  try {
    const res = await authFetch(url, { method: 'POST', headers: withAuthHeaders() });
    const data = await res.json().catch(() => ({}));
    if (res.ok && data.ok !== false) {
      show(`${label} ausgelöst.`, true);
    } else {
      show(`Fehler: ${data.error || res.status}`, false);
    }
  } catch (e) {
    show('Aktion fehlgeschlagen.', false);
  }
}

const rebootBtn = document.getElementById('rebootBtn');
if (rebootBtn) rebootBtn.addEventListener('click', () => systemAction('/api/host/reboot', 'Neu starten'));

const poweroffBtn = document.getElementById('poweroffBtn');
if (poweroffBtn) poweroffBtn.addEventListener('click', () => systemAction('/api/host/poweroff', 'Herunterfahren'));

const restartServiceBtn = document.getElementById('restartServiceBtn');
if (restartServiceBtn) restartServiceBtn.addEventListener('click', async () => {
  startRestartCountdown(20);
  await systemAction('/api/host/service/restart', 'Systemdienst neu starten');
});

// Admin log files modal
const logFilesBtn = document.getElementById('systemLogFilesBtn');
const logFilesModal = document.getElementById('systemLogFilesModal');
const logFilesCloseBtn = document.getElementById('systemLogFilesCloseBtn');
const logFilesRefreshBtn = document.getElementById('systemLogFilesRefreshBtn');
const logFilesList = document.getElementById('systemLogFilesList');
const logMailToInput = document.getElementById('systemLogMailTo');
const logMailSaveBtn = document.getElementById('systemLogMailSaveBtn');
const quickMailBtn = document.getElementById('systemQuickMailBtn');
const changesBtn = document.getElementById('systemChangesBtn');
const changesModal = document.getElementById('systemChangesModal');
const changesCloseBtn = document.getElementById('systemChangesCloseBtn');
const changesContent = document.getElementById('systemChangesContent');
const logQrModal = document.getElementById('systemLogQrModal');
const logQrCloseBtn = document.getElementById('systemLogQrCloseBtn');
const logQrImg = document.getElementById('systemLogQrImg');
const logQrMeta = document.getElementById('systemLogQrMeta');
const logQrOnlyIssues = document.getElementById('systemLogQrOnlyIssues');
let isAdminUser = false;

function setLogFilesOpen(isOpen) {
  if (!logFilesModal) return;
  logFilesModal.classList.toggle('hidden', !isOpen);
  document.body.classList.toggle('modal-open', isOpen);
}

function renderLogFiles(items = []) {
  if (!logFilesList) return;
  if (!items.length) {
    logFilesList.innerHTML = '<div class="text-gray-500 text-xs">Keine Logdateien gefunden.</div>';
    return;
  }
  logFilesList.innerHTML = items
    .map((f) => {
      const name = String(f?.name || '');
      const encodedName = encodeURIComponent(name);
      const size = fmtBytes(f?.size_bytes ?? 0);
      const modified = fmtDateTime(f?.modified_at);
      const safeName = escapeHtml(name);
      return `<div class="flex items-center justify-between gap-3 rounded bg-base-300 px-3 py-2">
        <div class="min-w-0">
          <div class="font-mono text-xs truncate">${safeName}</div>
          <div class="text-[11px] text-gray-400">${size} • ${modified}</div>
        </div>
        <div class="flex items-center gap-2">
          <button class="btn btn-xs btn-outline system-log-download-btn" data-name="${encodedName}">Download</button>
          <button class="btn btn-xs btn-outline system-log-qr-btn" data-name="${encodedName}">📱 QR</button>
          <button class="btn btn-xs btn-primary system-log-mail-btn" data-name="${encodedName}">Per Mail</button>
        </div>
      </div>`;
    })
    .join('');
}

async function refreshLogFiles() {
  if (!logFilesList) return;
  logFilesList.innerHTML = '<div class="text-gray-500 text-xs">Lade Dateien...</div>';
  try {
    const data = await fetchLogFiles();
    const files = Array.isArray(data?.files) ? data.files : [];
    renderLogFiles(files);
  } catch (e) {
    console.error('log files failed', e);
    logFilesList.innerHTML = '<div class="text-error text-xs">Dateiliste konnte nicht geladen werden.</div>';
  }
}

async function downloadLogFile(name) {
  const fileName = String(name || '');
  if (!fileName) return;
  try {
    const res = await authFetch(`/api/admin/logs/files/${encodeURIComponent(fileName)}`, {
      headers: withAuthHeaders(),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = fileName;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  } catch (e) {
    console.error('log file download failed', e);
    if (typeof window.showUiToast === 'function') {
      window.showUiToast('Download fehlgeschlagen.', false);
    }
  }
}

async function fetchLogFileText(fileName) {
  const res = await authFetch(`/api/admin/logs/files/${encodeURIComponent(fileName)}`, {
    headers: withAuthHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return await res.text();
}

function getLogMailTo() {
  const raw = (logMailToInput?.value || '').trim();
  if (raw) return raw;
  try {
    return (localStorage.getItem('ors_log_mail_to') || '').trim();
  } catch (_) {
    return '';
  }
}

function saveLogMailTo() {
  const val = getLogMailTo();
  try {
    localStorage.setItem('ors_log_mail_to', val);
  } catch (_) {}
  const done = () => {
    if (typeof window.showUiToast === 'function') {
      window.showUiToast(val ? 'E-Mail-Adresse gespeichert.' : 'E-Mail-Adresse geleert.', true);
    }
  };
  if (!isAdminUser) {
    done();
    return;
  }
  authFetch('/api/admin/settings', {
    method: 'POST',
    headers: withAuthHeaders({ 'Content-Type': 'application/json' }),
    body: JSON.stringify({ log_mail_to: val }),
  })
    .then((res) => {
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      done();
    })
    .catch((e) => {
      console.error('save admin log mail failed', e);
      if (typeof window.showUiToast === 'function') {
        window.showUiToast('Server-Voreinstellung konnte nicht gespeichert werden.', false);
      }
    });
}

function restoreLogMailTo(defaultFromServer = '') {
  if (!logMailToInput) return;
  if (defaultFromServer) {
    logMailToInput.value = defaultFromServer;
    return;
  }
  try {
    const val = localStorage.getItem('ors_log_mail_to') || '';
    logMailToInput.value = val;
  } catch (_) {}
}

async function ensureLogMailTo() {
  let to = getLogMailTo();
  if (to) return to;
  try {
    const res = await fetchUiLogMailTo();
    to = String(res?.mail_to || '').trim();
    if (to) {
      if (logMailToInput) logMailToInput.value = to;
      try { localStorage.setItem('ors_log_mail_to', to); } catch (_) {}
    }
  } catch (_) {}
  return to;
}

async function openLogMailDraft(fileName) {
  const to = await ensureLogMailTo();
  if (!to) {
    if (typeof window.showUiToast === 'function') {
      window.showUiToast('Bitte zuerst eine E-Mail-Adresse eintragen.', false);
    }
    logMailToInput?.focus();
    return;
  }
  const safeName = String(fileName || '').trim();
  if (!safeName) return;
  let logExcerpt = '(kein Logtext verfuegbar)';
  let excerptNote = '';
  try {
    const fullText = await fetchLogFileText(safeName);
    const lines = String(fullText || '')
      .split(/\r?\n/)
      .filter((l) => l.trim().length > 0);
    const wantedLines = 300;
    const picked = lines.slice(-wantedLines);
    logExcerpt = picked.join('\n') || '(kein Logtext verfuegbar)';
  } catch (e) {
    console.warn('log text for mail failed', e);
    const rawLog = (logBox?.textContent || '').trim();
    if (rawLog) {
      const fallbackLines = rawLog.split(/\r?\n/).filter((l) => l.trim().length > 0);
      logExcerpt = fallbackLines.slice(-60).join('\n');
      excerptNote = '(Fallback: Live-Log aus UI, da Dateiabruf fehlgeschlagen)';
    }
  }

  // mailto URLs have practical size limits depending on OS/client
  const maxExcerptChars = 12000;
  if (logExcerpt.length > maxExcerptChars) {
    logExcerpt = logExcerpt.slice(logExcerpt.length - maxExcerptChars);
    excerptNote = excerptNote
      ? `${excerptNote}\n(gekürzt wegen Mail-Client-Limit)`
      : '(gekürzt wegen Mail-Client-Limit)';
  }

  const subject = `OpenRescueSim Logdatei: ${safeName}`;
  const downloadUrl = `${window.location.origin}/api/admin/logs/files/${encodeURIComponent(safeName)}`;
  const fullBody = [
    'Hallo,',
    '',
    `anbei die Logdatei: ${safeName}`,
    '',
    'Direkter Download (Admin-Login erforderlich):',
    downloadUrl,
    '',
    'Hinweis: Datei ggf. im Mail-Client manuell anhaengen.',
    '',
    'Aktueller Logauszug:',
    logExcerpt,
    excerptNote,
  ].join('\n');
  let bodyToUse = fullBody;
  let copiedToClipboard = false;
  const encodedFull = encodeURIComponent(fullBody);
  const maxMailtoBodyLen = 1800; // konservativ fuer viele Mail-Clients
  if (encodedFull.length > maxMailtoBodyLen) {
    try {
      if (navigator?.clipboard?.writeText) {
        await navigator.clipboard.writeText(logExcerpt);
        copiedToClipboard = true;
      }
    } catch (_) {}
    bodyToUse = [
      'Hallo,',
      '',
      `anbei die Logdatei: ${safeName}`,
      '',
      'Direkter Download (Admin-Login erforderlich):',
      downloadUrl,
      '',
      copiedToClipboard
        ? 'Logauszug wurde in die Zwischenablage kopiert. Bitte in die Mail einfuegen (Strg+V).'
        : 'Logauszug ist fuer mailto zu lang. Bitte Datei anhaengen oder manuell aus der UI kopieren.',
    ].join('\n');
    if (typeof window.showUiToast === 'function') {
      window.showUiToast(
        copiedToClipboard
          ? 'Logauszug war zu lang und wurde in die Zwischenablage kopiert.'
          : 'Logauszug war zu lang fuer mailto.',
        true
      );
    }
  }
  const href = `mailto:${encodeURIComponent(to)}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(bodyToUse)}`;
  window.location.href = href;
}

// -------- Log als QR-Code (keine Netzwerkverbindung des Empfängers nötig) ----
// Ein QR-Code fasst nur ca. 2-3 KB Daten, eine Logdatei kann mehrere MB groß
// werden -> es geht nur ein Auszug, der von hinten (neueste Zeilen) her
// byte-genau bis zu einem konservativen Budget aufgefüllt wird.
const LOG_QR_MAX_BYTES = 1800;
let _logQrState = { fileName: '', fullText: '' };

function buildLogQrExcerpt(fullText, { onlyIssues = false, maxBytes = LOG_QR_MAX_BYTES } = {}) {
  const allLines = String(fullText || '').split(/\r?\n/).filter((l) => l.trim().length > 0);
  const candidateLines = onlyIssues ? allLines.filter((l) => /\b(WARN|ERROR)\b/.test(l)) : allLines;
  const encoder = new TextEncoder();
  const picked = [];
  let bytes = 0;
  for (let i = candidateLines.length - 1; i >= 0; i--) {
    const line = candidateLines[i];
    const lineBytes = encoder.encode(i === candidateLines.length - 1 ? line : line + '\n').length;
    if (bytes + lineBytes > maxBytes) break;
    picked.unshift(line);
    bytes += lineBytes;
  }
  return {
    text: picked.join('\n'),
    includedLines: picked.length,
    candidateLines: candidateLines.length,
    totalLines: allLines.length,
    bytes,
    truncated: picked.length < candidateLines.length,
  };
}

function setLogQrOpen(isOpen) {
  if (!logQrModal) return;
  logQrModal.classList.toggle('hidden', !isOpen);
  document.body.classList.toggle('modal-open', isOpen);
}

function renderLogQr() {
  if (!logQrImg || !logQrMeta) return;
  if (!_logQrState.fullText) {
    logQrImg.classList.add('hidden');
    logQrMeta.textContent = 'Kein Inhalt geladen.';
    return;
  }
  const onlyIssues = !!logQrOnlyIssues?.checked;
  const { text, includedLines, candidateLines, totalLines, bytes, truncated } = buildLogQrExcerpt(
    _logQrState.fullText,
    { onlyIssues }
  );
  if (!text) {
    logQrImg.classList.add('hidden');
    logQrMeta.textContent = onlyIssues
      ? 'Keine Fehler/Warnungen in dieser Datei gefunden.'
      : 'Kein Inhalt verfügbar.';
    return;
  }
  logQrImg.src = `/api/qr?text=${encodeURIComponent(text)}`;
  logQrImg.classList.remove('hidden');
  const scope = onlyIssues
    ? `${includedLines} von ${candidateLines} Fehler/Warn-Zeilen (${totalLines} Zeilen insgesamt)`
    : `${includedLines} von ${totalLines} Zeilen`;
  logQrMeta.textContent =
    `${_logQrState.fileName}: ${scope} enthalten (~${bytes} Bytes).` +
    (truncated ? ' Gekürzt — nur die neuesten Zeilen passen in einen QR-Code.' : '');
}

async function openLogQrModal(fileName) {
  const safeName = String(fileName || '').trim();
  if (!safeName) return;
  setLogFilesOpen(false);
  _logQrState = { fileName: safeName, fullText: '' };
  setLogQrOpen(true);
  if (logQrImg) logQrImg.classList.add('hidden');
  if (logQrMeta) logQrMeta.textContent = 'Lade Logdatei…';
  try {
    const fullText = await fetchLogFileText(safeName);
    _logQrState = { fileName: safeName, fullText };
    renderLogQr();
  } catch (e) {
    console.error('log qr fetch failed', e);
    if (logQrMeta) logQrMeta.textContent = 'Logdatei konnte nicht geladen werden.';
  }
}

async function quickMailCurrentLog() {
  const to = await ensureLogMailTo();
  if (!to) {
    if (typeof window.showUiToast === 'function') {
      window.showUiToast('Keine Empfängeradresse hinterlegt.', false);
    }
    return;
  }
  const fullLog = logLines.join('\n').trim() || (logBox?.textContent || '').trim();
  if (!fullLog) {
    if (typeof window.showUiToast === 'function') {
      window.showUiToast('Kein Logtext verfügbar.', false);
    }
    return;
  }
  try {
    if (navigator?.clipboard?.writeText) {
      await navigator.clipboard.writeText(fullLog);
    }
  } catch (e) {
    console.warn('clipboard write failed', e);
  }
  const subject = `OpenRescueSim Logauszug ${new Date().toLocaleString('de-DE')}`;
  const body = [
    'Hallo,',
    '',
    'der aktuelle Logauszug wurde in die Zwischenablage kopiert.',
    'Bitte im Mail-Client mit Strg+V einfuegen.',
  ].join('\n');
  const href = `mailto:${encodeURIComponent(to)}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(body)}`;
  window.location.href = href;
  if (typeof window.showUiToast === 'function') {
    window.showUiToast('Aktuelles Log in die Zwischenablage kopiert.', true);
  }
}

if (logFilesBtn) {
  logFilesBtn.addEventListener('click', async () => {
    setLogFilesOpen(true);
    await refreshLogFiles();
  });
}
if (logFilesCloseBtn) logFilesCloseBtn.addEventListener('click', () => setLogFilesOpen(false));
if (logFilesRefreshBtn) logFilesRefreshBtn.addEventListener('click', refreshLogFiles);
if (logMailSaveBtn) logMailSaveBtn.addEventListener('click', saveLogMailTo);
if (quickMailBtn) quickMailBtn.addEventListener('click', () => { quickMailCurrentLog().catch(() => {}); });
if (logMailToInput) {
  logMailToInput.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') {
      ev.preventDefault();
      saveLogMailTo();
    }
  });
}
if (logFilesModal) {
  logFilesModal.addEventListener('click', (ev) => {
    if (ev.target === logFilesModal) setLogFilesOpen(false);
  });
}
if (logFilesList) {
  logFilesList.addEventListener('click', (ev) => {
    const btn = ev.target.closest('.system-log-download-btn');
    if (btn) {
      const name = decodeURIComponent(btn.getAttribute('data-name') || '');
      downloadLogFile(name);
      return;
    }
    const mailBtn = ev.target.closest('.system-log-mail-btn');
    if (mailBtn) {
      const name = decodeURIComponent(mailBtn.getAttribute('data-name') || '');
      openLogMailDraft(name);
      return;
    }
    const qrBtn = ev.target.closest('.system-log-qr-btn');
    if (qrBtn) {
      const name = decodeURIComponent(qrBtn.getAttribute('data-name') || '');
      openLogQrModal(name);
    }
  });
}
if (logQrCloseBtn) {
  logQrCloseBtn.addEventListener('click', () => setLogQrOpen(false));
}
if (logQrModal) {
  logQrModal.addEventListener('click', (ev) => {
    if (ev.target === logQrModal) setLogQrOpen(false);
  });
}
if (logQrOnlyIssues) {
  logQrOnlyIssues.addEventListener('change', renderLogQr);
}

async function initAdminLogFiles() {
  restoreLogMailTo();
  if (!logFilesBtn) return;
  try {
    const me = await fetchMe();
    if (me?.role === 'admin') {
      isAdminUser = true;
      logFilesBtn.classList.remove('hidden');
      try {
        const admin = await fetchAdminSettings();
        const serverMail = String(admin?.log_mail_to || '').trim();
        if (serverMail) {
          restoreLogMailTo(serverMail);
          try {
            localStorage.setItem('ors_log_mail_to', serverMail);
          } catch (_) {}
        }
      } catch (e) {
        console.warn('admin settings load failed', e);
      }
    }
  } catch (_) {
    // no auth/no admin: keep button hidden
  }
}

function markdownToHtml(md) {
  const lines = String(md || '').split(/\r?\n/);
  const out = [];
  let inList = false;
  for (const raw of lines) {
    const line = raw.trimEnd();
    if (!line.trim()) {
      if (inList) {
        out.push('</ul>');
        inList = false;
      }
      continue;
    }
    if (line.startsWith('# ')) {
      if (inList) { out.push('</ul>'); inList = false; }
      out.push(`<h2 class="text-2xl font-bold text-white mb-3">${escapeHtml(line.slice(2))}</h2>`);
      continue;
    }
    if (line.startsWith('## ')) {
      if (inList) { out.push('</ul>'); inList = false; }
      out.push(`<h3 class="text-lg font-semibold text-sky-300 mt-5 mb-2">${escapeHtml(line.slice(3))}</h3>`);
      continue;
    }
    if (line.startsWith('### ')) {
      if (inList) { out.push('</ul>'); inList = false; }
      out.push(`<h4 class="text-base font-semibold text-slate-100 mt-4 mb-1">${escapeHtml(line.slice(4))}</h4>`);
      continue;
    }
    if (line.startsWith('- ')) {
      if (!inList) {
        out.push('<ul class="list-disc pl-5 space-y-1">');
        inList = true;
      }
      out.push(`<li>${escapeHtml(line.slice(2))}</li>`);
      continue;
    }
    if (inList) {
      out.push('</ul>');
      inList = false;
    }
    out.push(`<p class="text-slate-200">${escapeHtml(line)}</p>`);
  }
  if (inList) out.push('</ul>');
  return out.join('');
}

function setChangesOpen(isOpen) {
  if (!changesModal) return;
  changesModal.classList.toggle('hidden', !isOpen);
  document.body.classList.toggle('modal-open', isOpen);
}

async function openChangesModal() {
  if (!changesContent) return;
  setChangesOpen(true);
  changesContent.innerHTML = '<div class="text-slate-400">Lade Updates…</div>';
  try {
    const res = await fetchNutzerinfo();
    changesContent.innerHTML = markdownToHtml(res?.markdown || '');
  } catch (e) {
    console.error('updates snapshot fetch failed', e);
    changesContent.innerHTML = '<div class="text-red-300">Updates konnten nicht geladen werden.</div>';
  }
}

// Live Logs via WebSocket (/ws/logs)
let logSocket = null;
let logReconnectTimer = null;
const logLines = [];
const MAX_LOG_LINES = 500;

const logBox = document.getElementById('systemLogs');
const logMeta = document.getElementById('systemLogsMeta');

function setLogStatus(text, isError = false) {
  if (logMeta) {
    logMeta.textContent = text;
    logMeta.classList.toggle('text-error', isError);
  }
}

function renderLogs() {
  if (!logBox) return;
  logBox.textContent = logLines.join('\n') || 'Keine Logeinträge.';
  logBox.scrollTop = logBox.scrollHeight;
}

function appendLogLine(raw) {
  if (!raw) return;
  const lines = String(raw).split(/\r?\n/);
  for (const line of lines) {
    if (!line) continue;
    logLines.push(line);
    if (logLines.length > MAX_LOG_LINES) logLines.shift();
  }
  renderLogs();
}

function closeLogSocket() {
  if (logSocket) {
    logSocket.onopen = null;
    logSocket.onmessage = null;
    logSocket.onclose = null;
    logSocket.onerror = null;
    logSocket.close();
    logSocket = null;
  }
}

function scheduleReconnect() {
  if (logReconnectTimer) return;
  logReconnectTimer = setTimeout(() => {
    logReconnectTimer = null;
    connectLogs();
  }, 2000);
}

function reconnectLogs(force = false) {
  if (force) closeLogSocket();
  if (force && logLines.length) {
    logLines.length = 0;
    renderLogs();
  }
  if (logReconnectTimer) {
    clearTimeout(logReconnectTimer);
    logReconnectTimer = null;
  }
  connectLogs();
}

function connectLogs() {
  if (logSocket) return;
  if (logBox) logBox.textContent = 'Verbinde zur Logdatei…';
  setLogStatus('Logdatei (verbinde…)');

  const proto = window.location.protocol === 'https:' ? 'wss' : 'ws';
  // Live-Logs nur für Admins: Token als Parameter (WebSockets können keinen Header setzen)
  const logToken = localStorage.getItem('access_token') || '';
  const ws = new WebSocket(`${proto}://${window.location.host}/ws/logs?token=${encodeURIComponent(logToken)}`);
  logSocket = ws;

  ws.onopen = () => {
    setLogStatus('Logdatei (live)');
    setConnectionLost(false);
  };

  ws.onmessage = (ev) => {
    appendLogLine(ev.data);
  };

  ws.onerror = () => {
    setLogStatus('Logdatei (Fehler, reconnect…)', true);
    scheduleReconnect();
    setConnectionLost(true);
  };

  ws.onclose = () => {
    logSocket = null;
    setLogStatus('Logdatei (getrennt, reconnect…)', true);
    scheduleReconnect();
    setConnectionLost(true);
  };
}

initAdminLogFiles();
if (changesBtn) changesBtn.addEventListener('click', openChangesModal);
if (changesCloseBtn) changesCloseBtn.addEventListener('click', () => setChangesOpen(false));
if (changesModal) {
  changesModal.addEventListener('click', (ev) => {
    if (ev.target === changesModal) setChangesOpen(false);
  });
}
loadStats();
connectLogs();
setInterval(loadStats, 10000);
