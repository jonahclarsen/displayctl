#!/bin/zsh
# Starts the menu bar app in dev mode for this login session only.
set -eu
repo="${0:A:h:h}"
label=com.jonahclarsen.displayctl-dev
bundle_id=com.jonahclarsen.displayctl.launcher
# Deliberately outside LaunchAgents: reboot/logout must stop display control.
plist="$HOME/Library/Application Support/displayctl/$label.plist"
legacy_plist="$HOME/Library/LaunchAgents/$label.plist"
logs="$HOME/Library/Logs/displayctl"
app="$HOME/Applications/displayctl.app"
mkdir -p "$logs" "${plist:h}"

# macOS asks once for Documents access on first launch (the repo lives there).
if [[ ! -x "$app/Contents/MacOS/displayctl" || "$repo/launcher/main.swift" -nt "$app/Contents/MacOS/displayctl" \
      || "$(/usr/bin/plutil -extract DisplayctlRepo raw -o - "$app/Contents/Info.plist" 2>/dev/null)" != "$repo" ]]; then
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  swiftc -O "$repo/launcher/main.swift" -o "$app/Contents/MacOS/displayctl"
  iconset=$(mktemp -d)/AppIcon.iconset
  mkdir -p "$iconset"
  for size in 16 32 128 256 512; do
    sips -z $size $size "$repo/src-tauri/icons/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$repo/src-tauri/icons/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
  cat > "$app/Contents/Info.plist" <<INFO
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>displayctl</string>
  <key>CFBundleDisplayName</key><string>displayctl</string>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleExecutable</key><string>displayctl</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>LSUIElement</key><true/>
  <key>DisplayctlRepo</key><string>$repo</string>
</dict>
</plist>
INFO
  # A real certificate keeps the Documents permission across rebuilds; ad-hoc loses it.
  identity="${DISPLAYCTL_SIGNING_IDENTITY:-$(security find-identity -v -p codesigning | sed -nE 's/^ *[0-9]+\) ([A-F0-9]{40}) "Developer ID Application:.*/\1/p' | head -1)}"
  codesign --force --sign "${identity:--}" --identifier "$bundle_id" "$app"
fi
agent_path="${$(command -v node):h}:${$(command -v cargo):h}:${$(command -v pnpm):h}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>$app/Contents/MacOS/displayctl</string>
  </array>
  <key>AssociatedBundleIdentifiers</key>
  <array>
    <string>$bundle_id</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>$agent_path</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>StandardOutPath</key>
  <string>$logs/dev.log</string>
  <key>StandardErrorPath</key>
  <string>$logs/dev.log</string>
</dict>
</plist>
PLIST

launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
rm -f "$legacy_plist"
# bootout returns before the old process tree has necessarily finished exiting.
# launchd rejects an immediate replacement with EIO during that short window.
started=false
for attempt in {1..20}; do
  if launchctl bootstrap "gui/$(id -u)" "$plist" 2>/dev/null; then
    started=true
    break
  fi
  sleep 0.5
done
if [[ "$started" != true ]]; then
  echo "Could not start session-only job: $plist" >&2
  exit 1
fi
echo "Started for this session only (no launch at login); logs in $logs"
