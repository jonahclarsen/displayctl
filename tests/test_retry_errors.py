"""Exercise the production warning path without changing any display settings.

Run with: python3 tests/test_retry_errors.py
"""

from pathlib import Path
import subprocess
import tempfile
import unittest


class RetryErrorTests(unittest.TestCase):
    def test_probe_timeout_restores_then_exits_instead_of_rearming(self):
        result = self.run_swift(r'''
if CommandLine.arguments.dropFirst().first == "probe-display-state" {
    Thread.sleep(forTimeInterval: 30)
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "set-built-in" {
    precondition(CommandLine.arguments.last == "on", "Timeout must never disable a display")
    exit(0) // Fake the restore child; no display API is called by this harness.
}
extension DisplayController {
    func exerciseTimedOutProbe() throws {
        hotPlugged = { false }
        try saveRecovery(displayID: 123, watchdogPID: getpid())
        for _ in 0..<8 {
            reconcileDisplayState(displayID: 123)
            Thread.sleep(forTimeInterval: 1)
        }
        preconditionFailure("Sustained unavailable topology must terminate automatic switching")
    }
}
let record = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
    .appendingPathComponent("recovery.json")
try DisplayController(recoveryURL: record).exerciseTimedOutProbe()
''', expected_status=75)
        self.assertIn("automatic switching suspended", result.stdout)

    def test_probe_timeouts_never_restore_while_monitor_is_hot_plugged(self):
        result = self.run_swift(r'''
if CommandLine.arguments.dropFirst().first == "probe-display-state" { exit(1) }
if CommandLine.arguments.dropFirst().first == "set-built-in" {
    preconditionFailure("A slow probe with the monitor attached must not reconfigure anything")
}
extension DisplayController {
    func exerciseCPUSpike() throws {
        hotPlugged = { true }
        try saveRecovery(displayID: 123, watchdogPID: getpid(), automationSuspended: false)
        for second in 0..<40 {
            reconcileDisplayState(displayID: 123)
            if second < 39 { Thread.sleep(forTimeInterval: 1) }
        }
        let record = try loadRecoveryTarget()
        precondition(record?.automationSuspended == false)
        print("still supervising")
    }
}
let record = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
    .appendingPathComponent("recovery.json")
try DisplayController(recoveryURL: record).exerciseCPUSpike()
''')
        self.assertIn("still supervising", result.stdout)

    def test_isolated_unknown_probe_does_not_suspend_or_change_displays(self):
        self.run_swift(r'''
private var guardState = UnavailableProbeGuard()
precondition(!guardState.shouldSuspend(.unknown, uptime: 0))
precondition(!guardState.shouldSuspend(.ready, uptime: 1))
precondition(!guardState.shouldSuspend(.unknown, uptime: 10))
precondition(!guardState.shouldSuspend(.unknown, uptime: 20))
// A CPU spike can delay probes for a minute: 3 misses over 6 s is not enough.
precondition(!guardState.shouldSuspend(.unknown, uptime: 30))
precondition(!guardState.shouldSuspend(.unknown, uptime: 39))
precondition(guardState.shouldSuspend(.unknown, uptime: 40))
precondition(!guardState.shouldSuspend(.inactive, uptime: 41))
precondition(!guardState.shouldSuspend(.unknown, uptime: 50))
precondition(!guardState.shouldSuspend(.absent, uptime: 51))

if CommandLine.arguments.dropFirst().first == "probe-display-state" { exit(1) }
if CommandLine.arguments.dropFirst().first == "set-built-in" {
    preconditionFailure("An isolated failed probe must not reconfigure anything")
}
extension DisplayController {
    func exerciseIsolatedMiss() throws {
        try saveRecovery(displayID: 123, watchdogPID: getpid(), automationSuspended: false)
        reconcileDisplayState(displayID: 123)
        let record = try loadRecoveryTarget()
        precondition(record?.automationSuspended == false)
        precondition(record?.restored != true)
    }
}
let record = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
    .appendingPathComponent("recovery.json")
try DisplayController(recoveryURL: record).exerciseIsolatedMiss()
''')

    def test_suspension_survives_restore_and_only_explicit_start_clears_it(self):
        self.run_swift(r'''
extension DisplayController {
    func exerciseSuspension() throws {
        try saveRecovery(displayID: 123, watchdogPID: 456, automationSuspended: true)
        try saveRecovery(displayID: 123, watchdogPID: nil) // Emergency restore retry.
        try finishRecovery()
        let suspended = try loadRecoveryTarget()
        precondition(suspended?.automationSuspended == true)
        precondition(suspended?.restored == true)
        precondition(suspended?.builtInDisplayID == 123)
        try saveRecovery(displayID: 123, watchdogPID: 456, automationSuspended: false)
        let resumed = try loadRecoveryTarget()
        precondition(resumed?.automationSuspended == false)
        precondition(resumed?.restored != true)
    }
}
let record = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
    .appendingPathComponent("recovery.json")
try DisplayController(recoveryURL: record).exerciseSuspension()
''')

    def test_restore_verification_rejects_false_success_from_online_active_flags(self):
        self.run_swift(r'''
if CommandLine.arguments.dropFirst().first == "probe-display-state" {
    FileHandle.standardOutput.write(try JSONEncoder().encode(
        DisplaySnapshot(externalState: .absent, builtInOnline: true,
                        builtInActive: true, builtInUsable: false)
    ))
    exit(0)
}
extension DisplayController {
    func exerciseFalseSuccess() throws {
        try saveRecovery(displayID: 123, watchdogPID: nil)
        do {
            try verifyRestored(displayID: 123)
            try finishRecovery()
            preconditionFailure("Online/active alone must not finish recovery")
        } catch {
            let pending = try loadRecovery()
            precondition(pending?.builtInDisplayID == 123)
        }
    }
}
let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
defer { try? FileManager.default.removeItem(at: directory) }
try DisplayController(recoveryURL: directory.appendingPathComponent("recovery.json")).exerciseFalseSuccess()
''')

    def test_finished_recovery_retains_emergency_target_without_live_supervisor(self):
        self.run_swift(r'''
extension DisplayController {
    func exerciseRecoveryLifetime() throws {
        try saveRecovery(displayID: 123, watchdogPID: 456)
        let running = try loadRecovery()
        precondition(running?.watchdogPID == 456)
        try finishRecovery()
        let completed = try loadRecovery()
        precondition(completed == nil, "Completed recovery must not resume automation")
        let target = try loadRecoveryTarget()
        precondition(target?.builtInDisplayID == 123, "A later emergency still needs the target")
        precondition(target?.watchdogPID == nil)
        try saveRecovery(displayID: 123, watchdogPID: nil)
        let pending = try loadRecovery()
        precondition(pending?.builtInDisplayID == 123, "Retries must re-arm the safety guard")
    }
}
let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
defer { try? FileManager.default.removeItem(at: directory) }
try DisplayController(recoveryURL: directory.appendingPathComponent("recovery.json")).exerciseRecoveryLifetime()
''')

    def test_restore_verification_waits_for_stable_fresh_snapshots(self):
        self.run_swift(r'''
if CommandLine.arguments.dropFirst().first == "probe-display-state" {
    FileHandle.standardOutput.write(try JSONEncoder().encode(
        DisplaySnapshot(externalState: .absent, builtInOnline: true,
                        builtInActive: true, builtInUsable: true)
    ))
    exit(0)
}
extension DisplayController {
    func exerciseStableRestoration() throws {
        let start = ProcessInfo.processInfo.systemUptime
        try verifyRestored(displayID: 123)
        precondition(ProcessInfo.processInfo.systemUptime - start >= 0.5)
    }
}
try DisplayController().exerciseStableRestoration()
''')

    def test_snapshot_reads_built_in_state_from_fresh_process(self):
        self.run_swift(r'''
if CommandLine.arguments.dropFirst().first == "probe-display-state" {
    precondition(CommandLine.arguments.last == "123")
    // Simulate the panel reopening while the parent still has an old connection.
    FileHandle.standardOutput.write(try JSONEncoder().encode(
        DisplaySnapshot(externalState: .ready, builtInOnline: true, builtInActive: false, builtInUsable: false)
    ))
    exit(0)
}
extension DisplayController {
    func exerciseFreshSnapshot() {
        let snapshot = freshProcessDisplaySnapshot(displayID: 123)
        precondition(snapshot.externalState == .ready)
        precondition(snapshot.builtInOnline, "Use the child's built-in state")
        precondition(!snapshot.builtInActive, "An online panel may still be unusable")
        precondition(!snapshot.builtInUsable)
    }
}
DisplayController().exerciseFreshSnapshot()
''')

    def test_unplug_restores_online_but_inactive_panel(self):
        self.run_swift(r'''
extension DisplayController {
    func exerciseInactivePanel() {
        var requests: [Bool] = []
        reconcileDisplayState(externalState: .ready, builtInOnline: false,
                              builtInUsable: false) { requests.append($0) }
        reconcileDisplayState(externalState: .absent, builtInOnline: true,
                              builtInUsable: false) { requests.append($0) }
        precondition(requests == [true], "Online-but-inactive must trigger unplug recovery")
        reconcileDisplayState(externalState: .absent, builtInOnline: true,
                              builtInUsable: false) { requests.append($0) }
        precondition(requests == [true, true], "Retry until the panel is drawable")
        reconcileDisplayState(externalState: .absent, builtInOnline: true,
                              builtInUsable: true) { requests.append($0) }
        precondition(requests == [true, true], "Stop restoring once active")
        precondition(!displayStateMatches(online: true, observedOnline: true, usable: false))
        precondition(displayStateMatches(online: true, observedOnline: true, usable: true))
    }
}
DisplayController().exerciseInactivePanel()
''')

    def test_inactive_awake_connector_is_not_treated_as_monitor_sleep(self):
        self.run_swift(r'''
precondition(externalDisplayState([]) == .absent)
precondition(externalDisplayState([(active: true, asleep: false)]) == .ready)
precondition(externalDisplayState([(active: false, asleep: true)]) == .inactive)
precondition(externalDisplayState([(active: false, asleep: false)]) == .absent,
             "An unusable connector must allow laptop recovery")
precondition(externalDisplayState([(active: true, asleep: true)]) == .inactive)
precondition(externalDisplayState([(active: false, asleep: true), (active: true, asleep: false)]) == .ready)
''')

    def test_mirror_requires_an_awake_active_source(self):
        self.run_swift(r'''
precondition(usableDisplay(online: true, active: true, asleep: false, mirrorSourceActive: false))
precondition(usableDisplay(online: true, active: false, asleep: false, mirrorSourceActive: true),
             "A working hardware mirror can be inactive")
precondition(!usableDisplay(online: true, active: false, asleep: false, mirrorSourceActive: false),
             "An orphaned mirror is not restored")
precondition(!usableDisplay(online: true, active: true, asleep: true, mirrorSourceActive: true),
             "A sleeping panel is not verified restoration")
precondition(!usableDisplay(online: false, active: false, asleep: false, mirrorSourceActive: true))
''')

    def test_stuck_display_helper_is_killed_with_timeout(self):
        self.run_swift(r'''
let child = Process()
child.executableURL = URL(fileURLWithPath: "/bin/sleep")
child.arguments = ["30"]
try child.run()
let started = ProcessInfo.processInfo.systemUptime
do {
    try waitForDisplayProcess(child, timeout: 0.1)
    preconditionFailure("A stuck display helper must time out")
} catch {
    precondition(String(describing: error).contains("timed out"))
}
precondition(!child.isRunning)
precondition(ProcessInfo.processInfo.systemUptime - started < 2)
''')

    def test_lid_opens_with_external_already_connected(self):
        self.run_swift(r'''
extension DisplayController {
    func exerciseLidOpening() {
        var requests: [Bool] = []
        func step(online: Bool, time: Double) {
            reconcileDisplayState(
                externalState: .ready, builtInOnline: online, builtInUsable: online,
                now: Date(timeIntervalSinceReferenceDate: time)
            ) { requests.append($0) }
        }
        step(online: false, time: 0) // Plug monitor in while lid is shut.
        step(online: false, time: 2)
        precondition(requests.isEmpty)
        step(online: true, time: 3) // Open lid; external state hasn't changed.
        precondition(requests == [false], "Opening lid must disable panel automatically")
        step(online: false, time: 4)
        precondition(requests == [false], "Do not repeatedly configure an offline panel")
    }
}
DisplayController().exerciseLidOpening()
''')

    def test_daily_brightness_schedule(self):
        self.run_swift(r'''
private let single = BrightnessDisplay(id: 2, uuid: "external", useDefaultConnection: true)
precondition(single.ddcArguments + ["set", "luminance", "100"] == ["set", "luminance", "100"])
private let multiple = BrightnessDisplay(id: 3, uuid: "other", useDefaultConnection: false)
precondition(multiple.ddcArguments == ["display", "other"])
var calendar = Calendar(identifier: .gregorian)
calendar.timeZone = TimeZone(identifier: "America/Vancouver")!
let morning = calendar.date(from: DateComponents(year: 2026, month: 9, day: 14, hour: 6, minute: 30))!
private var rule = DailyBrightnessRule()
func tick(_ seconds: Double, _ displays: [String] = ["A"]) -> [String] {
    rule.due(displays: displays, now: morning.addingTimeInterval(seconds),
             uptime: seconds + 100, calendar: calendar)
}
for second in -10..<0 { precondition(tick(Double(second)).isEmpty, "Do not trigger during 06:29") }
for second in 0..<5 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(5) == ["A"], "Connected at 06:30 must wait five seconds")
rule.succeeded("A", now: morning.addingTimeInterval(5), calendar: calendar)
precondition(tick(6).isEmpty)
precondition(tick(7, []).isEmpty)
for second in 8...14 { precondition(tick(Double(second)).isEmpty) }

// Persistence uses stable monitor UUIDs, not transient display IDs.
let encoded = try JSONEncoder().encode(rule.completed)
rule = DailyBrightnessRule()
rule.completed = try JSONDecoder().decode([String: String].self, from: encoded)
precondition(tick(15).isEmpty, "Restart must not repeat a completed day")

// A second monitor arriving at 7:30 a.m. has its own delay.
for second in 3600..<3605 { precondition(tick(Double(second), ["A", "B"]).isEmpty) }
precondition(tick(3605, ["A", "B"]) == ["B"])

// Disconnect cancels a pending adjustment; reconnect starts from zero.
rule = DailyBrightnessRule()
for second in 3600...3603 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(3604, []).isEmpty)
for second in 3605..<3610 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(3610) == ["A"])
rule.retry("A", uptime: 3710)
for second in 3611..<3640 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(3640) == ["A"], "Failed writes retry after 30 seconds")

// Sleep or missed polling must not count toward the delay.
rule = DailyBrightnessRule()
precondition(tick(3600).isEmpty)
precondition(tick(3601).isEmpty)
for second in 7200..<7205 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(7205) == ["A"])
rule.succeeded("A", now: morning.addingTimeInterval(7205), calendar: calendar)

// Next day resets automatically, but nothing happens before 06:30.
precondition(tick(86400 - 1).isEmpty)
for second in 86400..<86405 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(86405) == ["A"])

// Clock changes cannot cause an early firing.
rule = DailyBrightnessRule()
precondition(tick(3600).isEmpty)
precondition(tick(3601).isEmpty)
precondition(tick(-10).isEmpty)
for second in 0..<5 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(5) == ["A"])
''')

    def run_swift(self, harness, expected_status=0):
        source = (Path(__file__).resolve().parents[1] / "main.swift").read_text()
        definitions, marker, _ = source.partition("// MARK: Command-line entry point")
        self.assertTrue(marker, "Could not locate the CLI entry point")
        # Compile in the same file so the harness can exercise private logging
        # without exposing a test command or invoking display configuration APIs.
        with tempfile.TemporaryDirectory(prefix="displayctl-tests-") as directory:
            main = Path(directory) / "main.swift"
            binary = Path(directory) / "test-displayctl"
            main.write_text(definitions + harness)
            subprocess.run(["swiftc", str(main), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=60)
            self.assertEqual(result.returncode, expected_status, result.stdout + result.stderr)
            if expected_status == 75:
                import json
                record = json.loads((Path(directory) / "recovery.json").read_text())
                self.assertTrue(record["automationSuspended"])
                self.assertTrue(record["restored"])
                self.assertEqual(record["builtInDisplayID"], 123)
            return result

    def test_warnings_stay_suppressed_until_recovery(self):
        result = self.run_swift(r'''
extension DisplayController {
    func exerciseRetryLogging() {
        let restore = "displayctl: could not restore: CoreGraphics error 1014"
        let disable = "displayctl: could not turn off: CoreGraphics error 1014"
        for _ in 0..<1_000 {
            printReconcileErrorOnce(restore)
        }
        // Alternating failures must not make an old warning appear again.
        for _ in 0..<1_000 {
            printReconcileErrorOnce(disable)
            printReconcileErrorOnce(restore)
        }
        // After recovery, a new failure must be visible again.
        reconcileErrors.reset()
        printReconcileErrorOnce(restore)
        printReconcileErrorOnce(restore)
    }
}
DisplayController().exerciseRetryLogging()
''')

        suffix = (
            " Retrying automatically; duplicate warnings will be suppressed "
            "until recovery."
        )
        self.assertEqual(result.stdout, "")
        # Drop the "[yyyy-MM-dd HH:mm:ss.SSS] " status timestamps.
        self.assertEqual([line.split("] ", 1)[1] for line in result.stderr.splitlines()], [
            "displayctl: could not restore: CoreGraphics error 1014" + suffix,
            "displayctl: could not turn off: CoreGraphics error 1014" + suffix,
            "displayctl: could not restore: CoreGraphics error 1014" + suffix,
        ])

    def test_sleep_wake_and_unplug_configuration_decisions(self):
        result = self.run_swift(r'''
extension DisplayController {
    func exerciseSleepWake() {
        var requests: [Bool] = []
        func step(_ state: ExternalDisplayState, online: Bool, time: Double) {
            reconcileDisplayState(
                externalState: state, builtInOnline: online, builtInUsable: online,
                now: Date(timeIntervalSinceReferenceDate: time)
            ) { requests.append($0) }
        }

        step(.ready, online: false, time: 0)
        for tick in 1...1_000 {
            step(.inactive, online: false, time: Double(tick))
        }
        // Sleeping must neither restore an offline panel nor disable an online one.
        step(.inactive, online: true, time: 1_001)
        precondition(requests.isEmpty, "Sleep must not configure the built-in display")

        step(.ready, online: true, time: 1_002)
        step(.ready, online: true, time: 1_003.9)
        precondition(requests.isEmpty, "Wake must start a fresh settle interval")
        step(.ready, online: true, time: 1_004)
        precondition(requests == [false], "Disable only after the monitor settles")
        step(.ready, online: false, time: 1_005)
        precondition(requests == [false], "Do not reconfigure a satisfied state")

        // A real unplug still restores promptly, even if the monitor was asleep.
        step(.inactive, online: false, time: 1_006)
        step(.absent, online: false, time: 1_007)
        precondition(requests == [false, true], "Unplug must restore the panel")
        step(.absent, online: true, time: 1_008)
        precondition(requests == [false, true])

        step(.ready, online: true, time: 1_009)
        step(.ready, online: true, time: 1_010.9)
        precondition(requests == [false, true], "Reconnect must also settle")
        step(.ready, online: true, time: 1_011)
        precondition(requests == [false, true, false])
    }
}
DisplayController().exerciseSleepWake()
''')
        self.assertEqual(result.stderr, "")

    def test_sleep_does_not_reset_failed_restoration_warnings(self):
        result = self.run_swift(r'''
extension DisplayController {
    func exerciseFailedRestoration() {
        var attempts = 0
        func fail(_ state: ExternalDisplayState) {
            reconcileDisplayState(externalState: state, builtInOnline: false, builtInUsable: false) { online in
                precondition(online)
                attempts += 1
                throw ToolError.complete(CGError(rawValue: 1014)!)
            }
        }
        fail(.absent)
        fail(.inactive)
        fail(.absent)
        precondition(attempts == 2, "Sleep must skip configuration, unplug must retry")
        // Recovery observed without our intervention must re-enable warnings too.
        reconcileDisplayState(externalState: .absent, builtInOnline: true, builtInUsable: true) { _ in
            preconditionFailure("Already restored")
        }
        fail(.absent)
        precondition(attempts == 3)
    }
}
DisplayController().exerciseFailedRestoration()
''')
        self.assertEqual(result.stdout, "")
        warnings = [line.split("] ", 1)[1] for line in result.stderr.splitlines()]
        self.assertEqual(len(warnings), 2)
        self.assertEqual(warnings[0], warnings[1])
        self.assertIn("CoreGraphics error 1014", warnings[0])


if __name__ == "__main__":
    unittest.main()
