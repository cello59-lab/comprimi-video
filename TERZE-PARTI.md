# Componenti di terze parti nell'AppImage

L'AppImage di Comprimi Video include programmi e librerie di altri progetti.
Ognuno resta sotto la propria licenza.

## FFmpeg

**FFmpeg 8.1.3**, compilato dai sorgenti ufficiali con lo script
[`pacchetto/compila-ffmpeg.sh`](pacchetto/compila-ffmpeg.sh), che contiene anche
le opzioni di configurazione esatte. Licenza: **GPL-2.0-or-later**, perché è compilato
con x264 e x265.

- Sorgenti: <https://github.com/FFmpeg/FFmpeg/tree/n8.1.3>
  (oppure <https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz>)
- Header NVENC (nv-codec-headers n12.0.16.1, licenza MIT):
  <https://github.com/FFmpeg/nv-codec-headers/tree/n12.0.16.1>

## Librerie da Ubuntu 24.04 LTS (noble)

Le altre librerie sono prese, senza modifiche, dai pacchetti binari di Ubuntu 24.04 per x86_64.

| Componente | Versione | Licenza | Sorgenti |
|---|---|---|---|
| x264 | 0.164 (`libx264-164`) | GPL-2.0-or-later | <https://launchpad.net/ubuntu/noble/+source/x264> |
| x265 | 3.5 (`libx265-199`) | GPL-2.0-or-later | <https://launchpad.net/ubuntu/noble/+source/x265> |
| SVT-AV1 | 1.7.0 | BSD-3-Clause-Clear | <https://launchpad.net/ubuntu/noble/+source/svt-av1> |
| libaom | 3.8.2 | BSD-2-Clause | <https://launchpad.net/ubuntu/noble/+source/aom> |
| dav1d | 1.4.1 | BSD-2-Clause | <https://launchpad.net/ubuntu/noble/+source/dav1d> |
| GTK 4 | 4.14 | LGPL-2.1-or-later | <https://launchpad.net/ubuntu/noble/+source/gtk4> |
| libadwaita | 1.5 | LGPL-2.1-or-later | <https://launchpad.net/ubuntu/noble/+source/libadwaita-1> |
| GStreamer e plugin base/good/bad/libav | 1.24 | LGPL-2.1-or-later | <https://launchpad.net/ubuntu/noble/+source/gstreamer1.0> |
| GLib, Pango, Cairo, HarfBuzz, librsvg e altre dipendenze | noble | LGPL / MIT / altre libere | <https://launchpad.net/ubuntu/noble> |
| Icone Adwaita | 46 | LGPL-3.0 / CC-BY-SA-3.0 | <https://launchpad.net/ubuntu/noble/+source/adwaita-icon-theme> |

L'elenco completo delle librerie incluse si vede estraendo l'AppImage:

```bash
./Comprimi_Video-*-x86_64.AppImage --appimage-extract
ls squashfs-root/usr/lib
```

Per ottenere i sorgenti esatti di un pacchetto di Ubuntu 24.04:
`apt-get source <nome-del-pacchetto>`. Su richiesta, tramite le *issue* del repository,
li fornisco direttamente.
