#!/usr/bin/env bash
# Renders every scene of a wallpaper folder with wepaper and compares each frame with a
# reference frame of the same name, when there is one.
#
#   tools/golden.sh <workshop dir> <reference dir> <output dir> [wepaper binary]
#
# Writes <output dir>/<id>.png, <id>.log and summary.tsv (id, status, difference, ms),
# sorted worst first. The difference is the mean absolute error, 0 (same) to 1.
set -uo pipefail
workshop=${1:?workshop dir} reference=${2:?reference dir} out=${3:?output dir}
wepaper=${4:-$(dirname "$0")/../target/release/wepaper}
mkdir -p "$out"
: > "$out/summary.unsorted"
for dir in "$workshop"/*/; do
    id=$(basename "$dir")
    grep -qi '"type"[[:space:]]*:[[:space:]]*"scene"' "$dir/project.json" 2>/dev/null || continue
    start=$(date +%s%N)
    if timeout 60 "$wepaper" -v render "$dir" -o "$out/$id.png" --size 1920x1080 --time 2 --frames 4 --stats >"$out/$id.log" 2>&1; then
        status=ok
    else
        status=FAILED
    fi
    ms=$(( ($(date +%s%N) - start) / 1000000 ))
    difference=-
    if [[ $status == ok && -s $reference/$id.png ]]; then
        difference=$(compare -metric MAE "$reference/$id.png" "$out/$id.png" null: 2>&1 | sed -n 's/.*(\(.*\)).*/\1/p')
    fi
    warnings=$(grep -c 'warn:' "$out/$id.log")
    printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$status" "${difference:--}" "$ms" "$warnings" >> "$out/summary.unsorted"
done
LC_ALL=C sort -t$'\t' -k3,3 -g -r "$out/summary.unsorted" > "$out/summary.tsv"
rm -f "$out/summary.unsorted"
echo "scenes: $(wc -l < "$out/summary.tsv"), failed: $(grep -c FAILED "$out/summary.tsv")"
