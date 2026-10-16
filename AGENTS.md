# Repository Instructions

- After every change, rebuild and relaunch the menu bar app in dev mode:

  ```sh
  pnpm relaunch
  ```

  This restarts the session-only `com.jonahclarsen.displayctl-dev` LaunchAgent, which rebuilds the
  `displayctl` CLI (also installing it to `~/.local/bin`) and runs `pnpm tauri dev`.
  Confirm it came back up in `~/Library/Logs/displayctl/dev.log` and `app.log`.
  After changing `launcher/` or `scripts/install-launch-agent.sh`, run
  `scripts/install-launch-agent.sh` instead.
- Never install the menu bar job in `~/Library/LaunchAgents` or enable launch at
  login: reboot must remain a recovery path. Keep the emergency restore shortcut
  (Control-Option-Shift-D, with Option-Shift-D as an alias) one-way and independent
  of the toggle.
- Recovery must verify the built-in display is awake and drawable, or mirrors an
  awake, active source; online alone can mean a black panel after unplug. Keep
  recovery state until this check succeeds. A simulated lid-open test with the
  external monitor connected does not validate physical-unplug recovery.
- Keep the menu bar unplug watchdog independent of the Swift supervisor queue.
  Validate stalled-supervisor recovery as well as the supervisor’s own decisions.
- After every change, commit it and push the commit to the current remote branch.
- The repository is public: never commit signing identities, personal paths, or logs.
