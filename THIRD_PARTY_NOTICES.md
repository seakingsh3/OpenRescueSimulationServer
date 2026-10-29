# Fremdbestandteile und Lizenzen

Open Rescue Simulation Server selbst steht unter der **GNU GPL v3** (siehe `LICENSE`). Diese Datei
listet mitgelieferte Fremdbestandteile mit ihren Lizenzen. Alle sind mit der GPLv3 verträglich.

## Eigene Inhalte

Bilder und Illustrationen (`src/web/frontend/static/img/`, `src/web/viewer/*.png`, ORS-Logos)
sind eigene Werke des Projekts, teils mit KI-Unterstützung erstellt. Sie gehören zum Projekt
und stehen wie der Code unter der GPLv3.

## Im Paket enthalten

| Bestandteil | Verwendung | Lizenz |
|---|---|---|
| [Lucide](https://lucide.dev) 0.454.0 | Icons unter `src/web/frontend/static/icons/` (alle außer den ORS-Logos) | ISC |
| [Tailwind CSS](https://tailwindcss.com) 3.4.17 | erzeugtes Stylesheet `static/dist/style.css` | MIT |
| [daisyUI](https://daisyui.com) 5.5.8 | Komponenten im Stylesheet `static/dist/style.css` | MIT |
| Rust-Bibliotheken (Cargo) | im Programm `openresquesim-server` einkompiliert | MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0, Unicode-3.0 u.a. – Liste: `cargo metadata` bzw. `cargo about` |
| egui/epaint | eingebettete Schriften der GUI | OFL-1.1, Ubuntu Font Licence 1.0 |

Die Rust-Abhängigkeiten wurden am 2026-10-23 geprüft: Es gibt keine GPL-unverträglichen
Lizenzen (das frühere `ring` 0.16 mit OpenSSL-Lizenz ist durch das Update auf
`jsonwebtoken` 9 entfallen).

## Zur Laufzeit genutzt, nicht mitgeliefert

GStreamer und seine Plugins (LGPL, `x264` GPL), FFmpeg (LGPL/GPL) und die übrigen
Systembibliotheken werden über die Paketabhängigkeiten aus Debian installiert.

## Lizenztexte

### Lucide (ISC)

```
ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### Tailwind CSS (MIT)

```
MIT License

Copyright (c) Tailwind Labs, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### daisyUI (MIT)

```
MIT License

Copyright (c) 2020 Pouya Saadeghi

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
