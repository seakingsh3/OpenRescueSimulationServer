// === tn_sheet.js ====================================================
// Teilnehmer-Notizblatt: lädt/speichert das Formular per Token-API,
// fragt bei neuer Simulation nach Verknüpfung, bietet Reset, QR und Druck.
(function () {
  'use strict';

  const body = document.body;
  const TOKEN = body.dataset.token || '';
  const MODE = body.dataset.mode === 'print' ? 'print' : 'edit';
  const API = `/api/tn/${TOKEN}`;
  const DECLINED_KEY = `ors_tn_declined_${TOKEN}`;
  const POLL_MS = 4000;

  const $ = (s) => document.querySelector(s);
  const $$ = (s) => Array.from(document.querySelectorAll(s));

  let sheet = null;          // letzter Serverstand
  let lastSaved = {};        // zuletzt gespeicherte Daten (Feld → Wert)
  let dirty = false;
  let saving = false;
  let pollTimer = null;
  let promptedSession = null; // Session-ID, für die das Popup gerade offen ist

  // ---------- Helpers ----------
  function setStatusBadge(cls, text) {
    const st = $('#tbStatus');
    if (!st) return;
    st.innerHTML = `<span class="badge ${cls}">${escapeHtml(text)}</span>`;
  }
  function flashSave(text, ok = true) {
    const st = $('#tbStatus');
    if (!st) return;
    const b = document.createElement('span');
    b.className = `badge ${ok ? 'badge-save' : 'badge-err'}`;
    b.textContent = text;
    st.appendChild(b);
    setTimeout(() => b.remove(), ok ? 1500 : 4000);
  }
  function escapeHtml(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    }[c]));
  }
  function fmtDate(ts) {
    if (!ts) return '';
    try { return new Date(ts * 1000).toLocaleString('de-DE'); } catch { return ''; }
  }
  async function api(path, init = {}) {
    const res = await fetch(API + path, {
      ...init,
      headers: { 'Content-Type': 'application/json', ...(init.headers || {}) },
      cache: 'no-store',
    });
    if (!res.ok) {
      const err = new Error(`${init.method || 'GET'} ${path} → ${res.status}`);
      err.status = res.status;
      throw err;
    }
    const ct = res.headers.get('content-type') || '';
    return ct.includes('application/json') ? res.json() : res.text();
  }

  // ---------- Formular <-> Daten ----------
  function fieldEls() {
    return $$('[data-field]');
  }
  function choiceGroups() {
    return $$('[data-choice-group]');
  }
  function collect() {
    const data = {};
    fieldEls().forEach((el) => { data[el.dataset.field] = el.value || ''; });
    choiceGroups().forEach((g) => { data[g.dataset.choiceGroup] = g.dataset.value || ''; });
    return data;
  }
  function apply(data) {
    const d = data || {};
    fieldEls().forEach((el) => { el.value = d[el.dataset.field] || ''; });
    choiceGroups().forEach((g) => setChoice(g, d[g.dataset.choiceGroup] || '', false));
  }
  function setChoice(group, value, fire) {
    group.dataset.value = value || '';
    group.querySelectorAll('[data-choice]').forEach((btn) => {
      const cls = btn.dataset.activeClass || 'active';
      btn.classList.toggle(cls, !!value && btn.dataset.choice === value);
    });
    if (fire) onFieldChanged(group.dataset.choiceGroup, group.dataset.label || group.dataset.choiceGroup, value || '');
  }
  function labelFor(field) {
    const el = document.querySelector(`[data-field="${field}"]`) || document.querySelector(`[data-choice-group="${field}"]`);
    return el?.dataset?.label || field;
  }

  // ---------- Speichern ----------
  let pendingChanges = new Map(); // field → {field,label,value}
  let saveTimer = null;

  function onFieldChanged(field, label, value) {
    if (MODE === 'print') return;
    const prev = lastSaved[field] || '';
    if ((value || '') === prev) return;
    pendingChanges.set(field, { field, label, value: value || '' });
    dirty = true;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => save().catch(() => {}), 350);
  }

  async function save(opts = {}) {
    if (MODE === 'print') return;
    if (saving && !opts.force) { dirty = true; return; }
    saving = true;
    const data = collect();
    const changes = Array.from(pendingChanges.values());
    pendingChanges = new Map();
    try {
      const res = await api('/data', { method: 'PUT', body: JSON.stringify({ data, changes }) });
      lastSaved = data;
      dirty = false;
      if (res?.markers_created) flashSave(`✓ gespeichert · ${res.markers_created} Marker`);
      else flashSave('✓ gespeichert');
    } catch (e) {
      // Änderungen behalten, später erneut versuchen
      changes.forEach((c) => pendingChanges.set(c.field, c));
      dirty = true;
      flashSave('Speichern fehlgeschlagen', false);
      console.error('save failed', e);
    } finally {
      saving = false;
      if (dirty && pendingChanges.size) {
        clearTimeout(saveTimer);
        saveTimer = setTimeout(() => save().catch(() => {}), 2000);
      }
    }
  }

  function bindFields() {
    fieldEls().forEach((el) => {
      if (MODE === 'print') { el.readOnly = true; el.tabIndex = -1; return; }
      el.addEventListener('change', () => onFieldChanged(el.dataset.field, el.dataset.label, el.value));
      // Tastatur-Enter in einzeiligen Feldern: zum nächsten Feld springen
      if (el.tagName === 'INPUT') {
        el.addEventListener('keydown', (ev) => {
          if (ev.key === 'Enter') {
            ev.preventDefault();
            const all = fieldEls();
            const i = all.indexOf(el);
            if (i >= 0 && all[i + 1]) all[i + 1].focus();
            else el.blur();
          }
        });
      }
    });
    choiceGroups().forEach((g) => {
      g.querySelectorAll('[data-choice]').forEach((btn) => {
        if (MODE === 'print') { btn.disabled = true; return; }
        btn.addEventListener('click', () => {
          const next = g.dataset.value === btn.dataset.choice ? '' : btn.dataset.choice;
          setChoice(g, next, true);
        });
      });
    });
  }

  // ---------- Kopf/Status ----------
  function renderHeader() {
    if (!sheet) return;
    const title = $('#tbTitle');
    if (title) title.textContent = `Notizblatt: ${sheet.name}`;
    const nameLbl = $('#sheetNameLabel');
    if (nameLbl) nameLbl.textContent = sheet.name;
    const dateLbl = $('#sheetDateLabel');
    if (dateLbl) dateLbl.textContent = sheet.updated_at ? `Stand ${fmtDate(sheet.updated_at)}` : fmtDate(sheet.created_at);
    document.title = `Notizblatt ${sheet.name} – Open Rescue Simulation Server`;

    const unlinkBtn = $('#btnUnlink');
    if (sheet.session_id && sheet.linked_active) {
      setStatusBadge('badge-linked', `verknüpft mit „${sheet.session_name || ('Session ' + sheet.session_id)}“`);
      if (unlinkBtn) unlinkBtn.style.display = MODE === 'edit' ? '' : 'none';
    } else if (sheet.session_id) {
      setStatusBadge('badge-ended', `„${sheet.session_name || ('Session ' + sheet.session_id)}“ beendet`);
      if (unlinkBtn) unlinkBtn.style.display = 'none';
    } else {
      setStatusBadge('badge-free', 'nicht verknüpft');
      if (unlinkBtn) unlinkBtn.style.display = 'none';
    }
  }

  // ---------- Session-Popup ----------
  function declinedSessionId() {
    try { return Number(localStorage.getItem(DECLINED_KEY) || 0) || 0; } catch { return 0; }
  }
  function rememberDeclined(id) {
    try { localStorage.setItem(DECLINED_KEY, String(id)); } catch {}
  }
  function maybePromptSession() {
    if (MODE === 'print' || !sheet) return;
    const active = sheet.active_session;
    if (!active || !active.id) { closeSessionModal(); return; }
    if (sheet.session_id === active.id) { closeSessionModal(); return; }
    if (declinedSessionId() === active.id) return;
    if (promptedSession === active.id) return;
    promptedSession = active.id;
    const txt = $('#sessionModalText');
    if (txt) {
      txt.textContent = `Simulation „${active.name || ('Session ' + active.id)}“ läuft. Soll das Notizblatt „${sheet.name}“ damit verknüpft werden? Deine Einträge erscheinen dann live in der Marker-Liste des Ausbilders.`;
    }
    $('#sessionModal')?.classList.add('open');
  }
  function closeSessionModal() {
    $('#sessionModal')?.classList.remove('open');
    promptedSession = null;
  }
  async function linkToActive() {
    const active = sheet?.active_session;
    if (!active?.id) return closeSessionModal();
    try {
      await api('/link', { method: 'POST', body: JSON.stringify({ session_id: active.id }) });
      closeSessionModal();
      await load(true);
      flashSave('✓ verknüpft');
    } catch (e) {
      closeSessionModal();
      flashSave(e.status === 409 ? 'Session läuft nicht mehr' : 'Verknüpfen fehlgeschlagen', false);
    }
  }

  // ---------- Laden / Polling ----------
  async function load(silent) {
    try {
      const fresh = await api('');
      const prevUpdated = sheet?.updated_at || 0;
      const firstLoad = !sheet;
      sheet = fresh;
      const focusInField = document.activeElement && (document.activeElement.matches?.('[data-field]'));
      if (firstLoad || (!dirty && !focusInField && (fresh.updated_at || 0) > prevUpdated)) {
        apply(fresh.data);
        lastSaved = collect();
      }
      renderHeader();
      maybePromptSession();
      $('#errorPage').style.display = 'none';
      $('#sheetPage').style.display = '';
    } catch (e) {
      if (e.status === 404) {
        $('#errorPage').style.display = '';
        $('#sheetPage').style.display = 'none';
        $('#tbTitle').textContent = 'Notizblatt nicht gefunden';
        setStatusBadge('badge-err', 'ungültiger Link');
        stopPoll();
      } else if (!silent) {
        flashSave('Verbindung fehlgeschlagen', false);
      }
    }
  }
  function startPoll() {
    stopPoll();
    pollTimer = setInterval(() => load(true).catch(() => {}), POLL_MS);
  }
  function stopPoll() {
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = null;
  }

  // ---------- QR / Download ----------
  function downloadUrl() {
    return `${location.origin}/tn/${TOKEN}/download`;
  }
  function showQr() {
    const url = downloadUrl();
    const c = $('#qrContainer');
    if (c) c.innerHTML = `<img alt="QR-Code" src="/api/qr?text=${encodeURIComponent(url)}">`;
    const l = $('#qrLink');
    if (l) l.textContent = url;
    $('#qrModal')?.classList.add('open');
  }

  // Eigenständige HTML-Datei mit eingetragenen Werten erzeugen (ohne Skripte/Toolbar)
  function buildStandaloneHtml() {
    const doc = document.documentElement.cloneNode(true);
    // aktuelle Werte in Attribute/Text schreiben
    const liveFields = fieldEls();
    doc.querySelectorAll('[data-field]').forEach((el, i) => {
      const live = liveFields[i];
      const v = live ? live.value : '';
      if (el.tagName === 'TEXTAREA') el.textContent = v;
      else el.setAttribute('value', v);
      el.setAttribute('readonly', 'readonly');
    });
    doc.querySelectorAll('[data-choice]').forEach((b) => b.setAttribute('disabled', 'disabled'));
    doc.querySelectorAll('script, .toolbar, .modal-bg, #errorPage').forEach((n) => n.remove());
    doc.querySelector('body')?.classList.add('mode-print');
    const meta = doc.createElement('meta');
    meta.setAttribute('name', 'generator');
    meta.setAttribute('content', 'Open Rescue Simulation Server Teilnehmer-Notizblatt');
    doc.querySelector('head')?.appendChild(meta);
    return '<!DOCTYPE html>\n' + doc.outerHTML;
  }
  function saveHtml() {
    const html = buildStandaloneHtml();
    const blob = new Blob([html], { type: 'text/html;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    const safe = (sheet?.name || 'Notizblatt').replace(/[^\w\-äöüÄÖÜß ]+/g, '_').trim() || 'Notizblatt';
    a.href = url;
    a.download = `Notizblatt_${safe}.html`;
    document.body.appendChild(a);
    a.click();
    setTimeout(() => { a.remove(); URL.revokeObjectURL(url); }, 1500);
  }

  // ---------- Reset ----------
  async function doReset() {
    try {
      await api('/reset', { method: 'POST' });
      pendingChanges = new Map();
      dirty = false;
      apply({});
      lastSaved = collect();
      $('#resetModal')?.classList.remove('open');
      await load(true);
      flashSave('✓ zurückgesetzt');
    } catch (e) {
      $('#resetModal')?.classList.remove('open');
      flashSave('Zurücksetzen fehlgeschlagen', false);
    }
  }

  // ---------- Bindings ----------
  function bindToolbar() {
    $('#btnReset')?.addEventListener('click', () => $('#resetModal')?.classList.add('open'));
    $('#resetYes')?.addEventListener('click', () => doReset());
    $('#resetNo')?.addEventListener('click', () => $('#resetModal')?.classList.remove('open'));

    $('#btnQr')?.addEventListener('click', showQr);
    $('#qrClose')?.addEventListener('click', () => $('#qrModal')?.classList.remove('open'));

    $('#btnPrint')?.addEventListener('click', () => window.print());
    $('#btnSaveHtml')?.addEventListener('click', saveHtml);

    $('#btnUnlink')?.addEventListener('click', async () => {
      try {
        await api('/unlink', { method: 'POST' });
        if (sheet?.active_session?.id) rememberDeclined(sheet.active_session.id);
        await load(true);
        flashSave('Verknüpfung gelöst');
      } catch { flashSave('Lösen fehlgeschlagen', false); }
    });

    $('#sessionLinkYes')?.addEventListener('click', linkToActive);
    $('#sessionLinkNo')?.addEventListener('click', () => {
      if (sheet?.active_session?.id) rememberDeclined(sheet.active_session.id);
      closeSessionModal();
    });

    $$('.modal-bg').forEach((m) => m.addEventListener('click', (e) => {
      if (e.target === m && m.id !== 'sessionModal') m.classList.remove('open');
    }));

    if (MODE === 'print') {
      $('#btnReset').style.display = 'none';
      $('#btnQr').style.display = 'none';
      $('#btnUnlink').style.display = 'none';
      $('#btnSaveHtml').style.display = '';
      const edit = $('#btnEdit');
      if (edit) { edit.style.display = ''; edit.href = `/tn/${TOKEN}`; }
    }

    // Ungespeichertes beim Verlassen noch wegschicken (best effort)
    window.addEventListener('beforeunload', () => {
      if (MODE === 'print' || (!dirty && pendingChanges.size === 0)) return;
      try {
        const payload = JSON.stringify({ data: collect(), changes: Array.from(pendingChanges.values()) });
        fetch(API + '/data', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: payload, keepalive: true });
      } catch {}
    });
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden' && (dirty || pendingChanges.size)) save({ force: true }).catch(() => {});
    });
  }

  // ---------- Start ----------
  if (!TOKEN) {
    $('#errorPage').style.display = '';
    $('#sheetPage').style.display = 'none';
    $('#tbTitle').textContent = 'Notizblatt nicht gefunden';
    return;
  }
  bindFields();
  bindToolbar();
  load(false).then(() => { if (MODE === 'edit') startPoll(); else stopPoll(); });
})();
