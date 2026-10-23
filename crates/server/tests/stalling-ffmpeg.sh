#!/bin/sh
# Stands in for ffmpeg in the tests of the server's stop. It says it has started by creating its
# input's path with `.started` added, then runs without a word until it is killed, or for 10 seconds,
# so a failed test leaves it behind for no longer.
for arg; do
    [ "$prev" = -i ] && input=${arg#file:}
    prev=$arg
done
touch "$input.started"
exec sleep 10
