#!/usr/bin/env bash
# Builds target/release/bundle/gridterm.app (ad-hoc signed).
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -f "$HOME/.cargo/env" ]]; then source "$HOME/.cargo/env"; fi
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
cargo build --release -p gridterm
APP=target/release/bundle/gridterm.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/gridterm "$APP/Contents/MacOS/gridterm"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>gridterm</string>
  <key>CFBundleDisplayName</key><string>gridterm</string>
  <key>CFBundleIdentifier</key><string>dev.gridterm.app</string>
  <key>CFBundleExecutable</key><string>gridterm</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <!-- Finder: Open With → gridterm for scripts, Unix executables and folders
       (Alternate: Terminal stays the default until the user picks gridterm
       with Get Info → Open with → Change All). -->
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Shell script</string>
      <key>CFBundleTypeRole</key><string>Shell</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>com.apple.terminal.shell-script</string>
        <string>public.shell-script</string>
        <string>public.unix-executable</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Folder</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array><string>public.folder</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "warning: ad-hoc codesign failed"
echo "$APP"
