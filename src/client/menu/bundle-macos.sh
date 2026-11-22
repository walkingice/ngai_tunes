#!/bin/sh
set -eu

cd "$(dirname "$0")/../../.."
cargo bundle --release --bin ngai-tunes --format osx
target_dir=$(cargo metadata --no-deps --format-version 1 |
    /usr/bin/plutil -extract target_directory raw -o - -)
contents="$target_dir/release/bundle/osx/Ngai Tunes.app/Contents"
plist="$contents/Info.plist"
launcher="$contents/MacOS/ngai-tunes-menu"

cat > "$launcher" <<'LAUNCHER'
#!/bin/sh
exec "$(dirname "$0")/ngai-tunes" --menu
LAUNCHER
chmod +x "$launcher"
/usr/bin/plutil -replace CFBundleExecutable -string ngai-tunes-menu "$plist"
/usr/bin/plutil -replace LSUIElement -bool YES "$plist"
/usr/bin/codesign --force --sign - "${contents%/Contents}"
printf '%s\n' "${contents%/Contents}"
