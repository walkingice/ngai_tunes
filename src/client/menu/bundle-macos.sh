#!/bin/sh
set -eu

cd "$(dirname "$0")/../../.."
cargo bundle --release --bin ngai-tunes --format osx
target_dir=$(cargo metadata --no-deps --format-version 1 |
    /usr/bin/plutil -extract target_directory raw -o - -)
contents="$target_dir/release/bundle/osx/Ngai Tunes.app/Contents"
plist="$contents/Info.plist"

/usr/bin/plutil -replace LSUIElement -bool YES "$plist"
# Launch the Mach-O directly so AppKit can register its status item.
/usr/bin/plutil -replace LSEnvironment -json '{"NGAI_TUNES_BUNDLE_MENU":"1"}' "$plist"
/usr/bin/codesign --force --sign - "${contents%/Contents}"
printf '%s\n' "${contents%/Contents}"
