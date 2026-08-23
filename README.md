# displayctl

`displayctl` is a small macOS command-line tool for running a MacBook with only a physical external display. It soft-disconnects the built-in panel and stays running as a dock supervisor:

- When every physical external display disappears, it restores the built-in display.
- When an external display reconnects and settles, it turns the built-in display off again.
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

## Limitations

- Only a single built-in display is supported.
- An active physical external display is required before external-only mode can start.
- The behavior relies on private CoreGraphics/SkyLight symbols.

## License

No license has been granted yet. All rights are reserved by the copyright holder.
