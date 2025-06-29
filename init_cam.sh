#!/usr/bin/env bash
# init_cam.sh  [width] [height] [pixfmt]
# Example:  ./init_cam.sh 640 480 YUYV

      # e.g. NV12, RGB3 …

ssh clamp bash -se << 'EOS'
WIDTH=${1:-640}
HEIGHT=${2:-480}
PIX=${3:-YUYV}  
set -euo pipefail
WIDTH=$WIDTH HEIGHT=$HEIGHT PIX=$PIX

MBE=/dev/media1

# Берём первые ДВА id 'rp1-cfe-fe_image0' именно из /dev/media1
read FE0 FE1 < <(media-ctl -d $MBE -p |
                 awk '/rp1-cfe-fe_image0/ {gsub(/:/,"",$2); print $2}' | head -n2)

BE=$(media-ctl -d $MBE -p |
     awk '/pispbe / {gsub(/:/,"",$2); print $2; exit}')

echo "FE0=$FE0  FE1=$FE1  BE=$BE (media=$MBE)"

media-ctl -d $MBE -r
media-ctl -d $MBE -l ${FE0}:0->${BE}:0[1]
media-ctl -d $MBE -l ${FE1}:0->${BE}:1[1]

Sink=SBGGR16_1X16
Src=${PIX}8_2X8

media-ctl -d $MBE -V \
        ${BE}:0"[fmt:$Sink/${WIDTH}x${HEIGHT}]" \
        ${BE}:2"[fmt:$Src/${WIDTH}x${HEIGHT}]" \
        ${BE}:1"[fmt:$Sink/${WIDTH}x${HEIGHT}]" \
        ${BE}:3"[fmt:$Src/${WIDTH}x${HEIGHT}]"

for node in /dev/video20 /dev/video24; do
  echo ">>> testing $node"
  v4l2-ctl -d $node --set-fmt-video=width=$WIDTH,height=$HEIGHT,pixelformat=$PIX \
           --stream-mmap --stream-count=3 --stream-poll
done
EOS
