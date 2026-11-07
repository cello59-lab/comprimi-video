#!/bin/bash
# Crea Comprimi_Video-<versione>-x86_64.AppImage
#
# Va lanciato su Ubuntu 24.04 (o derivate): l'AppImage gira sui sistemi con glibc 2.39
# o più recente (Ubuntu 24.04+, Mint 22+, Debian 13+, Fedora 40+).
#
# Pacchetti necessari per la costruzione:
#   sudo apt install ffmpeg libgtk-4-dev libadwaita-1-dev libgtk-4-media-gstreamer \
#     gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
#     gstreamer1.0-libav gstreamer1.0-pulseaudio gstreamer1.0-alsa librsvg2-common \
#     adwaita-icon-theme patchelf
# Strumenti (in $STRUMENTI, predefinito ~/strumenti):
#   linuxdeploy-x86_64.AppImage e appimagetool-x86_64.AppImage

set -euo pipefail

QUI=$(cd "$(dirname "$0")" && pwd)
RADICE=$(dirname "$QUI")
STRUMENTI=${STRUMENTI:-$HOME/strumenti}
VERSIONE=$(grep -m1 '^version' "$RADICE/Cargo.toml" | cut -d'"' -f2)
APPDIR="$RADICE/target/AppDir"
USCITA="$RADICE/target/Comprimi_Video-$VERSIONE-x86_64.AppImage"
L=/usr/lib/x86_64-linux-gnu
export APPIMAGE_EXTRACT_AND_RUN=1   # gli strumenti funzionano anche senza FUSE

# Plugin GStreamer per l'anteprima dei tagli: lettura dei contenitori, decodifica
# software (libav), conversioni e audio. Volutamente esclusi: va (decodifica su GPU,
# artefatti verdi), gtk* (per GTK3, bloccano l'app), directfb e altre uscite video.
PLUGIN_GST=(coreelements typefindfunctions playback app videoconvertscale audioconvert
            audioresample volume autodetect isomp4 matroska avi libav pulseaudio alsa
            audioparsers videoparsersbad mpegtsdemux audiofx)

# Librerie da NON includere: le prende dal sistema.
#  - libva*: deve essere quella allineata ai driver VA del sistema (c'è un ripiego, vedi sotto)
#  - libstdc++/libgcc_s/libz/libexpat/libzstd/libelf/wayland: i driver grafici del sistema
#    (Mesa) le usano e richiedono versioni recenti quanto il sistema stesso
ESCLUSE=('libva.so*' 'libva-drm.so*' 'libva-x11.so*' 'libva-wayland.so*'
         'libstdc++.so*' 'libgcc_s.so*' 'libz.so*' 'libexpat.so*' 'libzstd.so*' 'libelf.so*'
         'libwayland-client.so*' 'libwayland-server.so*' 'libwayland-egl.so*' 'libwayland-cursor.so*')

echo "== Compilazione $VERSIONE"
cargo build --release --manifest-path "$RADICE/Cargo.toml"

echo "== Preparazione AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"

argomenti=(--appdir "$APPDIR"
           --executable "$RADICE/target/release/comprimi-video"
           --executable /usr/bin/ffmpeg
           --executable /usr/bin/ffprobe
           --desktop-file "$QUI/io.github.cello59_lab.ComprimiVideo.desktop"
           --icon-file "$QUI/io.github.cello59_lab.ComprimiVideo.svg"
           --library "$L/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner"
           --library "$L/gtk-4.0/4.0.0/media/libmedia-gstreamer.so"
           --library "$L/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-svg.so"
           --library "$L/gio/modules/libdconfsettings.so")
for p in "${PLUGIN_GST[@]}"; do
    argomenti+=(--library "$L/gstreamer-1.0/libgst$p.so")
done
for e in "${ESCLUSE[@]}"; do
    argomenti+=(--exclude-library "$e")
done
"$STRUMENTI/linuxdeploy-x86_64.AppImage" "${argomenti[@]}"

echo "== Moduli nelle loro cartelle"
U="$APPDIR/usr"
sposta() {   # sposta <file> <cartella> <rpath relativo alla nuova cartella>
    mkdir -p "$2"
    mv "$U/lib/$(basename "$1")" "$2/"
    patchelf --set-rpath "$3" "$2/$(basename "$1")"
}
for p in "${PLUGIN_GST[@]}"; do
    sposta "libgst$p.so" "$U/lib/gstreamer-1.0" '$ORIGIN/..'
done
sposta gst-plugin-scanner "$U/libexec/gstreamer-1.0" '$ORIGIN/../../lib'
chmod +x "$U/libexec/gstreamer-1.0/gst-plugin-scanner"
sposta libmedia-gstreamer.so "$U/lib/gtk-4.0/4.0.0/media" '$ORIGIN/../../../'
sposta libpixbufloader-svg.so "$U/lib/gdk-pixbuf-2.0/2.10.0/loaders" '$ORIGIN/../../../'
sposta libdconfsettings.so "$U/lib/gio/modules" '$ORIGIN/../../'

# elenco dei caricatori di immagini (per le icone SVG) con un segnaposto al posto del
# percorso: AppRun lo sostituisce con il percorso di montaggio di ogni avvio
LD_LIBRARY_PATH="$U/lib" "$L/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders" \
    "$U/lib/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-svg.so" \
    | sed "s|$APPDIR|@APPDIR@|g" > "$U/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache.in"

echo "== libva di ripiego (usata solo se il sistema non ha la sua)"
mkdir -p "$U/lib/libva-ripiego"
for f in libva.so.2 libva-drm.so.2 libva-x11.so.2; do
    cp -L "$L/$f" "$U/lib/libva-ripiego/"
done

echo "== Icone, schemi e dati"
mkdir -p "$U/share/icons/Adwaita" "$U/share/glib-2.0/schemas"
cp -r /usr/share/icons/Adwaita/symbolic /usr/share/icons/Adwaita/scalable /usr/share/icons/Adwaita/index.theme \
    "$U/share/icons/Adwaita/"
cp /usr/share/icons/hicolor/index.theme "$U/share/icons/hicolor/" 2>/dev/null || true
cp /usr/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$U/share/glib-2.0/schemas/"
glib-compile-schemas "$U/share/glib-2.0/schemas"

echo "== AppRun"
rm -f "$APPDIR/AppRun"
cp "$QUI/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

echo "== Creazione dell'AppImage"
rm -f "$USCITA"
ARCH=x86_64 "$STRUMENTI/appimagetool-x86_64.AppImage" --no-appstream "$APPDIR" "$USCITA"
ls -la "$USCITA"
