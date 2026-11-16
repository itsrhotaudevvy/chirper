#!/usr/bin/env bash
# Builds the Go core C ABI (go/core/capi) for one Rust target triple into go/out/<triple>/.
#
# Usage: scripts/build-go.sh <rust-target-triple> [--release]
#
# Output: libchirpercore.a (Apple and iOS, c-archive) or libchirpercore.so / chirpercore.dll
# (c-shared), plus chirper.h copied from go/core/capi. Environment:
#   CHIRPER_TG_API_ID, CHIRPER_TG_API_HASH  build-time Telegram credentials (may be empty; never printed)
#   CHIRPER_GO_TAGS                         extra Go build tags
#   ANDROID_NDK_HOME                        NDK root (default ~/Library/Android/sdk/ndk/28.2.13676358)
#   CC                                      C compiler override (required for Windows unless on MinGW/MSYS)
#   CHIRPER_LIBSIGNAL_DIR                   directory with libsignal_ffi.a built for the Android target.
#                                           Android builds include Signal, which links it (see third_party/signal).
#   CHIRPER_SQLCIPHER                       1 links the encrypted SQLite from native/out/<triple> (see
#                                           scripts/build-sqlcipher.sh) and adds the sqlcipher,libsqlite3 tags.
#                                           Defaults to 1 for --release, 0 otherwise. Set 0 to opt out of a
#                                           release build's encryption; that build then stores databases in
#                                           plaintext and is for local testing only.
#                                           Apple c-archives fold libsqlite3.a into libchirpercore.a, so Rust
#                                           links that one archive plus the Security and CoreFoundation frameworks.
set -euo pipefail
export GODEBUG=http2client=0

usage() {
	echo "usage: $0 <rust-target-triple> [--release]" >&2
	exit 2
}

die() {
	echo "build-go.sh: $*" >&2
	exit 1
}

[[ $# -ge 1 ]] || usage
triple="$1"
shift
release=0
for arg in "$@"; do
	case "$arg" in
	--release) release=1 ;;
	*) usage ;;
	esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
core_dir="$root/go/core"
out_dir="$root/go/out/$triple"
host_os="$(uname -s)"

goarm=""
cc="${CC:-}"
cgo_cflags=""
extra_ldflags=""
cgo_ldflags=""
case "$triple" in
aarch64-apple-darwin | x86_64-apple-darwin)
	goos=darwin
	goarch=$([[ "$triple" == aarch64-* ]] && echo arm64 || echo amd64)
	mode=c-archive
	lib=libchirpercore.a
	export MACOSX_DEPLOYMENT_TARGET=11.0
	cgo_cflags="-mmacosx-version-min=11.0"
	;;
aarch64-apple-ios | aarch64-apple-ios-sim | x86_64-apple-ios)
	goos=ios
	mode=c-archive
	lib=libchirpercore.a
	case "$triple" in
	aarch64-apple-ios) goarch=arm64 sdk=iphoneos arch=arm64 minflag="-miphoneos-version-min=15.0" ;;
	aarch64-apple-ios-sim) goarch=arm64 sdk=iphonesimulator arch=arm64 minflag="-mios-simulator-version-min=15.0" ;;
	x86_64-apple-ios) goarch=amd64 sdk=iphonesimulator arch=x86_64 minflag="-mios-simulator-version-min=15.0" ;;
	esac
	[[ "$host_os" == Darwin ]] || die "iOS targets need macOS with full Xcode"
	sdk_path="$(xcrun --sdk "$sdk" --show-sdk-path 2>/dev/null)" ||
		die "iOS builds need full Xcode with the $sdk SDK (check xcode-select -p)"
	clang="$(xcrun --sdk "$sdk" --find clang 2>/dev/null)" || die "no clang for the $sdk SDK"
	cc="$clang -arch $arch -isysroot $sdk_path $minflag"
	cgo_cflags="$minflag"
	;;
aarch64-linux-android | armv7-linux-androideabi | x86_64-linux-android | i686-linux-android)
	goos=android
	mode=c-shared
	lib=libchirpercore.so
	api=26
	ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
	[[ -d "$ndk" ]] || die "Android NDK not found at $ndk (set ANDROID_NDK_HOME)"
	case "$host_os" in
	Darwin) ndk_host=darwin ;;
	Linux) ndk_host=linux ;;
	*) die "Android builds need a macOS or Linux host" ;;
	esac
	case "$triple" in
	aarch64-linux-android) goarch=arm64 clang_triple=aarch64-linux-android ;;
	armv7-linux-androideabi) goarch=arm goarm=7 clang_triple=armv7a-linux-androideabi ;;
	x86_64-linux-android) goarch=amd64 clang_triple=x86_64-linux-android ;;
	i686-linux-android) goarch=386 clang_triple=i686-linux-android ;;
	esac
	cc="$ndk/toolchains/llvm/prebuilt/$ndk_host-x86_64/bin/$clang_triple$api-clang"
	[[ -x "$cc" ]] || die "NDK compiler not found: $cc"
	extra_ldflags="-extldflags=-Wl,-soname,libchirpercore.so"
	;;
x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu)
	goos=linux
	goarch=$([[ "$triple" == aarch64-* ]] && echo arm64 || echo amd64)
	mode=c-shared
	lib=libchirpercore.so
	[[ "$host_os" == Linux || -n "$cc" ]] || die "linux targets build on a Linux host (or set CC)"
	;;
x86_64-pc-windows-msvc | aarch64-pc-windows-msvc)
	goos=windows
	goarch=$([[ "$triple" == aarch64-* ]] && echo arm64 || echo amd64)
	mode=c-shared
	lib=chirpercore.dll
	# cgo needs a GNU toolchain (mingw gcc), not MSVC. Native Windows CI provides one.
	[[ -n "$cc" || "$host_os" == MINGW* || "$host_os" == MSYS* ]] ||
		die "Windows builds need mingw gcc as CC (for example CC=x86_64-w64-mingw32-gcc)"
	;;
*)
	die "unsupported target triple: $triple"
	;;
esac

# Build-time Telegram credentials. Empty values are valid. They reach the linker and are never echoed.
ldflags="-s -w"
ldflags+=" -X chirper.dev/core/engine.telegramDefaultAPIID=${CHIRPER_TG_API_ID:-}"
ldflags+=" -X chirper.dev/core/engine.telegramDefaultAPIHash=${CHIRPER_TG_API_HASH:-}"
[[ -n "$extra_ldflags" ]] && ldflags+=" $extra_ldflags"

[[ "$release" == 1 ]] && cgo_cflags+=" -O2"

# Signal is opt-in on every target: it links the Rust libsignal static library (libsignal_ffi.a),
# built per target by third_party/signal. Set CHIRPER_LIBSIGNAL_DIR to enable it.
go_tags="${CHIRPER_GO_TAGS:-}"
libsignal_dir="${CHIRPER_LIBSIGNAL_DIR:-}"
if [[ -n "$libsignal_dir" ]]; then
	[[ -f "$libsignal_dir/libsignal_ffi.a" ]] || die "CHIRPER_LIBSIGNAL_DIR has no libsignal_ffi.a: $libsignal_dir"
	cgo_ldflags+=" -L$libsignal_dir"
	go_tags="${go_tags:+$go_tags,}withsignal"
fi
# SQLCipher is on by default for release builds so that a release artifact never stores plaintext.
sqlcipher="${CHIRPER_SQLCIPHER:-$([[ "$release" == 1 ]] && echo 1 || echo 0)}"
case "$sqlcipher" in
0 | 1) ;;
*) die "CHIRPER_SQLCIPHER must be 0 or 1, got: $sqlcipher" ;;
esac
native_out="$root/native/out/$triple"
if [[ "$sqlcipher" == 1 ]]; then
	[[ -f "$native_out/lib/libsqlite3.a" ]] ||
		die "CHIRPER_SQLCIPHER=1 needs $native_out/lib/libsqlite3.a: run scripts/build-sqlcipher.sh $triple"
	go_tags="${go_tags:+$go_tags,}sqlcipher,libsqlite3"
	cgo_cflags+=" -I$native_out/include"
	cgo_ldflags+=" -L$native_out/lib -lsqlite3"
	case "$goos" in
	darwin | ios) cgo_ldflags+=" -framework Security -framework CoreFoundation" ;; # CommonCrypto
	android) cgo_ldflags+=" -lcrypto" ;;
	linux) cgo_ldflags+=" -lcrypto -ldl -lpthread -lm" ;;
	windows) cgo_ldflags+=" -lcrypto -lws2_32 -lgdi32 -lcrypt32" ;;
	esac
elif [[ "$release" == 1 ]]; then
	echo "build-go.sh: warning: release build without SQLCipher; databases will be plaintext" >&2
fi
tag_args=()
[[ -n "$go_tags" ]] && tag_args=(-tags "$go_tags")

mkdir -p "$out_dir"
echo "Building go/core/capi for $triple ($goos/${goarch}, $mode)"
(
	cd "$core_dir"
	export CGO_ENABLED=1 GOOS="$goos" GOARCH="$goarch"
	[[ -n "$goarm" ]] && export GOARM="$goarm"
	[[ -n "$cc" ]] && export CC="$cc"
	[[ -n "$cgo_cflags" ]] && export CGO_CFLAGS="${CGO_CFLAGS:-} $cgo_cflags"
	[[ -n "$cgo_ldflags" ]] && export CGO_LDFLAGS="${CGO_LDFLAGS:-} $cgo_ldflags"
	go build -buildmode="$mode" -trimpath -buildvcs=false ${tag_args[@]+"${tag_args[@]}"} \
		-ldflags "$ldflags" -o "$out_dir/$lib" ./capi
)

# A c-archive holds Go and cgo objects but not the libraries they link against, so libsqlite3.a is
# folded in. The Rust side then links one archive. libtool is Apple's; c-shared outputs link SQLCipher
# statically already.
if [[ "$sqlcipher" == 1 && "$mode" == c-archive ]]; then
	command -v libtool >/dev/null 2>&1 || die "merging libsqlite3.a into $lib needs Apple libtool"
	libtool -static -o "$out_dir/$lib.merged" "$out_dir/$lib" "$native_out/lib/libsqlite3.a"
	mv "$out_dir/$lib.merged" "$out_dir/$lib"
fi

# Ship the hand-maintained header. Go also writes a generated header next to the library; it is
# removed so that only chirper.h is shipped.
cp "$core_dir/capi/chirper.h" "$out_dir/chirper.h"
rm -f "$out_dir/${lib%.*}.h"

echo "Wrote $out_dir/$lib"
