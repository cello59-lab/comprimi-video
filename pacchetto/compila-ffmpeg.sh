#!/bin/bash
# Compila FFmpeg per l'AppImage di Comprimi Video, dai sorgenti ufficiali.
#
# Va lanciato su Ubuntu 24.04, come crea-appimage.sh: così ffmpeg usa le stesse librerie
# di sistema e l'AppImage continua a girare con glibc 2.39 (Ubuntu 24.04+, Mint 22+).
#
# Pacchetti necessari:
#   sudo apt install build-essential nasm pkg-config git libx264-dev libx265-dev \
#     libsvtav1enc-dev libaom-dev libdav1d-dev libva-dev
#
# Risultato: target/ffmpeg/install/bin/ffmpeg e ffprobe (librerie di FFmpeg incluse
# nell'eseguibile; x264, x265 ecc. restano librerie a parte, che crea-appimage.sh copia).

set -euo pipefail

FFMPEG_VERSIONE=${FFMPEG_VERSIONE:-8.1.3}
# NVENC: gli header 12.0 richiedono il driver NVIDIA 520 o più recente (serve per AV1)
NV_HEADERS=${NV_HEADERS:-n12.0.16.1}

QUI=$(cd "$(dirname "$0")" && pwd)
RADICE=$(dirname "$QUI")
LAVORO="$RADICE/target/ffmpeg"
SORGENTI="$LAVORO/sorgenti-$FFMPEG_VERSIONE"
NV="$LAVORO/nv-codec-headers"
PREFISSO="$LAVORO/install"
mkdir -p "$LAVORO"

echo "== Sorgenti"
if [ ! -d "$SORGENTI" ]; then
    git clone --depth 1 --branch "n$FFMPEG_VERSIONE" https://github.com/FFmpeg/FFmpeg "$SORGENTI"
fi
if [ ! -d "$NV" ]; then
    git clone --depth 1 --branch "$NV_HEADERS" https://github.com/FFmpeg/nv-codec-headers "$NV"
fi
make -C "$NV" install PREFIX="$LAVORO/nv-install" >/dev/null

echo "== Configurazione di FFmpeg $FFMPEG_VERSIONE"
cd "$SORGENTI"
# --disable-autodetect: entra solo quello elencato qui, niente dipendenze prese per caso
# dal sistema di costruzione (X11, SDL, ALSA…) che poi mancherebbero altrove.
PKG_CONFIG_PATH="$LAVORO/nv-install/lib/pkgconfig" ./configure \
    --prefix="$PREFISSO" \
    --extra-version=comprimi-video \
    --enable-gpl \
    --disable-autodetect \
    --disable-debug --disable-doc --disable-ffplay \
    --enable-pthreads --enable-zlib \
    --enable-vaapi \
    --enable-ffnvcodec --enable-nvenc \
    --enable-libx264 --enable-libx265 --enable-libsvtav1 --enable-libaom \
    --enable-libdav1d

echo "== Compilazione (richiede un po' di tempo)"
make -j"$(nproc)"
make install

echo "== Fatto"
"$PREFISSO/bin/ffmpeg" -hide_banner -version | sed -n 1p
# l'elenco si legge una volta sola: con «ffmpeg | grep -q» e pipefail, grep che si ferma
# alla prima riga trovata farebbe risultare un errore anche quando l'encoder c'è
ENCODER=$("$PREFISSO/bin/ffmpeg" -hide_banner -encoders 2>/dev/null)
for e in h264_vaapi hevc_vaapi av1_vaapi h264_nvenc hevc_nvenc av1_nvenc libx264 libx265 libsvtav1 libaom-av1; do
    if grep -q " $e " <<<"$ENCODER"; then
        echo "  ✓ $e"
    else
        echo "  ✗ $e MANCANTE"
        exit 1
    fi
done
