#!/bin/sh
# Builds plex-rust.app in target/: the release binary, its icon and an
# Info.plist, signed ad hoc so that macOS will run it.
#
# The binary links libmpv statically (scripts/build-libmpv.sh), so the app
# needs nothing but macOS itself.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
app=$root/target/plex-rust.app
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/app/Cargo.toml" | head -1)

cargo build --release --manifest-path "$root/Cargo.toml"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/plex-rust" "$app/Contents/MacOS/plex-rust"

# An .icns holds the icon at every size macOS asks for.
iconset=$(mktemp -d)/plex-rust.iconset
mkdir "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns "$iconset" --output "$app/Contents/Resources/plex-rust.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>plex-rust</string>
    <key>CFBundleDisplayName</key><string>plex-rust</string>
    <key>CFBundleIdentifier</key><string>dev.plex-rust.app</string>
    <key>CFBundleExecutable</key><string>plex-rust</string>
    <key>CFBundleIconFile</key><string>plex-rust</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>plex-rust connects to the Plex Media Server on your network.</string>
</dict>
</plist>
PLIST

codesign --force --sign - "$app"
echo "$app"
