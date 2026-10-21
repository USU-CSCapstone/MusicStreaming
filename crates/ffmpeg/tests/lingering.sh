#!/bin/sh
# Stands in for ffmpeg and ffprobe in the registry's tests. It waits about 5 seconds at most for its
# input file to exist, then closes its output and lives on for 2 seconds more, so a test can see whether
# it is unregistered before it is reaped.
for arg; do
    [ "$prev" = -i ] && input=${arg#file:}
    prev=$arg
done
for _ in $(seq 50); do [ -e "$input" ] && break; sleep 0.1; done
exec sleep 2 >&- 2>&-
