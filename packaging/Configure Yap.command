#!/bin/bash
cd "$(dirname "$0")" || exit 1
binary="./Yap.app/Contents/MacOS/yap"
if [[ ! -x "$binary" ]]; then
	binary="/Applications/Yap.app/Contents/MacOS/yap"
fi
"$binary" setup
result=$?
read -r -p "Press Enter to close."
exit "$result"
