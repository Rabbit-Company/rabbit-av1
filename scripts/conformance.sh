#!/usr/bin/env bash
# Parses every AV1 stream below a folder with the av1dump example and checks that it finds
# as many shown pictures as dav1d decodes.
#
#   scripts/conformance.sh <folder> [av1dump binary]
#
# The libaom test vectors are listed in libaom's test/test_vectors.cc and can be
# downloaded from https://storage.googleapis.com/aom-test-data/<name>.
set -u
folder=${1:?usage: conformance.sh <folder> [av1dump binary]}
dump=${2:-target/release/examples/av1dump}

decoded_frames() {
    local format=()
    # ffmpeg reads Annex B only when told to.
    case "$1" in *annexb*) format=(-f av1) ;; esac
    ffmpeg -v error -nostdin -c:v libdav1d "${format[@]}" -i "$1" -map 0:V:0 -f framemd5 - 2>/dev/null | grep -cv '^#'
}

passed=0
failed=0
skipped=0
while IFS= read -r input; do
    name=${input#"$folder"/}
    expected=$(decoded_frames "$input")
    if [ "${expected:-0}" -eq 0 ]; then
        echo "SKIP $name: dav1d decodes no frames"
        skipped=$((skipped + 1))
        continue
    fi
    if ! output=$("$dump" "$input" 2>&1); then
        failed=$((failed + 1))
        echo "FAIL $name: $(echo "$output" | tail -1)"
        continue
    fi
    shown=$(echo "$output" | sed -n 's/.* coded frames, \([0-9]*\) shown pictures.*/\1/p')
    if [ "$shown" = "$expected" ]; then
        passed=$((passed + 1))
    else
        failed=$((failed + 1))
        echo "FAIL $name: $shown shown pictures, dav1d decodes $expected"
    fi
done < <(find "$folder" -type f \( -name '*.ivf' -o -name '*.mkv' -o -name '*.webm' -o -name '*.obu' \) | sort)
echo "$passed passed, $failed failed, $skipped skipped"
[ "$failed" -eq 0 ]
