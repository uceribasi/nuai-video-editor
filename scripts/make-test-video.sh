#!/usr/bin/env bash
# Builds a short Turkish test video full of things nuai should remove:
# long pauses, an abandoned take, a filler and a frozen picture.
# macOS only (uses the `say` voice "Yelda"). Usage: scripts/make-test-video.sh [out.mp4]
set -euo pipefail

OUT="${1:-test-video.mp4}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

parts=()
speak() { # text
  local f="$WORK/p${#parts[@]}.wav"
  say -v Yelda -o "$WORK/tmp.aiff" "$1"
  ffmpeg -v error -y -i "$WORK/tmp.aiff" -ar 48000 -ac 1 "$f"
  parts+=("$f")
}
pause() { # seconds
  local f="$WORK/p${#parts[@]}.wav"
  ffmpeg -v error -y -f lavfi -i "anoisesrc=r=48000:a=0.0008:c=pink" -t "$1" -ac 1 "$f"
  parts+=("$f")
}

pause 1.2
speak "Merhaba arkadaşlar, bugün size yeni projemizi tanıtacağım."
pause 2.0
speak "Bu uygulama videolardaki gereksiz"
pause 0.9
speak "Bu uygulama videolardaki gereksiz kısımları otomatik olarak siliyor."
pause 1.4
speak "Eee"
pause 0.6
speak "Örneğin uzun sessizlikleri ve tekrar eden cümleleri kaldırıyor."
FREEZE_FROM=$(for p in "${parts[@]}"; do ffprobe -v error -show_entries format=duration -of csv=p=0 "$p"; done | paste -sd+ - | bc)
pause 3.5
speak "Sonuç olarak videonuz çok daha kısa ve akıcı oluyor."
pause 1.0

list="$WORK/list.txt"
for p in "${parts[@]}"; do echo "file '$p'" >> "$list"; done
ffmpeg -v error -y -f concat -safe 0 -i "$list" -c:a pcm_s16le "$WORK/audio.wav"
DUR=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/audio.wav")

# Animated test pattern, frozen during the 3.5 s pause after the third sentence.
FPS=30
FIRST=$(printf "%.0f" "$(echo "$FREEZE_FROM * $FPS" | bc)")
LAST=$(printf "%.0f" "$(echo "($FREEZE_FROM + 3.5) * $FPS" | bc)")
ffmpeg -v error -y -f lavfi -i "testsrc2=size=1280x720:rate=$FPS" -i "$WORK/audio.wav" -t "$DUR" \
  -filter_complex "[0:v]split[a][b];[a][b]freezeframes=first=$FIRST:last=$LAST:replace=$FIRST[v]" \
  -map "[v]" -map 1:a -c:v libx264 -pix_fmt yuv420p -preset veryfast -c:a aac -b:a 160k -shortest "$OUT"
echo "Wrote $OUT (${DUR}s, picture frozen ${FREEZE_FROM}s → $(echo "$FREEZE_FROM + 3.5" | bc)s)"
