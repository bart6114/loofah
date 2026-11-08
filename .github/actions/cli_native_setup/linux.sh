#!/usr/bin/env bash
set -euo pipefail

target="${1:?Pass the Linux Rust target triple}"
case "$target" in
  x86_64-unknown-linux-gnu)
    archive=onnxruntime-linux-x64-static_lib-1.28.0-glibc2_28.zip
    checksum=204cd1ea7efdc76057974afe6a97141666fbed8cf5614fc52b305667a621c119
    ;;
  aarch64-unknown-linux-gnu)
    archive=onnxruntime-linux-aarch64-static_lib-1.28.0-glibc2_28.zip
    checksum=31c56eaa8c79723555e2284b251e09a9924bf106560a2d6e4e93da6ddf44b4f2
    ;;
  *) echo "Unsupported Linux target: $target" >&2; exit 1 ;;
esac

destination="${RUNNER_TEMP:?}/cli-onnxruntime"
environment="${GITHUB_ENV:?}"
mkdir -p "$destination"
curl --proto '=https' --tlsv1.2 --location --fail --retry 5 \
  "https://github.com/csukuangfj/onnxruntime-libs/releases/download/v1.28.0/$archive" \
  --output "$destination/$archive"
printf '%s  %s\n' "$checksum" "$destination/$archive" | sha256sum --check --strict
unzip -q -o "$destination/$archive" -d "$destination"
library_dir="$destination/${archive%.zip}/lib"
test -s "$library_dir/libonnxruntime.a"

privileged() {
  if [[ "$EUID" -eq 0 ]]; then "$@"; else sudo "$@"; fi
}
if ! command -v g++-14 >/dev/null; then
  privileged add-apt-repository -y ppa:ubuntu-toolchain-r/test
  privileged apt-get update
  privileged apt-get install -y g++-14
fi

# ONNX needs the newer C++ ABI; retain the existing CLI's static runtime policy.
cxx_library="$(g++-14 -print-file-name=libstdc++.a)"
test -s "$cxx_library"
cxx_hash="$(sha256sum "$cxx_library" | awk '{print $1}')"
target_env="$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')"
{
  echo "ORT_LIB_PATH=$library_dir"
  echo "CC=gcc-14"
  echo "CXX=g++-14"
  echo "CARGO_TARGET_${target_env}_LINKER=gcc-14"
  echo "ORT_CXX_STDLIB=static=stdc++"
  echo "RUSTFLAGS=${RUSTFLAGS:-} -L native=$(dirname "$cxx_library")"
  echo "LOOFAH_NATIVE_CACHE=onnxruntime-$checksum-static-cxx-$cxx_hash"
} >> "$environment"
