#!/bin/sh
# ffprobe for the tests: killed by a signal when the file is named `killed*`, the real one
# otherwise. The file is the last argument.
for file; do :; done
case "${file##*/}" in
    killed*) kill -9 $$ ;;
esac
exec ffprobe "$@"
