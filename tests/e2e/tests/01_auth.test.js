// Anmeldung und Token-Schlüssel (auch für nur aktualisierte Server ohne Schlüssel).
'use strict';
const { execFileSync } = require('child_process');
const path = require('path');

module.exports = {
  name: 'Anmeldung & Token-Schlüssel',
  needs: ['sqlite3'],
  async run(t) {
    const db = path.join(t.dataDir, 'app.db');
    const sql = (q) => String(execFileSync('sqlite3', [db, q])).trim();

    t.check('Login liefert Token', !!t.api.token);
    const noAuth = await fetch(t.base + '/api/voice/speakers');
    t.check('Geschützte API ohne Token → 401', noAuth.status === 401, noAuth.status);
    const pub = await fetch(t.base + '/api/sim/status');
    t.check('/api/sim/status ist öffentlich (On-Air-Lampe)', pub.status === 200, pub.status);

    // Wie ein alter Server: Standardwert in der DB → beim Start durch Zufallsschlüssel ersetzen
    const oldToken = t.api.token;
    sql("UPDATE settings SET value='change-me-please' WHERE key='auth.secret'");
    const mark = t.logMark();
    await t.restartServer();
    const secret = sql("SELECT value FROM settings WHERE key='auth.secret'");
    t.check('Unsicherer Standardwert wird ersetzt', secret.length === 64 && secret !== 'change-me-please', secret.length + ' Zeichen');
    t.check('Erzeugung wird im Log gemeldet', t.logSince(mark).includes('zufälligen Schlüssel erzeugt'));
    t.api.token = oldToken;
    const withOld = await t.api.get('/api/voice/speakers');
    t.check('Alte Anmeldung danach ungültig', withOld.status === 401, withOld.status);
    await t.api.login();
    t.check('Neue Anmeldung funktioniert', (await t.api.get('/api/voice/speakers')).status === 200);

    const mark2 = t.logMark();
    await t.restartServer();
    t.check('Schlüssel bleibt beim nächsten Start erhalten', sql("SELECT value FROM settings WHERE key='auth.secret'") === secret);
    t.check('Kein erneutes Erzeugen', !t.logSince(mark2).includes('zufälligen Schlüssel erzeugt'));
    t.check('Anmeldung bleibt über Neustart gültig', (await t.api.get('/api/voice/speakers')).status === 200);
  },
};
