#!/bin/zsh
# Builds the displayctl CLI next to main.swift (used by the menu bar app) and installs
# it in ~/.local/bin (used by the brightness LaunchAgent and the terminal).
# Writes to a temporary file first: overwriting a running binary in place gets the
# running supervisor killed by code signing, leaving the built-in display off.
set -eu
cd "${0:A:h:h}"
if [[ ! -x displayctl || main.swift -nt displayctl ]]; then
  swiftc main.swift -o displayctl.new
  mv -f displayctl.new displayctl
fi
install -m 755 displayctl "$HOME/.local/bin/displayctl"
