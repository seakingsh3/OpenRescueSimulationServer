// Hilfeseite: Expertenmodus blendet Admin-Abschnitte ein/aus, Handy-Breite ohne Querscrollen,
// alle Inhaltsverzeichnis-Links zeigen auf vorhandene Abschnitte.
'use strict';

module.exports = {
  name: 'Hilfeseite',
  async run(t) {
    const browser = await t.browser();
    const page = await browser.page();
    await page.goto(t.base + '/help/', 2000);
    const display = (sel) => page.eval(`getComputedStyle(document.querySelector(${JSON.stringify(sel)})).display`);
    t.check('Anwender-Abschnitt sichtbar', (await display('#zusatzgeraete')) !== 'none');
    t.check('Admin-Abschnitt ohne Expertenmodus versteckt', (await display('#zusatzgeraete-admin')) === 'none');
    await page.eval(`document.getElementById('expertToggle').click(); 'ok'`);
    t.check('Admin-Abschnitt mit Expertenmodus sichtbar', (await display('#zusatzgeraete-admin')) !== 'none');
    const broken = await page.eval(`[...document.querySelectorAll('.help-toc a[href^="#"]')].map((a) => a.getAttribute('href')).filter((h) => !document.querySelector(h))`);
    t.check('Alle Inhaltsverzeichnis-Links führen zu Abschnitten', broken.length === 0, broken);
    await page.setViewport(390, 900);
    await t.lib.sleep(500);
    t.check('Handy-Breite: kein Querscrollen', !(await page.eval('document.documentElement.scrollWidth > window.innerWidth')));
    // Lizenzen: Texte öffentlich abrufbar, Karte auf der Systemseite
    const gpl = await fetch(t.base + '/licenses/gpl').then((r) => r.text());
    const third = await fetch(t.base + '/licenses/third-party').then((r) => r.text());
    t.check('GPLv3-Text abrufbar', gpl.includes('GNU GENERAL PUBLIC LICENSE') && gpl.includes('Version 3'));
    t.check('Fremdlizenzen abrufbar (Lucide, Tailwind, daisyUI)', ['Lucide', 'Tailwind', 'daisyUI', 'ISC License', 'MIT License'].every((w) => third.includes(w)));
    const sys = await browser.page();
    await t.api.loginPage(sys);
    await sys.goto(t.base + '/ui/options/system', 2500);
    t.check('Systemseite zeigt „Über Open Rescue Simulation & Lizenzen“', /GPL/.test(await sys.eval(`document.getElementById('systemAbout')?.innerText || ''`)));
    t.check('Keine JS-Fehler', page.errors.length === 0, page.errors);
  },
};
