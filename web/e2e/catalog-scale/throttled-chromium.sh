#!/bin/sh
# Runs the Chromium named by $CHROMIUM with all its processes together limited to a quarter of one
# CPU, in slices of 4 ms: a phone stand-in that reaches workers, which Chrome DevTools'
# CPU throttling does not. Needs Linux with systemd.
exec systemd-run --user --scope --quiet -p CPUQuota=25% -p CPUQuotaPeriodSec=4ms -- "$CHROMIUM" "$@"
