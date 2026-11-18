#!/usr/bin/env bash
# Builds SQLCipher 4.9.0 as a static libsqlite3.a, plus its crypto provider, for one Rust target
# triple. Output: native/out/<triple>/lib/ (libsqlite3.a, and libcrypto.a where OpenSSL is used) and
# native/out/<triple>/include/ (sqlite3.h, sqlite3ext.h, and openssl/*.h where OpenSSL is used).
#
# Usage: scripts/build-sqlcipher.sh <rust-target-triple>
#
# Crypto provider per target:
#   apple (aarch64/x86_64-apple-darwin, aarch64-apple-ios, aarch64-apple-ios-sim)
#                                 CommonCrypto (SQLCIPHER_CRYPTO_CC). Needs no OpenSSL; link the
#                                 Security and CoreFoundation frameworks.
#   android (4 triples, NDK 28.2.13676358, API 26), linux-gnu, windows-msvc (mingw gcc)
#                                 OpenSSL 3.5.9 static libcrypto (SQLCIPHER_CRYPTO_OPENSSL).
#
# Why not libtomcrypt: SQLCipher 4.9.0's libtomcrypt provider hangs inside sqlite3_open_v2(). Its
# activation calls sqlite3_randomness() from inside SQLITE_EXTRA_INIT (reproduced on darwin/arm64).
# Defining SQLCIPHER_TEST avoids the hang but drops the OS entropy seed, so it is not an option.
#
# Environment:
#   ANDROID_NDK_HOME  NDK root (default ~/Library/Android/sdk/ndk/28.2.13676358)
#   CC, AR, RANLIB, NM  Windows and Linux toolchain overrides (Windows default: x86_64-w64-mingw32-*)
#
# Downloads (~75 MB) are sha256-verified and cached in native/sqlcipher/downloads/. Build trees go
# to native/sqlcipher/. Both are gitignored. The script is idempotent: it returns at once when the
# outputs exist and are newer than this script.
set -euo pipefail

SQLCIPHER_COMMIT=c7e811b399379c948b423872ad7ba91d2ce38434 # tag v4.9.0
SQLCIPHER_URL="https://github.com/sqlcipher/sqlcipher/archive/$SQLCIPHER_COMMIT.tar.gz"
SQLCIPHER_SHA256=41bfb007aebc5426a428c9209d23c34d142c2d770c5734e97f6af4597da1fe78
OPENSSL_VERSION=3.5.9
OPENSSL_URL="https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSION/openssl-$OPENSSL_VERSION.tar.gz"
OPENSSL_SHA256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a

# Compile-time options, as in the reference build. SQLITE_HAS_CODEC and the SQLCipher provider define
# come from the target; the rest is shared. FTS5, JSON1 and SECURE_DELETE are required by the core.
SQLITE_DEFINES=(
	-DSQLITE_HAS_CODEC
	-DSQLITE_EXTRA_INIT=sqlcipher_extra_init
	-DSQLITE_EXTRA_SHUTDOWN=sqlcipher_extra_shutdown
	-DSQLITE_TEMP_STORE=3
	-DSQLITE_ENABLE_FTS5
	-DSQLITE_ENABLE_JSON1
	-DSQLITE_DEFAULT_WAL_SYNCHRONOUS=1
	-DSQLITE_SECURE_DELETE
	-DSQLITE_THREADSAFE=1
	-DSQLITE_USE_URI=1
	-DHAVE_USLEEP=1
)

die() {
	echo "build-sqlcipher.sh: $*" >&2
	exit 1
}

usage() {
	echo "usage: $0 <rust-target-triple>" >&2
	exit 2
}

[[ $# -eq 1 ]] || usage
triple="$1"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$root/scripts/build-sqlcipher.sh"
native="$root/native"
work="$native/sqlcipher"
out="$native/out/$triple"
host_os="$(uname -s)"
jobs="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)"

# Set by select_target. Empty NDK_BIN means no Android toolchain is in use.
TARGET_FLAGS=()
CC_BIN="" AR_BIN="" RANLIB_BIN="" NM_BIN=""
CRYPTO="" OPENSSL_TARGET="" OPENSSL_FLAGS=""
NDK_BIN=""

require_tool() {
	command -v "$1" >/dev/null 2>&1 || die "$1 not found: $2"
}

# select_target: maps the triple to its toolchain, flags, crypto provider and OpenSSL target.
select_target() {
	case "$triple" in
	aarch64-apple-darwin | x86_64-apple-darwin)
		[[ "$host_os" == Darwin ]] || die "$triple needs a macOS host"
		local arch=arm64
		[[ "$triple" == x86_64-* ]] && arch=x86_64
		export MACOSX_DEPLOYMENT_TARGET=11.0
		CC_BIN="$(xcrun --find clang)"
		AR_BIN="$(xcrun --find ar)"
		RANLIB_BIN="$(xcrun --find ranlib)"
		NM_BIN="$(xcrun --find nm)"
		TARGET_FLAGS=(-arch "$arch" -mmacosx-version-min=11.0 -isysroot "$(xcrun --show-sdk-path)")
		CRYPTO=commoncrypto
		;;
	aarch64-apple-ios | aarch64-apple-ios-sim)
		[[ "$host_os" == Darwin ]] || die "$triple needs a macOS host with full Xcode"
		local sdk=iphoneos minflag="-miphoneos-version-min=15.0"
		[[ "$triple" == aarch64-apple-ios-sim ]] && sdk=iphonesimulator minflag="-mios-simulator-version-min=15.0"
		local sdk_path
		sdk_path="$(xcrun --sdk "$sdk" --show-sdk-path 2>/dev/null)" ||
			die "$triple needs full Xcode with the $sdk SDK (xcode-select -p shows $(xcode-select -p 2>/dev/null || echo none)); build it in CI"
		CC_BIN="$(xcrun --sdk "$sdk" --find clang)"
		AR_BIN="$(xcrun --sdk "$sdk" --find ar)"
		RANLIB_BIN="$(xcrun --sdk "$sdk" --find ranlib)"
		NM_BIN="$(xcrun --sdk "$sdk" --find nm)"
		TARGET_FLAGS=(-arch arm64 -isysroot "$sdk_path" "$minflag")
		CRYPTO=commoncrypto
		;;
	aarch64-linux-android | armv7-linux-androideabi | x86_64-linux-android | i686-linux-android)
		local ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}" api_triple ndk_host
		case "$host_os" in
		Darwin) ndk_host=darwin-x86_64 ;;
		Linux) ndk_host=linux-x86_64 ;;
		*) die "Android builds need a macOS or Linux host" ;;
		esac
		[[ -d "$ndk" ]] || die "Android NDK not found at $ndk (set ANDROID_NDK_HOME)"
		export ANDROID_NDK_HOME="$ndk"
		NDK_BIN="$ndk/toolchains/llvm/prebuilt/$ndk_host/bin"
		case "$triple" in
		aarch64-linux-android) api_triple=aarch64-linux-android OPENSSL_TARGET=android-arm64 ;;
		armv7-linux-androideabi) api_triple=armv7a-linux-androideabi OPENSSL_TARGET=android-arm ;;
		x86_64-linux-android) api_triple=x86_64-linux-android OPENSSL_TARGET=android-x86_64 ;;
		i686-linux-android) api_triple=i686-linux-android OPENSSL_TARGET=android-x86 ;;
		esac
		CC_BIN="$NDK_BIN/${api_triple}26-clang"
		AR_BIN="$NDK_BIN/llvm-ar"
		RANLIB_BIN="$NDK_BIN/llvm-ranlib"
		NM_BIN="$NDK_BIN/llvm-nm"
		[[ -x "$CC_BIN" ]] || die "NDK compiler not found: $CC_BIN"
		CRYPTO=openssl
		OPENSSL_FLAGS="-D__ANDROID_API__=26"
		;;
	x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | armv7-unknown-linux-gnueabihf)
		[[ "$host_os" == Linux || -n "${CC:-}" ]] || die "$triple builds need a Linux host, or CC set to a cross compiler"
		CC_BIN="${CC:-cc}"
		AR_BIN="${AR:-ar}"
		RANLIB_BIN="${RANLIB:-ranlib}"
		NM_BIN="${NM:-nm}"
		case "$triple" in
		x86_64-*) OPENSSL_TARGET=linux-x86_64 ;;
		aarch64-*) OPENSSL_TARGET=linux-aarch64 ;;
		armv7-*) OPENSSL_TARGET=linux-armv4 ;;
		esac
		CRYPTO=openssl
		;;
	x86_64-pc-windows-msvc | i686-pc-windows-msvc)
		# cgo and SQLCipher need a GNU toolchain. MSVC cannot build this.
		if [[ "$triple" == i686-* ]]; then
			CC_BIN="${CC:-i686-w64-mingw32-gcc}"
			AR_BIN="${AR:-i686-w64-mingw32-ar}"
			RANLIB_BIN="${RANLIB:-i686-w64-mingw32-ranlib}"
			NM_BIN="${NM:-i686-w64-mingw32-nm}"
			OPENSSL_TARGET=mingw
		else
			CC_BIN="${CC:-x86_64-w64-mingw32-gcc}"
			AR_BIN="${AR:-x86_64-w64-mingw32-ar}"
			RANLIB_BIN="${RANLIB:-x86_64-w64-mingw32-ranlib}"
			NM_BIN="${NM:-x86_64-w64-mingw32-nm}"
			OPENSSL_TARGET=mingw64
		fi
		CRYPTO=openssl
		command -v "$CC_BIN" >/dev/null 2>&1 ||
			die "$triple needs a mingw-w64 gcc (for example: brew install mingw-w64, or set CC) on this host"
		;;
	aarch64-pc-windows-msvc)
		die "$triple: OpenSSL $OPENSSL_VERSION has no Windows ARM64 Configure target, so libcrypto cannot be built"
		;;
	*)
		die "unsupported target triple: $triple"
		;;
	esac
}

sha256_of() {
	# sha256sum ships with Git Bash and Linux; shasum is the macOS fallback.
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	else
		shasum -a 256 "$1" | cut -d' ' -f1
	fi
}

# fetch <url> <sha256> <dest>: downloads once, refuses anything whose hash differs from the pin.
fetch() {
	local url=$1 want=$2 dest=$3
	if [[ -f "$dest" && "$(sha256_of "$dest")" == "$want" ]]; then
		return
	fi
	curl -fsSL --retry 3 -o "$dest.part" "$url"
	if [[ "$(sha256_of "$dest.part")" != "$want" ]]; then
		rm -f "$dest.part"
		die "sha256 mismatch for $url"
	fi
	mv "$dest.part" "$dest"
}

# extract <tarball> <dest>: unpacks a release archive (single top-level directory) into dest.
extract() {
	rm -rf "$2"
	mkdir -p "$2"
	tar xzf "$1" -C "$2" --strip-components=1
}

# generate_amalgamation: SQLCipher's sqlite3.c is produced on the host (needs tclsh). The result is
# target-independent and is compiled once per target.
generate_amalgamation() {
	local src="$work/src/sqlcipher" amalg="$work/amalgamation"
	[[ -f "$amalg/sqlite3.c" ]] && return
	extract "$work/downloads/sqlcipher.tar.gz" "$src"
	(cd "$src" && ./configure --disable-tcl >"$work/configure.log" 2>&1 && make sqlite3.c >"$work/amalgamation.log" 2>&1) ||
		die "SQLCipher amalgamation failed; see $work/configure.log and $work/amalgamation.log"
	mkdir -p "$amalg"
	cp "$src/sqlite3.c" "$src/sqlite3.h" "$src/sqlite3ext.h" "$amalg/"
}

# build_openssl: static libcrypto for $triple, with no-shared, no-dso and no-engine so that the
# library links into the Go c-shared object or the Go archive without a runtime dependency.
build_openssl() {
	local build="$work/build/$triple/openssl"
	extract "$work/downloads/openssl.tar.gz" "$build"
	(
		cd "$build"
		if [[ -n "$NDK_BIN" ]]; then
			export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME" PATH="$NDK_BIN:$PATH" # OpenSSL finds clang via PATH
		else
			export CC="$CC_BIN" AR="$AR_BIN" RANLIB="$RANLIB_BIN"
		fi
		# shellcheck disable=SC2086
		./Configure "$OPENSSL_TARGET" $OPENSSL_FLAGS no-shared no-dso no-engine no-tests no-apps no-docs \
			>"$work/build/$triple/configure.log" 2>&1
		make -j"$jobs" build_libs >"$work/build/$triple/make.log" 2>&1
	) || die "OpenSSL $OPENSSL_VERSION build failed; see $work/build/$triple/make.log"
	cp "$build/libcrypto.a" "$out/lib/"
	mkdir -p "$out/include/openssl"
	cp "$build/include/openssl/"*.h "$out/include/openssl/"
}

# build_sqlcipher: compiles the amalgamation for $triple and archives it as libsqlite3.a.
build_sqlcipher() {
	local build="$work/build/$triple" amalg="$work/amalgamation"
	local provider=-DSQLCIPHER_CRYPTO_OPENSSL
	[[ "$CRYPTO" == commoncrypto ]] && provider=-DSQLCIPHER_CRYPTO_CC
	mkdir -p "$build"
	cp "$amalg/sqlite3.h" "$amalg/sqlite3ext.h" "$out/include/"
	"$CC_BIN" -c "${SQLITE_DEFINES[@]}" "$provider" ${TARGET_FLAGS[@]+"${TARGET_FLAGS[@]}"} \
		-O2 -fPIC -include stdint.h -I"$out/include" "$amalg/sqlite3.c" -o "$build/sqlite3.o"
	"$AR_BIN" rcs "$out/lib/libsqlite3.a" "$build/sqlite3.o"
	"$RANLIB_BIN" "$out/lib/libsqlite3.a"

	# Sanity check: the archive contains the SQLCipher keying entry point.
	local symbols
	symbols="$("$NM_BIN" "$out/lib/libsqlite3.a")"
	grep -q ' T _\{0,1\}sqlite3_key$' <<<"$symbols" || die "sqlite3_key missing from libsqlite3.a"
}

# up_to_date: true when every output exists and is newer than this script.
up_to_date() {
	local f
	local outputs=("$out/lib/libsqlite3.a" "$out/include/sqlite3.h")
	[[ "$CRYPTO" == openssl ]] && outputs+=("$out/lib/libcrypto.a")
	for f in "${outputs[@]}"; do
		[[ -f "$f" && "$f" -nt "$script" ]] || return 1
	done
}

main() {
	select_target
	if up_to_date; then
		echo "up to date: $out"
		return
	fi
	mkdir -p "$work/downloads"
	fetch "$SQLCIPHER_URL" "$SQLCIPHER_SHA256" "$work/downloads/sqlcipher.tar.gz"
	require_tool tclsh "needed to generate the SQLCipher amalgamation"
	generate_amalgamation

	rm -rf "$out" "$work/build/$triple"
	mkdir -p "$out/lib" "$out/include" "$work/build/$triple"
	if [[ "$CRYPTO" == openssl ]]; then
		fetch "$OPENSSL_URL" "$OPENSSL_SHA256" "$work/downloads/openssl.tar.gz"
		build_openssl
	fi
	build_sqlcipher
	echo "built $out"
}

main
