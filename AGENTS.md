# Repository Instructions

- After every change, rebuild and relaunch the menu bar app in dev mode:

  ```sh
  pnpm relaunch
  ```

  This restarts the `com.jonahclarsen.displayctl-dev` LaunchAgent, which rebuilds the
  `displayctl` CLI (also installing it to `~/.local/bin`) and runs `pnpm tauri dev`.
  Confirm it came back up in `~/Library/Logs/displayctl/dev.log` and `app.log`.
  After changing `launcher/` or `scripts/install-launch-agent.sh`, run
  `scripts/install-launch-agent.sh` instead.
- After every change, commit it and push the commit to the current remote branch.
- The repository is public: never commit signing identities, personal paths, or logs.
