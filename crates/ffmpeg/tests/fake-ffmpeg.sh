#!/bin/sh
# Stands in for ffmpeg in the decode tests. It writes 13,653 whole lines of errors (200 KB) to
# stderr, more than a pipe holds, then decodes to 1000 frames of stereo f32 silence. An input path
# containing "unsupported" fails the way a missing decoder does instead.
yes "Header missing" | head -n 13653 >&2
case "$*" in
    *unsupported*) echo "Decoder not found" >&2; exit 1 ;;
esac
head -c 8000 /dev/zero
