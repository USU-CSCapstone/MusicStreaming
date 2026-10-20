#!/bin/sh
# Stands in for ffmpeg in the decode tests. It writes 13,653 whole lines of errors (200 KB) to
# stderr, more than a pipe holds, then decodes to 1000 frames of stereo f32: 8000 bytes of the
# lines 0000 to 1999, so no frame repeats another. An input path containing "unsupported" fails the
# way a missing decoder does instead; one containing "split" writes 4 bytes and then 8, pausing
# after each so reads end part way through a frame, before the rest; and one containing "trailing"
# ends with half a frame.

# Writes `$2` bytes of the output, from byte `$1`.
pcm() { seq -w 0 1999 | tail -c +$(($1 + 1)) | head -c "$2"; }

yes "Header missing" | head -n 13653 >&2
case "$*" in
    *unsupported*) echo "Decoder not found" >&2; exit 1 ;;
    *split*) pcm 0 4; sleep 0.1; pcm 4 8; sleep 0.1; pcm 12 7988; exit ;;
    *trailing*) pcm 0 8004; exit ;;
esac
pcm 0 8000
