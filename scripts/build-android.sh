#!/usr/bin/env bash
# Build the Android app with the all-Rust core.
#
# SQLCipher's vendored OpenSSL is built by `make`, which looks for `<triple>-ranlib` and `<triple>-ar`
# on PATH. The NDK only ships the LLVM tools, so point every Android target at them.
#
# Usage: scripts/build-android.sh [cargo tauri android build arguments...]
#   e.g. scripts/build-android.sh --debug --target aarch64 --apk
set -euo pipefail

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
ndk="${NDK_HOME:-${ANDROID_NDK_HOME:-$sdk/ndk/28.2.13676358}}"
if [ ! -d "$ndk" ]; then
  echo "Android NDK not found at $ndk (set NDK_HOME)" >&2
  exit 1
fi

case "$(uname -s)" in
  Darwin) host="darwin-x86_64" ;;
  Linux) host="linux-x86_64" ;;
  *) echo "unsupported build host $(uname -s)" >&2; exit 1 ;;
esac
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"

export ANDROID_HOME="$sdk" NDK_HOME="$ndk"
for triple in aarch64_linux_android armv7_linux_androideabi i686_linux_android x86_64_linux_android; do
  export "AR_${triple}=$bin/llvm-ar"
  export "RANLIB_${triple}=$bin/llvm-ranlib"
done

cd "$(dirname "$0")/.."
exec cargo tauri android build "$@"
