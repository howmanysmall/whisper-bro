#!/bin/bash
cd "$(dirname "$0")" || exit 1
"./Whisper Bro.app/Contents/MacOS/whisper-bro" setup
result=$?
read -r -p "Press Enter to close."
exit "$result"
