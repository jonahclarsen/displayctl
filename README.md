# displayctl

`displayctl` is a small macOS command-line tool for running a MacBook with only a physical external display. It soft-disconnects the built-in panel and stays running as a dock supervisor:

- When every physical external display disappears, it restores the built-in display.
- When an external display reconnects and settles, it turns the built-in display off again.
- When a connected external display sleeps, it leaves the display configuration alone and waits for the monitor to wake and settle.
- If macOS has already turned the built-in display off because the lid is closed, it simply starts the supervisor.

Virtual displays and AirPlay displays do not count as physical external displays.

## Requirements

- macOS
- Swift toolchain (included with Xcode or the Xcode Command Line Tools)
- A Mac with one built-in display and at least one active physical external display

The tool uses an undocumented macOS display API and may stop working after a macOS update.

## Quick install

Open Terminal, paste this entire line, and press Return:

```sh
git clone https://github.com/jonahclarsen/displayctl.git && cd displayctl && swiftc main.swift -o displayctl && sudo mkdir -p /usr/local/bin && sudo install -m 755 displayctl /usr/local/bin/displayctl
```

Enter your Mac password when prompted. Terminal will not show characters while you type the password; this is normal. If the command reports that `swiftc` is missing, run `xcode-select --install`, finish the installation, and then try the command again.

## Build and install

```sh
swiftc main.swift -o displayctl
install -m 755 displayctl /usr/local/bin/displayctl
```

If `/usr/local/bin` is not writable, use `sudo` for the `install` command or install the binary in another directory on your `PATH`.

## Usage

```text
displayctl list
displayctl off [--restore-after SECONDS]
displayctl on
```

`displayctl off` remains in the foreground. Leave it running while using the external-only setup. Press Control-C, or run `displayctl on` from another terminal, to restore the built-in display and stop the supervisor.

If a display change fails, the supervisor keeps retrying but prints each distinct warning only once until the requested display state is reached. Installing an update does not replace an already-running supervisor; restart `displayctl off` to use the new version.

Examples:

```sh
# Show detected displays and watchdog state
displayctl list

# Disable the built-in panel and supervise display changes
displayctl off

# Try external-only mode for 30 seconds, then restore the panel
displayctl off --restore-after 30

# Restore the panel and stop a running supervisor
displayctl on
```

Recovery state is stored at `~/.local/state/displayctl/recovery.json`. If a previous supervisor was interrupted, the next `off` invocation uses this record to recover safely before continuing.

## Menu bar app

A Tauri/Svelte menu bar app wraps `displayctl off` / `displayctl on`. Its icon is a
laptop seen from the side:

- lid open, glowing: built-in display on
- lid half shut, pulsing in the popover: supervisor running, waiting for an external monitor
- lid shut: built-in display off

The number beside it is the main display's current refresh rate in hertz.

Click the icon for a one-button popover, press Control-Shift-D anywhere, or run
`pkill -USR1 displayctl-menubar` to toggle. Right-click for Quit. The supervisor runs
in its own process group, so it keeps going across app restarts, and the app resumes
it when manually relaunched if the display was off.

**Emergency off: Control–Option–Shift–D or Option–Shift–D**, anywhere while the
menu bar app is running. Both shortcuts always restore; neither toggles.
This stops the supervisor, wakes and enables the built-in display, and verifies
that it is drawable or mirrors an active source. It repairs an orphaned mirror
relationship during recovery. If targeted recovery fails with no active external
monitor, it falls back to restoring the saved macOS display configuration.
Pressing it again never turns the display off. If recovery fails, its record is
kept so another press can retry. The last recovery target is retained even after
success, so an emergency retry can still find a panel missing from enumeration.
Restore verification uses fresh processes and requires a stable, awake built-in
panel; cached online/active flags alone do not count. Enabling is committed before
attempting mirror repair, so a mirror error cannot cancel the enable.
Display commands have time limits so a stuck
command cannot block later recovery requests indefinitely. It is also in the
right-click menu. For terminal recovery, run `displayctl on` or `pkill -USR2 displayctl-menubar`.
Automation stays off until you explicitly toggle it back on.

The menu bar app also runs an independent unplug watchdog. It reads fresh display
state every second, without relying on the supervisor's event queue. If there is
no usable screen for two seconds, it restores the built-in panel and replaces the
supervisor so reconnect detection continues. A genuinely sleeping monitor is left
alone; an inactive but awake connector does not count as a usable monitor.
Failed safety probes are treated as unknown, not as a confirmed unplug. Isolated
misses are retried without changing either display; a probe may take up to five
seconds. Three consecutive failures spanning at least thirty seconds (a CPU spike
can slow probes for over a minute) request recovery and suspend automatic switching
until explicitly enabled again;
otherwise a slow probe could repeatedly restore and re-disable the laptop panel.
The suspension survives an app relaunch. Confirmed unplug/reconnect detection
remains automatic. Failed recovery is retried at ten-second intervals.

It runs in dev mode (debug build, verbose logs) only when manually started.
It does **not** launch at login or after a reboot, so restarting the Mac is a
recovery path. Display changes apply only to the current macOS session.

Start it after login with:

```sh
pnpm install
scripts/install-launch-agent.sh   # start for this session; removes old login registration
pnpm relaunch                     # rebuild and restart after changes
scripts/uninstall-launch-agent.sh
```

The session-only launchd job starts `~/Applications/displayctl.app`, a small launcher
that holds the one-time Documents folder permission for the dev toolchain. Its job
file lives in `~/Library/Application Support/displayctl/`, outside `LaunchAgents`,
so macOS does not load it at login. Logs are in `~/Library/Logs/displayctl/`:
`app.log` (the app), `supervisor.log` (`displayctl off` output), and `dev.log` (build output).

## Daily morning brightness

On Apple Silicon Macs, install [m1ddc](https://github.com/waydabber/m1ddc) and enable the background rule:

```sh
brew install m1ddc
swiftc main.swift -o displayctl
install -m 755 displayctl "$HOME/.local/bin/displayctl"
displayctl brightness-install
```

Every day at or after 6:30 a.m. local time, each awake physical external monitor
gets a maximum-brightness command after five seconds of continuous availability.
A monitor already connected at 6:30 a.m. is adjusted around 6:30:05; one first
connected at 9 a.m. is adjusted around 9:00:05. Detection polls every half second.
Unplugging or sleeping during the delay starts a fresh delay on reconnect/wake.
The Mac must be awake and logged in; a missed morning runs after wake/login.

Successful commands are remembered per monitor and local calendar day in
`~/.local/state/displayctl/brightness.json`, including across restarts. Lowering
brightness or reconnecting afterward does not trigger another adjustment that day.
Enabling the rule after 6:30 a.m. also triggers today's adjustment.

This runs independently of `displayctl off`, starts at login, and leaves the
built-in panel alone. The LaunchAgent is
`~/Library/LaunchAgents/com.jonahclarsen.displayctl-brightness.plist`.
Logs are in `~/.local/state/displayctl/brightness{,-error}.log`.
Run `displayctl brightness-install` again after updating to restart the rule.
For foreground troubleshooting, use `displayctl brightness-watch` after stopping
the LaunchAgent; a lock prevents duplicate watchers.

The monitor must support DDC brightness writes. The rule queries its maximum,
falling back to 100 if the monitor returns no usable value. A successful command
does not guarantee a physical change on monitors that cannot report brightness.
MonitorControl can still be used manually, but its slider may retain its previous
value after another tool changes hardware brightness. Software dimming is not
changed by this rule.

For the ASUS XG32UCWMG setup, install the tested upstream helper with
`bash scripts/install-brightness-helper.sh`, then run `displayctl brightness-install`.
This pins upstream revision `04d949794102eb8df01ad3681afff6464a3eede2` in
`~/.local/libexec/displayctl/m1ddc`, which takes precedence over Homebrew's version.
The successful manual test used this helper's unqualified `set luminance 100`
command. The rule uses that default connection when there is exactly one physical
external display and it is the main display; otherwise it selects by UUID.
On this ASUS setup, UUID-selected writes returned success without a visible
change and brightness reads returned zero. Verify visible behavior when changing
the connection or helper; exit status alone is insufficient. The default connection
and newer helper were tested together, so their individual effects are unconfirmed.

To stop the rule (including future logins):

```sh
launchctl bootout "gui/$(id -u)/com.jonahclarsen.displayctl-brightness"
mv "$HOME/Library/LaunchAgents/com.jonahclarsen.displayctl-brightness.plist" "$HOME/.local/state/displayctl/brightness-agent.disabled.plist"
```

## Limitations

- Only a single built-in display is supported.
- Without an active physical external display, the supervisor waits with the built-in panel on.
- The behavior relies on private CoreGraphics/SkyLight symbols.

## License

No license has been granted yet. All rights are reserved by the copyright holder.
