"""Exercise the production warning path without changing any display settings.

Run with: python3 tests/test_retry_errors.py
"""

from pathlib import Path
import subprocess
import tempfile
import unittest


class RetryErrorTests(unittest.TestCase):
    def test_warnings_stay_suppressed_until_recovery(self):
        source = (Path(__file__).resolve().parents[1] / "main.swift").read_text()
        definitions, marker, _ = source.partition("// MARK: Command-line entry point")
        self.assertTrue(marker, "Could not locate the CLI entry point")
        # Compile in the same file so the harness can exercise private logging
        # without exposing a test command or invoking display configuration APIs.
        harness = r'''
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
'''
        with tempfile.TemporaryDirectory(prefix="displayctl-tests-") as directory:
            main = Path(directory) / "main.swift"
            binary = Path(directory) / "retry-errors"
            main.write_text(definitions + harness)
            subprocess.run(["swiftc", str(main), "-o", str(binary)], check=True)
            result = subprocess.run(
                [str(binary)], capture_output=True, text=True, check=True
            )

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


if __name__ == "__main__":
    unittest.main()
