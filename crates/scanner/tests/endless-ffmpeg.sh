#!/bin/sh
# Stands in for ffprobe and ffmpeg in the test that a stop ends an analysis between reads. Asked
# to probe, it describes one mono track at 8000 Hz. Asked to decode, it writes a block of silence
# every 50 ms until it is killed (a closed pipe ends it too), or for 10 seconds, so a failed
# test leaves it behind for no longer.
case "$*" in
    *-print_format*)
        echo '{"streams":[{"codec_type":"audio","sample_rate":"8000","channels":1}],"format":{}}'
        exit ;;
esac
for arg; do
    [ "$prev" = -i ] && input=${arg#file:}
    prev=$arg
done
touch "$input.started"
i=0
while [ $i -lt 200 ]; do
    head -c 4096 /dev/zero || exit
    sleep 0.05
    i=$((i + 1))
done
