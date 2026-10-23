#!/bin/sh
# Stands in for ffmpeg in the image cache's tests. Its input goes away as it reads it, as a file on a
# share that drops does, and it fails as ffmpeg would.
for arg; do
    [ "$prev" = -i ] && input=${arg#file:}
    prev=$arg
done
rm "$input"
echo "$input: Input/output error" >&2
exit 1
