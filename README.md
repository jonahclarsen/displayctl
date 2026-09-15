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

## Daily morning brightness

On Apple Silicon Macs, install [m1ddc](https://github.com/waydabber/m1ddc) and enable the background rule:

```sh
brew install m1ddc
swiftc main.swift -o displayctl
install -m 755 displayctl "$HOME/.local/bin/displayctl"
displayctl brightness-install
```

Every day at or after 8 a.m. local time, each awake physical external monitor
gets a maximum-brightness command after five seconds of continuous availability.
A monitor already connected at 8 a.m. is adjusted around 8:00:05; one first
connected at 9 a.m. is adjusted around 9:00:05. Detection polls every half second.
Unplugging or sleeping during the delay starts a fresh delay on reconnect/wake.
The Mac must be awake and logged in; a missed morning runs after wake/login.

Successful commands are remembered per monitor and local calendar day in
`~/.local/state/displayctl/brightness.json`, including across restarts. Lowering
brightness or reconnecting afterward does not trigger another adjustment that day.
Enabling the rule after 8 a.m. also triggers today's adjustment.

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

To stop the rule (including future logins):

```sh
launchctl bootout "gui/$(id -u)/com.jonahclarsen.displayctl-brightness"
mv "$HOME/Library/LaunchAgents/com.jonahclarsen.displayctl-brightness.plist" "$HOME/.local/state/displayctl/brightness-agent.disabled.plist"
```

## Limitations

- Only a single built-in display is supported.
- An active physical external display is required before external-only mode can start.
- The behavior relies on private CoreGraphics/SkyLight symbols.

## License

No license has been granted yet. All rights are reserved by the copyright holder.
