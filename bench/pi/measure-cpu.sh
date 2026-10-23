#!/bin/sh
# Sample aggregate CPU ticks for one or more PIDs.
# usage: measure-cpu.sh SECONDS PID...
set -eu
seconds=$1
shift
if [ "$#" -eq 0 ]; then
    echo "usage: $0 SECONDS PID..." >&2
    exit 2
fi
clock=$(getconf CLK_TCK)
ticks() {
    if [ -r "/proc/$1/stat" ]; then
        awk '{print $14 + $15}' "/proc/$1/stat"
    else
        echo 0
    fi
}
before=0
for pid in "$@"; do
    before=$((before + $(ticks "$pid")))
done
sleep "$seconds"
after=0
for pid in "$@"; do
    after=$((after + $(ticks "$pid")))
done
delta=$((after - before))
awk -v ticks="$delta" -v hz="$clock" -v secs="$seconds" '
    BEGIN {
        printf "cpu_ticks=%d cpu_seconds=%.2f cpu_percent=%.3f\n",
            ticks, ticks / hz, ticks / (hz * secs) * 100
    }'
