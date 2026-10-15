#!/bin/zsh
set -eu
label=com.jonahclarsen.displayctl-dev
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
rm -rf "$HOME/Applications/displayctl.app"
rm -f "$HOME/Library/LaunchAgents/$label.plist"
rm -f "$HOME/Library/Application Support/displayctl/$label.plist"
