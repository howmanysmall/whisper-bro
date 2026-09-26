#!/bin/bash
cd "$(dirname "$0")" || exit 1
binary="./Whisper Bro.app/Contents/MacOS/whisper-bro"
if [[ ! -x "$binary" ]]; then
    binary="/Applications/Whisper Bro.app/Contents/MacOS/whisper-bro"
fi
"$binary" setup
result=$?
read -r -p "Press Enter to close."
exit "$result"
