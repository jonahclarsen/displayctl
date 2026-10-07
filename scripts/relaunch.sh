#!/bin/zsh
# Rebuilds and relaunches the dev-mode menu bar app through its LaunchAgent.
set -eu
label=com.jonahclarsen.displayctl-dev
if ! launchctl print "gui/$(id -u)/$label" >/dev/null 2>&1; then
  exec "${0:A:h}/install-launch-agent.sh"
fi
launchctl kickstart -k "gui/$(id -u)/$label"
echo "Relaunched; follow with: tail -f ~/Library/Logs/displayctl/dev.log"
