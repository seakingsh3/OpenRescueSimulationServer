const $ = (s) => document.querySelector(s);

function randHex(bytes = 32) {
  const arr = new Uint8Array(bytes);
  crypto.getRandomValues(arr);
  return Array.from(arr, (b) => b.toString(16).padStart(2, '0')).join('');
}

function setStatus(msg, ok = true) {
  const el = $('#setupStatus');
  if (!el) return;
  el.textContent = msg;
  el.classList.toggle('text-error', !ok);
  el.classList.toggle('text-success', ok);
}

function addMarkerTypeRow(name = '', color = '#ff0000') {
  const tbody = $('#markerTypesBody');
  if (!tbody) return;
  const tr = document.createElement('tr');
  tr.innerHTML = `
    <td><input class="input input-bordered w-full mt-name" type="text" value="${name}"></td>
    <td><input class="input input-bordered w-full mt-color" type="text" value="${color}"></td>
    <td><button type="button" class="btn btn-ghost btn-sm mt-remove">Entfernen</button></td>
  `;
  tr.querySelector('.mt-remove')?.addEventListener('click', () => tr.remove());
  tbody.appendChild(tr);
}

function addMarkerNoteRow(text = '') {
  const list = $('#markerNotesList');
  if (!list) return;
  const row = document.createElement('div');
  row.className = 'flex gap-2';
  row.innerHTML = `
    <input class="input input-bordered flex-1 note-text" type="text" value="${text}">
    <button type="button" class="btn btn-ghost btn-sm note-remove">Entfernen</button>
  `;
  row.querySelector('.note-remove')?.addEventListener('click', () => row.remove());
  list.appendChild(row);
}

function addCameraRow(cam = {}) {
  const tbody = $('#camerasBody');
  if (!tbody) return;
  const tr = document.createElement('tr');
  const name = cam.name || '';
  const url = cam.url || '';
  const fov = cam.fov_deg ?? '';
  const enabled = cam.enabled !== false;
  tr.innerHTML = `
    <td><input class="input input-bordered w-full cam-name" type="text" value="${name}"></td>
    <td><input class="input input-bordered w-full cam-url" type="text" value="${url}"></td>
    <td><input class="input input-bordered w-full cam-fov" type="number" step="0.1" value="${fov}"></td>
    <td><input class="toggle toggle-primary cam-enabled" type="checkbox" ${enabled ? 'checked' : ''}></td>
    <td><button type="button" class="btn btn-ghost btn-sm cam-remove">Entfernen</button></td>
  `;
  tr.querySelector('.cam-remove')?.addEventListener('click', () => tr.remove());
  tbody.appendChild(tr);
}

async function loadDefaults() {
  const res = await fetch('/api/setup/status');
  if (!res.ok) throw new Error('status failed');
  const data = await res.json();
  if (data.setup_done) {
    window.location.href = '/ui';
    return null;
  }
  return data.defaults || null;
}

function fillForm(d) {
  if (!d) return;
  $('#adminUsername').value = d.admin_username || 'admin';
  $('#authSecret').value = d.auth_secret || randHex(32);
  $('#recordEnabled').checked = !!d.settings?.record_enabled;
  $('#recordFormat').value = d.settings?.record_format || 'mkv';
  $('#recordSegmentSecs').value = d.settings?.record_segment_secs ?? 300;
  $('#recordMaxFiles').value = d.settings?.record_max_files ?? 0;
  $('#recordPrefix').value = d.settings?.record_prefix || 'rec';
  $('#recordDir').value = d.settings?.record_dir || '';
  $('#audioDevice').value = d.settings?.audio_device || 'default';

  (d.marker_types || []).forEach((m) => addMarkerTypeRow(m.name, m.color));
  (d.marker_notes || []).forEach((n) => addMarkerNoteRow(n));
  (d.cameras || []).forEach((c) => addCameraRow(c));
}

function collectMarkerTypes() {
  return Array.from(document.querySelectorAll('#markerTypesBody tr')).map((tr) => ({
    name: tr.querySelector('.mt-name')?.value?.trim() || '',
    color: tr.querySelector('.mt-color')?.value?.trim() || '',
  })).filter((m) => m.name);
}

function collectMarkerNotes() {
  return Array.from(document.querySelectorAll('#markerNotesList .note-text'))
    .map((el) => el.value.trim())
    .filter(Boolean);
}

function collectCameras() {
  return Array.from(document.querySelectorAll('#camerasBody tr')).map((tr) => {
    const fovRaw = tr.querySelector('.cam-fov')?.value;
    const fov = fovRaw === '' ? null : Number(fovRaw);
    return {
      name: tr.querySelector('.cam-name')?.value?.trim() || '',
      url: tr.querySelector('.cam-url')?.value?.trim() || '',
      fov_deg: Number.isNaN(fov) ? null : fov,
      enabled: !!tr.querySelector('.cam-enabled')?.checked,
    };
  }).filter((c) => c.name && c.url);
}

async function submitSetup(e) {
  e.preventDefault();
  const adminUsername = $('#adminUsername').value.trim();
  const adminPassword = $('#adminPassword').value.trim();
  const authSecret = $('#authSecret').value.trim();
  if (!adminUsername || !adminPassword || !authSecret) {
    setStatus('Bitte Admin, Passwort und Auth Secret ausfuellen.', false);
    return;
  }

  const payload = {
    admin_username: adminUsername,
    admin_password: adminPassword,
    auth_secret: authSecret,
    marker_types: collectMarkerTypes(),
    marker_notes: collectMarkerNotes(),
    cameras: collectCameras(),
    settings: {
      record_enabled: $('#recordEnabled').checked,
      record_format: $('#recordFormat').value,
      record_segment_secs: Number($('#recordSegmentSecs').value || 0),
      record_max_files: Number($('#recordMaxFiles').value || 0),
      record_prefix: $('#recordPrefix').value.trim(),
      record_dir: $('#recordDir').value.trim(),
      audio_device: $('#audioDevice').value.trim(),
    },
  };

  $('#setupSubmit').disabled = true;
  setStatus('Speichere Setup...', true);
  try {
    const res = await fetch('/api/setup/complete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error('setup failed');
    const data = await res.json();
    setStatus('Setup abgeschlossen. Weiterleitung...', true);
    if (data?.requires_restart) {
      setStatus('Setup gespeichert. Bitte Service neu starten.', true);
    }
    setTimeout(() => {
      window.location.href = '/ui';
    }, 1200);
  } catch (err) {
    setStatus('Setup fehlgeschlagen. Bitte pruefe deine Eingaben.', false);
  } finally {
    $('#setupSubmit').disabled = false;
  }
}

document.addEventListener('DOMContentLoaded', async () => {
  try {
    const defaults = await loadDefaults();
    if (!defaults) return;
    fillForm(defaults);
  } catch (_) {
    setStatus('Setup-Status konnte nicht geladen werden.', false);
  }

  $('#authSecretGen').addEventListener('click', () => {
    $('#authSecret').value = randHex(32);
  });
  $('#addMarkerType').addEventListener('click', () => addMarkerTypeRow('', '#ff0000'));
  $('#addMarkerNote').addEventListener('click', () => addMarkerNoteRow(''));
  $('#addCamera').addEventListener('click', () => addCameraRow({}));
  $('#setupForm').addEventListener('submit', submitSetup);
});
