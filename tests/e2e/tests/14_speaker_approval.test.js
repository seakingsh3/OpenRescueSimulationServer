// Freigabe der Raumlautsprecher: neue Firmware wartet auf Freigabe (kein Ton), Freigeben
// in der Lautsprecherliste schickt den Schlüssel, danach Ton – auch nach Neustart des
// Geräts. Fremdes Gerät mit derselben ID wird abgewiesen, Entziehen stoppt den Ton.
// Alte Firmware ohne Schlüssel läuft übergangsweise weiter.
'use strict';
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

module.exports = {
  name: 'Freigabe Raumlautsprecher',
  needs: ['python3'],
  async run(t) {
    const { sleep, waitFor, START_TX, STOP_TX } = t.lib;
    const host = new URL(t.base).host;
    const script = path.join(__dirname, '..', 'fake_speaker.py');
    const procs = [];
    const statusFile = (n) => path.join(t.workDir, `appr_${n}.json`);
    const keyFile = path.join(t.workDir, 'appr_neu.key');
    const spk = (n) => { try { return JSON.parse(fs.readFileSync(statusFile(n), 'utf8')); } catch { return { rtp_packets: 0, commands: [] }; } };
    const start = (n, devId, port, withKey) => {
      const p = spawn('python3', ['-I', script, host, devId, `Lautsprecher ${n}`, String(port), statusFile(n), ...(withKey ? [withKey] : [])], { stdio: 'ignore' });
      procs.push(p);
      return p;
    };
    const listed = async (name) => ((await t.api.get('/api/voice/speakers')).json?.speakers || []).find((s) => s.name === name);
    const packets = async (names, ms = 2000) => { const a = names.map((n) => spk(n).rtp_packets); await sleep(ms); return Object.fromEntries(names.map((n, i) => [n, spk(n).rtp_packets - a[i]])); };

    try {
      const neu = start('Neu', 'e2e-neu', 19611, keyFile);
      start('Alt', 'e2e-alt', 19612, null);
      await waitFor(async () => (await listed('Lautsprecher Neu'))?.online && (await listed('Lautsprecher Alt'))?.online, 15000);
      t.check('Neue Firmware: „nicht freigegeben“', (await listed('Lautsprecher Neu'))?.auth === 'pending', (await listed('Lautsprecher Neu'))?.auth);
      t.check('Alte Firmware: „alt · ungesichert“', (await listed('Lautsprecher Alt'))?.auth === 'legacy');

      const browser = await t.browser();
      const tx = await browser.page();
      await t.api.loginPage(tx);
      await t.api.post('/api/admin/settings', { record_enabled: false });
      await tx.eval(START_TX('default', false));
      await waitFor(async () => (await packets(['Alt'], 600)).Alt > 0, 15000);
      let p = await packets(['Neu', 'Alt']);
      t.check('Durchsage: nicht freigegebenes Gerät bekommt keinen Ton, altes schon', p.Neu === 0 && p.Alt > 40, p);

      const idNeu = (await listed('Lautsprecher Neu')).id;
      await t.api.post(`/api/voice/speakers/${idNeu}/cmd`, { cmd: 'identify' });
      await sleep(800);
      t.check('Identifizieren klappt auch ohne Freigabe', spk('Neu').commands.includes('identify'));

      // Freigeben über die Lautsprecherliste
      const opt = await browser.page();
      await t.api.loginPage(opt);
      await opt.goto(t.base + '/ui/options', 4000);
      const clicked = await opt.eval(`(() => { window.confirm = () => true;
        const row = [...document.querySelectorAll('#speakersTableBody tr[data-id]')].find((tr) => [...tr.querySelectorAll('input')].some((i) => i.value === 'Lautsprecher Neu'));
        const b = row && row.querySelector('button[data-action="approve"]'); if (!b) return false; b.click(); return true; })()`);
      t.check('Knopf „Freigeben“ vorhanden', clicked);
      await waitFor(async () => (await listed('Lautsprecher Neu'))?.auth === 'approved', 8000);
      const gotKey = await waitFor(() => spk('Neu').commands.includes('pair') && fs.existsSync(keyFile), 5000);
      t.check('Gerät erhält Schlüssel und ist freigegeben', !!gotKey && (await listed('Lautsprecher Neu')).auth === 'approved');
      p = await packets(['Neu']);
      t.check('Nach Freigabe: sofort Ton (laufende Durchsage)', p.Neu > 40, p);
      await tx.eval(STOP_TX);
      await sleep(1500);

      // Gerät startet neu → meldet sich mit Schlüssel an
      neu.kill('SIGKILL');
      await waitFor(async () => !(await listed('Lautsprecher Neu'))?.online, 20000, 1000);
      start('Neu', 'e2e-neu', 19611, keyFile);
      await waitFor(async () => (await listed('Lautsprecher Neu'))?.online, 15000);
      t.check('Nach Neustart mit Schlüssel: freigegeben', (await listed('Lautsprecher Neu'))?.auth === 'approved');

      // Fremdes Gerät mit derselben ID, aber ohne Schlüssel
      const fakeKey = path.join(t.workDir, 'appr_fremd.key');
      start('Fremd', 'e2e-neu', 19613, fakeKey);
      await sleep(2500);
      await tx.eval(START_TX('default', false));
      await waitFor(async () => (await packets(['Neu'], 600)).Neu > 0, 15000);
      p = await packets(['Neu', 'Fremd']);
      t.check('Fremdes Gerät mit gleicher ID: abgewiesen, echtes bleibt', p.Neu > 40 && p.Fremd === 0 && (await listed('Lautsprecher Neu'))?.auth === 'approved', p);

      // Freigabe entziehen
      await t.api.post(`/api/voice/speakers/${idNeu}/revoke`, {});
      await sleep(1000);
      p = await packets(['Neu']);
      t.check('Freigabe entzogen: kein Ton mehr, Schlüssel gelöscht', p.Neu === 0 && !fs.existsSync(keyFile) && (await listed('Lautsprecher Neu'))?.auth === 'pending', p);
      await tx.eval(STOP_TX);
      t.check('Keine JS-Fehler', opt.errors.length + tx.errors.length === 0, [...opt.errors, ...tx.errors]);
    } finally {
      procs.forEach((pr) => { try { pr.kill('SIGKILL'); } catch {} });
    }
  },
};
