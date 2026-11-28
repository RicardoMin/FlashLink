#!/usr/bin/env bash
# Build the engine in an MSYS2 UCRT64 shell. FlashLink supplies the tray UI.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
work=${1:-"$root/target/sunshine"}
mkdir -p "$work"
work=$(cd "$work" && pwd)
patch="$root/integrations/sunshine/live-settings.patch"
revision=63d35f702ee9e362e43263742981836ec0710384
patch_hash=$(tr -d '\r' < "$patch" | sha256sum | cut -d' ' -f1)
src="$work/src-${patch_hash:0:12}"
build="$work/cmake-build-${patch_hash:0:12}"

if [[ ! -d "$src/.git" ]]; then
    git clone --depth 1 --branch v2026.914.233613 https://github.com/SunshineStream/Sunshine.git "$src"
fi
[[ $(git -C "$src" rev-parse HEAD) == "$revision" ]]
git -C "$src" submodule update --init --depth 1 --jobs 6
git -C "$src" submodule update --init --recursive --depth 1 --jobs 6 -- \
    third-party/moonlight-common-c third-party/libdisplaydevice third-party/libvirtualhid third-party/lizardbyte-common
if git -C "$src" diff --cached --quiet; then
    git -C "$src" apply --index "$patch"
fi
# Require precisely the checked-in changes, including during incremental builds.
expected_index=$(mktemp -d)
trap 'rm -f "$expected_index/index"; rmdir "$expected_index"' EXIT
index_file=$(cygpath -m "$expected_index/index")
GIT_INDEX_FILE="$index_file" git -C "$src" read-tree HEAD
GIT_INDEX_FILE="$index_file" git -C "$src" apply --cached "$patch"
expected_tree=$(GIT_INDEX_FILE="$index_file" git -C "$src" write-tree)
git -C "$src" diff --exit-code --ignore-submodules=untracked "$expected_tree" --

archive="$work/Windows-AMD64-ffmpeg.tar.gz"
if [[ ! -f "$archive" ]]; then
    curl --fail --location --retry 3 -o "$archive.part" \
        https://github.com/LizardByte/build-deps/releases/download/v2026.910.121303/Windows-AMD64-ffmpeg.tar.gz
    mv "$archive.part" "$archive"
fi
echo "ee79a7a295e8fbd24305a3f4ec94a06f550e0b52f49cb1c4535f649e2dd8b234  $archive" | sha256sum -c -
tar -xzf "$archive" -C "$work"
export BUILD_VERSION=v2026.914.233613 BRANCH=flashlink COMMIT="$revision"
SOURCE_DATE_EPOCH=$(git -C "$src" show -s --format=%ct "$revision")
export SOURCE_DATE_EPOCH
cmake -S "$src" -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
    -DFFMPEG_PREPARED_BINARIES="$(cygpath -m "$work/ffmpeg")" \
    -DBUILD_DOCS=OFF -DBUILD_TESTS=OFF -DSUNSHINE_ENABLE_TRAY=OFF -DSUNSHINE_USE_STATIC_QT=OFF \
    -DDOTNET_EXECUTABLE:FILEPATH=OFF -DOPENSSL_USE_STATIC_LIBS=TRUE \
    -DSUNSHINE_ASSETS_DIR=assets -DSUNSHINE_PUBLISHER_NAME=FlashLink \
    -DSUNSHINE_PUBLISHER_ISSUE_URL=https://github.com/RicardoMin/FlashLink/issues
cmake --build "$build" --target sunshine --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-4}"
strip "$build/sunshine.exe"
mkdir -p "$work/engine"
cp "$build/sunshine.exe" "$work/engine/sunshine.exe"
cp "$src/LICENSE" "$work/engine/Sunshine-LICENSE"
exe_hash=$(sha256sum "$work/engine/sunshine.exe" | cut -d' ' -f1)
printf '%s\n' "$revision" "$patch_hash" "$exe_hash" > "$work/engine/flashlink-engine.txt"
