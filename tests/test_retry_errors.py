"""Exercise the production warning path without changing any display settings.

Run with: python3 tests/test_retry_errors.py
"""

from pathlib import Path
import subprocess
import tempfile
import unittest


class RetryErrorTests(unittest.TestCase):
    def test_daily_brightness_schedule(self):
        self.run_swift(r'''
var calendar = Calendar(identifier: .gregorian)
calendar.timeZone = TimeZone(identifier: "America/Vancouver")!
let morning = calendar.date(from: DateComponents(year: 2026, month: 9, day: 14, hour: 8))!
private var rule = DailyBrightnessRule()
func tick(_ seconds: Double, _ displays: [String] = ["A"]) -> [String] {
    rule.due(displays: displays, now: morning.addingTimeInterval(seconds),
             uptime: seconds + 100, calendar: calendar)
}
precondition(tick(-1).isEmpty)
for second in 0..<5 { precondition(tick(Double(second)).isEmpty) }
precondition(tick(5) == ["A"], "Connected at 8:00 must wait five seconds")
rule.succeeded("A", now: morning.addingTimeInterval(5), calendar: calendar)
precondition(tick(6).isEmpty)
precondition(tick(7, []).isEmpty)
for second in 8...14 { precondition(tick(Double(second)).isEmpty) }

// Persistence uses stable monitor UUIDs, not transient display IDs.
let encoded = try JSONEncoder().encode(rule.completed)
rule = DailyBrightnessRule()
rule.completed = try JSONDecoder().decode([String: String].self, from: encoded)
precondition(tick(15).isEmpty, "Restart must not repeat a completed day")

// A second monitor arriving at 9 a.m. has its own delay.
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

// Next day resets automatically, but nothing happens before 08:00.
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

    def run_swift(self, harness):
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
            return subprocess.run(
                [str(binary)], capture_output=True, text=True, check=True
            )

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
        self.assertEqual(result.stderr.splitlines(), [
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
                externalState: state, builtInOnline: online,
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
            reconcileDisplayState(externalState: state, builtInOnline: false) { online in
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
        reconcileDisplayState(externalState: .absent, builtInOnline: true) { _ in
            preconditionFailure("Already restored")
        }
        fail(.absent)
        precondition(attempts == 3)
    }
}
DisplayController().exerciseFailedRestoration()
''')
        self.assertEqual(result.stdout, "")
        warnings = result.stderr.splitlines()
        self.assertEqual(len(warnings), 2)
        self.assertEqual(warnings[0], warnings[1])
        self.assertIn("CoreGraphics error 1014", warnings[0])


if __name__ == "__main__":
    unittest.main()
