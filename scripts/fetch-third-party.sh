#!/usr/bin/env bash
# Clones third-party repos at pinned commits (third_party/pins.txt).
set -euo pipefail
cd "$(dirname "$0")/.."; mkdir -p third_party
while read -r name sha; do
  dir="third_party/$name"; url="https://github.com/mautrix/$name"; [ "$name" = mautrix-go ] && url=https://github.com/mautrix/go
  [ -d "$dir/.git" ] || git clone -q "$url" "$dir"
  git -C "$dir" fetch -q --depth 1 origin "$sha" && git -C "$dir" checkout -q "$sha"
done < third_party/pins.txt
