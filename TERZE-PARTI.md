# Componenti di terze parti nell'AppImage

L'AppImage di Comprimi Video include, senza modifiche, programmi e librerie presi
dai pacchetti binari di **Ubuntu 24.04 LTS (noble)** per x86_64. Ognuno resta sotto
la propria licenza.

| Componente | Versione | Licenza | Sorgenti |
|---|---|---|---|
| FFmpeg (ffmpeg, ffprobe, libav*) | 7:6.1.1-3ubuntu5 | GPL-2.0-or-later (compilato con x264/x265) | <https://launchpad.net/ubuntu/+source/ffmpeg/7:6.1.1-3ubuntu5> |
| x264 | pacchetto `libx264-164` di noble | GPL-2.0-or-later | <https://launchpad.net/ubuntu/noble/+source/x264> |
| x265 | pacchetto `libx265-199` di noble | GPL-2.0-or-later | <https://launchpad.net/ubuntu/noble/+source/x265> |
| SVT-AV1, libaom, dav1d | pacchetti di noble | BSD-2/3-Clause | <https://launchpad.net/ubuntu/noble> |
| GTK 4 | 4.14 (noble) | LGPL-2.1-or-later | <https://launchpad.net/ubuntu/noble/+source/gtk4> |
| libadwaita | 1.5 (noble) | LGPL-2.1-or-later | <https://launchpad.net/ubuntu/noble/+source/libadwaita-1> |
| GStreamer e plugin base/good/bad/libav | 1.24 (noble) | LGPL-2.1-or-later | <https://launchpad.net/ubuntu/noble/+source/gstreamer1.0> |
| GLib, Pango, Cairo, HarfBuzz, librsvg e altre dipendenze | pacchetti di noble | LGPL / MIT / altre libere | <https://launchpad.net/ubuntu/noble> |
| Icone Adwaita | 46 (noble) | LGPL-3.0 / CC-BY-SA-3.0 | <https://launchpad.net/ubuntu/noble/+source/adwaita-icon-theme> |

L'elenco completo delle librerie incluse si vede estraendo l'AppImage:

```bash
./Comprimi_Video-*-x86_64.AppImage --appimage-extract
ls squashfs-root/usr/lib
```

Per ottenere i sorgenti esatti di qualunque componente su Ubuntu 24.04:
`apt-get source <nome-del-pacchetto>`. Su richiesta, tramite le *issue* del repository,
li fornisco direttamente.
