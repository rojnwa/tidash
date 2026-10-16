#!/usr/bin/env bash
# The performance budget (spec §10) on macOS and Linux, after `cargo build --release -p tidash`:
#   bash packaging/budget.sh target/release/tidash
# Windows has budget.ps1.
#
# It starts Tidash on the user's own profile, so log in first and quit any running Tidash. It waits
# 15 s for Home, measures 10 s of idle, ends Tidash and prints:
# - first frame: start to the first drawn frame (target < 500 ms with a warm cache; the first launch
#   of a new build is slower, so run it again if that one misses);
# - RAM after Home: the physical footprint (`footprint`) on macOS, Pss on Linux (target < 150 MB);
# - idle passes in 10 s: frames drawn while nothing changes (target 0);
# - CPU idle: CPU time over those 10 s, in % of one core (no target of its own; expect ~0 %);
# - binary: the executable's size (the 20 MB target is the installer's).
# It exits 1 if a target is missed, 2 if it could not measure. Still by hand, as docs/releasing.md
# lists: CPU paused with the window hidden (0 %), CPU playing with the window minimized (< 1 %), and
# scroll pacing (PresentMon, Windows).
set -euo pipefail

if [ $# -ne 1 ] || [ ! -x "$1" ]; then
    echo "usage: budget.sh <tidash binary>" >&2
    exit 2
fi
bin=$1
os=$(uname -s)
case $os in
    Darwin | Linux) ;;
    *) echo "budget.sh: macOS and Linux only; Windows has budget.ps1" >&2; exit 2 ;;
esac

log=$(mktemp)
TIDASH_REPAINT_LOG=1 RUST_LOG=tidash=info "$bin" 2>"$log" &
pid=$!
# The wait keeps bash from reporting the job's "Terminated".
trap '{ kill "$pid" && wait "$pid"; } 2>/dev/null || true; rm -f "$log"' EXIT

# Every awk runs with LC_ALL=C: macOS awk reads and prints decimals with the locale's separator.
mib() {
    if [ "$os" = Darwin ]; then
        footprint -f bytes "$pid" | LC_ALL=C awk '$1 == "phys_footprint:" { print $2 / 1048576 }'
    else
        LC_ALL=C awk '$1 == "Pss:" { print $2 / 1024 }' "/proc/$pid/smaps_rollup"
    fi
}

cpu_seconds() {
    if [ "$os" = Darwin ]; then
        # [hh:]mm:ss.hh of user + system time
        ps -o time= -p "$pid" | LC_ALL=C awk '{
            n = split($1, t, ":"); s = 0
            for (i = 1; i <= n; i++) s = s * 60 + t[i]
            print s
        }'
    else
        # utime and stime, fields 14 and 15 in clock ticks, counted after the ")" that ends the name
        sed 's/.*) //' "/proc/$pid/stat" |
            LC_ALL=C awk -v hz="$(getconf CLK_TCK)" '{ print ($12 + $13) / hz }'
    fi
}

passes() { grep -c 'repaint causes' "$log" || true; }

sleep 15
if ! kill -0 "$pid" 2>/dev/null; then
    echo "budget.sh: Tidash ended early; is another Tidash running?" >&2
    exit 2
fi
ram=$(mib)
passes1=$(passes)
cpu1=$(cpu_seconds)
sleep 10
passes2=$(passes)
cpu2=$(cpu_seconds)
first=$(sed -n 's/.*first frame \([0-9]*\) ms after start.*/\1/p' "$log" | head -n 1)
size=$(wc -c <"$bin")

LC_ALL=C awk -v first="${first:--1}" -v ram="$ram" -v idle=$((passes2 - passes1)) \
    -v cpu1="$cpu1" -v cpu2="$cpu2" -v size="$size" 'BEGIN {
    if (first < 0) print "first frame: none"; else printf "first frame: %d ms\n", first
    printf "RAM after Home: %.1f MB\n", ram
    printf "idle passes in 10 s: %d\n", idle
    printf "CPU idle: %.1f %% of one core\n", (cpu2 - cpu1) * 10
    # 10^6 bytes, as spec §10 counts binary sizes; RAM is in MiB.
    printf "binary: %.1f MB\n", size / 1e6
    if (first < 0 || first >= 500) missed = missed ", first frame"
    if (ram >= 150) missed = missed ", RAM"
    if (idle > 0) missed = missed ", idle passes"
    if (missed != "") { print "missed: " substr(missed, 3) > "/dev/stderr"; exit 1 }
}'
