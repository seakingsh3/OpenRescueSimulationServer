// Raumlautsprecher: Gruppen, Lautsprecher je Raum, PTT-Dialog mit Raum-Ziel, Neustart-Knopf,
// Raum-Editor. Nutzt drei simulierte Lautsprecher (fake_speaker.py).
'use strict';
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

module.exports = {
  name: 'Raumlautsprecher & Durchsage',
  needs: ['python3'],
  async run(t) {
    const { sleep, waitFor } = t.lib;
    const procs = [];
    const statusFile = (n) => path.join(t.workDir, `spk_${n}.json`);
    const spk = (n) => { try { return JSON.parse(fs.readFileSync(statusFile(n), 'utf8')); } catch { return { rtp_packets: 0, commands: [] }; } };
    const host = new URL(t.base).host;
    ['A', 'B', 'C'].forEach((n, i) => {
      procs.push(spawn('python3', ['-I', path.join(__dirname, '..', 'fake_speaker.py'), host, `e2e-${n}`, `Lautsprecher ${n}`, String(19501 + i), statusFile(n)], { stdio: 'ignore' }));
    });
    try {
      const list = async () => (await t.api.get('/api/voice/speakers')).json?.speakers || [];
      const all = await waitFor(async () => { const l = await list(); return l.filter((s) => s.online && /^Lautsprecher [ABC]$/.test(s.name)).length === 3 ? l : null; }, 15000);
      t.check('Drei Lautsprecher melden sich an', !!all);
      const id = Object.fromEntries((all || []).filter((s) => /^Lautsprecher [ABC]$/.test(s.name)).map((s) => [s.name.slice(-1), s.id]));
      await t.api.patch(`/api/voice/speakers/${id.A}`, { group: 'flur' });
      await t.api.patch(`/api/voice/speakers/${id.B}`, { group: 'flur' });
      await t.api.patch(`/api/voice/speakers/${id.C}`, { group: 'simraum' });
      const room = await t.api.post('/api/rooms', { name: 'E2E Simraum', layout: '1', sources: '[]', speakers: JSON.stringify([id.A, id.C]) });
      t.check('Raum mit Lautsprechern angelegt', room.status === 200 && room.json.speakers === JSON.stringify([id.A, id.C]), room.json);

      await t.api.post('/api/admin/settings', { record_enabled: false, output_rtmp: '' });
      await t.api.post('/api/sim/start', { name: 'E2E Raumdurchsage', room_id: room.json.id });

      const browser = await t.browser();
      const page = await browser.page();
      await t.api.loginPage(page);
      await page.eval(`localStorage.removeItem('ors_ptt_group'); sessionStorage.clear(); 'ok'`);
      await page.goto(t.base + '/ui', 3000);
      const packets = async (ms) => { const a = ['A', 'B', 'C'].map((n) => spk(n).rtp_packets); await sleep(ms); return Object.fromEntries(['A', 'B', 'C'].map((n, i) => [n, spk(n).rtp_packets - a[i]])); };
      const tap = () => page.eval(`document.getElementById('pttModalStart').click(); 'ok'`);
      // Bis Ton fließt: Mikrofon + WebRTC-Aufbau dauern beim ersten Mal unterschiedlich lang
      const flowing = () => waitFor(async () => { const q = await packets(500); return q.A + q.B + q.C > 0; }, 15000, 0);

      await page.eval(`document.getElementById('pttBtn').click(); 'ok'`);
      await sleep(2000);
      const modal = await page.eval(`({ value: document.getElementById('pttGroupSelect').value, options: [...document.querySelectorAll('#pttGroupSelect option')].map((o) => o.textContent) })`);
      t.check('PTT-Dialog: Raum-Ziel vorhanden und vorausgewählt', modal.value === `room:${room.json.id}` && modal.options[0].startsWith('🏠 Raum E2E Simraum'), modal);
      await page.eval(`(() => { const x = document.getElementById('pttModeToggle'); if (!x.checked) x.click(); return 'ok'; })()`);

      await tap(); await flowing();
      let p = await packets(2000);
      t.check('Durchsage an Raum erreicht genau A und C', p.A > 40 && p.C > 40 && p.B === 0, p);
      t.check('Statuszeile meldet Empfang', /2 von 2/.test(await page.eval(`document.getElementById('pttStatus').textContent`)));
      // Raum während der laufenden Durchsage ändern: A + C → A + B
      await t.api.patch(`/api/rooms/${room.json.id}`, { speakers: JSON.stringify([id.A, id.B]) });
      await sleep(1000);
      p = await packets(2000);
      t.check('Raum während Durchsage geändert: sofort A und B statt A und C', p.A > 40 && p.B > 40 && p.C === 0, p);
      await t.api.patch(`/api/rooms/${room.json.id}`, { speakers: JSON.stringify([id.A, id.C]) });
      await tap(); await sleep(1500);
      p = await packets(1500);
      t.check('Nach Stopp kein Ton mehr', p.A + p.B + p.C === 0, p);

      await page.eval(`(() => { const s = document.getElementById('pttGroupSelect'); s.value = 'flur'; s.dispatchEvent(new Event('change')); return 'ok'; })()`);
      await tap(); await flowing();
      p = await packets(2000);
      t.check('Durchsage an Gruppe „flur“ erreicht A und B', p.A > 40 && p.B > 40 && p.C === 0, p);
      await tap(); await sleep(1500);
      await page.eval(`document.getElementById('pttModalClose').click(); document.getElementById('pttBtn').click(); 'ok'`);
      await sleep(2000);
      t.check('Bewusste Wahl bleibt beim erneuten Öffnen', (await page.eval(`document.getElementById('pttGroupSelect').value`)) === 'flur');

      // Neustart-Knopf + Raum-Editor
      await page.goto(t.base + '/ui/options', 4000);
      const clicked = await page.eval(`(() => { window.confirm = () => true;
        const row = [...document.querySelectorAll('#speakersTableBody tr[data-id]')].find((tr) => [...tr.querySelectorAll('input')].some((i) => i.value === 'Lautsprecher B'));
        if (!row) return false; row.querySelector('button[data-action="reboot"]').click(); return true; })()`);
      await sleep(1500);
      t.check('Neustart-Knopf: nur B bekommt „reboot“', clicked && spk('B').commands.includes('reboot') && !spk('A').commands.includes('reboot'), { B: spk('B').commands });

      await page.eval(`document.getElementById('openRoomsModalBtn').click(); 'ok'`); await sleep(1500);
      await page.eval(`document.querySelector('[data-action="edit-room"][data-id="${room.json.id}"]').click(); 'ok'`); await sleep(1500);
      const boxes = () => page.eval(`[...document.querySelectorAll('#roomSpeakersList label')].filter((l) => /Lautsprecher [ABC]/.test(l.innerText)).map((l) => (l.querySelector('input').checked ? '☑' : '☐') + l.innerText.match(/Lautsprecher ([ABC])/)[1]).sort((a, b) => a.slice(1).localeCompare(b.slice(1))).join(' ')`);
      t.check('Raum-Editor zeigt Zuordnung', (await boxes()) === '☑A ☐B ☑C', await boxes());
      await page.eval(`(() => { const box = (n) => [...document.querySelectorAll('#roomSpeakersList label')].find((l) => l.innerText.includes('Lautsprecher ' + n)).querySelector('input'); box('B').click(); box('C').click(); document.querySelector('#roomEditForm button[type=submit]').click(); return 'ok'; })()`);
      await sleep(1500);
      const saved = (await t.api.get(`/api/rooms/${room.json.id}`)).json?.speakers;
      t.check('Raum-Editor speichert neue Auswahl', JSON.stringify(JSON.parse(saved || '[]').sort()) === JSON.stringify([id.A, id.B].sort()), saved);
      t.check('Keine JS-Fehler', page.errors.length === 0, page.errors);
    } finally {
      procs.forEach((pr) => { try { pr.kill('SIGKILL'); } catch {} });
    }
  },
};
