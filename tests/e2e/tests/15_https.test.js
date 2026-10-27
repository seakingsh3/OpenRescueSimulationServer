// HTTPS aus der Oberfläche: Karte nur für Admins, Eingaben werden geprüft, Einrichtung
// läuft mit Live-Log, danach ist das Wurzelzertifikat herunterladbar (Download + QR).
// Statt des echten Skripts (root, mkcert) läuft tests/e2e/fake_https_setup.sh.
'use strict';
const path = require('path');

module.exports = {
  name: 'HTTPS einrichten & Wurzelzertifikat',
  needs: ['openssl'],
  async run(t) {
    const { sleep, waitFor } = t.lib;
    // Normalbetrieb ohne root und ohne sudo-Regel (wie beim Entwickeln): klare Meldung statt Fehlversuch
    if (process.getuid && process.getuid() !== 0 && !require('fs').existsSync('/etc/sudoers.d/openresquesim')) {
      const st = (await t.api.get('/api/admin/https/status')).json || {};
      t.check('Ohne Rechte: Einrichten gesperrt mit Begründung', st.can_run === false && /root|sudo/.test(st.reason || ''), st.reason);
    }
    await t.restartServer({ ORS_HTTPS_SETUP_SCRIPT: path.join(__dirname, '..', 'fake_https_setup.sh') });
    try {
      await t.api.login();
      const ca0 = await fetch(t.base + '/ca/ors-rootCA.crt');
      t.check('Vor der Einrichtung: kein Wurzelzertifikat (404)', ca0.status === 404, ca0.status);

      const bad = await t.api.post('/api/admin/https/setup', { names: ['ors.local; rm -rf /'] });
      t.check('Ungültige Namen werden abgelehnt', bad.status === 400, bad.status);

      // Nicht-Admin: kein Zugriff, Karte verborgen
      await t.api.post('/api/users', { username: 'e2e-https-user', password: 'e2e-userpass', role: 'user' });
      const user = t.lib.makeApi(t.base);
      await user.login('e2e-https-user', 'e2e-userpass');
      t.check('Nicht-Admin: Status gesperrt', (await user.get('/api/admin/https/status')).status === 403);
      t.check('Nicht-Admin: Einrichten gesperrt', (await user.post('/api/admin/https/setup', {})).status === 403);
      // getrennte Browser: Nicht-Admin und Admin teilen sich sonst den Login-Speicher
      const userBrowser = await t.browser();
      const up = await userBrowser.page();
      await user.loginPage(up);
      await up.goto(t.base + '/ui/options/system', 2500);
      t.check('Nicht-Admin: Karte „HTTPS“ verborgen', await up.eval(`document.getElementById('systemHttps').classList.contains('hidden')`));

      // Admin richtet ein
      const browser = await t.browser();
      const page = await browser.page();
      await t.api.loginPage(page);
      await page.goto(t.base + '/ui/options/system', 2500);
      const cardState = () => page.eval(`({ hidden: document.getElementById('systemHttps').classList.contains('hidden'), badge: document.getElementById('httpsBadge').textContent })`);
      const shown = await waitFor(async () => { const c = await cardState(); return !c.hidden && /nicht aktiv/.test(c.badge) ? c : null; }, 8000, 300);
      t.check('Admin: Karte sichtbar, Zustand „nicht aktiv“', !!shown, shown || await cardState());
      await page.eval(`window.confirm = () => true; document.getElementById('httpsNames').value = 'ors.local, 192.168.1.20'; document.getElementById('httpsSetupBtn').click(); 'ok'`);
      const sawRunning = await waitFor(async () => /läuft/.test(await page.eval(`document.getElementById('httpsBadge').textContent`)), 5000, 200);
      t.check('Während der Einrichtung: „Einrichtung läuft …“', !!sawRunning);
      const done = await waitFor(async () => /Fertig/.test(await page.eval(`document.getElementById('httpsLog').textContent`)), 15000);
      t.check('Live-Log zeigt den Ablauf bis „Fertig“', !!done && /ors\.local 192\.168\.1\.20/.test(await page.eval(`document.getElementById('httpsLog').textContent`)));

      const ca = await fetch(t.base + '/ca/ors-rootCA.crt');
      const body = await ca.text();
      t.check('Wurzelzertifikat herunterladbar (ohne Login)', ca.status === 200 && body.includes('BEGIN CERTIFICATE'), ca.status);
      t.check('Dateityp für iPads (application/x-x509-ca-cert)', ca.headers.get('content-type') === 'application/x-x509-ca-cert', ca.headers.get('content-type'));
      await page.reload(2500);
      const caState = () => page.eval(`({ link: !document.getElementById('httpsCaBox').classList.contains('hidden'), qr: document.getElementById('httpsCaQr').getAttribute('src') || '', qrUrl: document.getElementById('httpsCaQr').title })`);
      const ui = (await waitFor(async () => { const c = await caState(); return c.link && c.qr ? c : null; }, 8000, 300)) || await caState();
      t.check('Karte zeigt Download und QR-Code', ui.link && ui.qr.startsWith('/api/qr?text=') && /^http:\/\/.+\/ca\/ors-rootCA\.crt$/.test(ui.qrUrl), ui);
      t.check('Keine JS-Fehler', page.errors.length + up.errors.length === 0, [...page.errors, ...up.errors]);
    } finally {
      await t.restartServer();
    }
  },
};
