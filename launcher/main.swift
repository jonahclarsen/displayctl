// Session-only launcher for the dev-mode menu bar app. Being a real app gives the
// LaunchAgent a readable name and icon in System Settings, and lets macOS grant it
// access to ~/Documents (a bare launchd shell script is denied). It stays alive as
// the parent of scripts/dev.sh so that permission covers the whole dev toolchain.
import Foundation

guard let repo = Bundle.main.object(forInfoDictionaryKey: "DisplayctlRepo") as? String else {
    fputs("displayctl launcher: DisplayctlRepo missing from Info.plist\n", stderr)
    exit(EXIT_FAILURE)
}

let dev = Process()
dev.executableURL = URL(fileURLWithPath: "/bin/zsh")
dev.arguments = ["\(repo)/scripts/dev.sh"]

var sources: [DispatchSourceSignal] = []
for number in [SIGTERM, SIGINT, SIGHUP] {
    signal(number, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: number)
    // Process makes the child a process group leader, out of reach of launchd's own
    // cleanup, so signal the whole group (pnpm, vite, cargo and the app).
    source.setEventHandler { if dev.isRunning { kill(-dev.processIdentifier, number) } }
    source.resume()
    sources.append(source)
}

dev.terminationHandler = { exit($0.terminationStatus) }
do {
    try dev.run()
} catch {
    fputs("displayctl launcher: \(error)\n", stderr)
    exit(EXIT_FAILURE)
}
dispatchMain()
