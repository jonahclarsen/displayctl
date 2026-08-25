import CoreGraphics
import Darwin
import Foundation

private typealias ConfigureDisplayEnabled = @convention(c) (
    OpaquePointer?, CGDirectDisplayID, Int32
) -> CGError

private typealias GetDisplayList = @convention(c) (
    UInt32,
    UnsafeMutablePointer<CGDirectDisplayID>?,
    UnsafeMutablePointer<UInt32>?
) -> CGError

private typealias CreateDisplayInfo = @convention(c) (
    CGDirectDisplayID
) -> Unmanaged<CFDictionary>?

private enum ToolError: Error, CustomStringConvertible {
    case privateAPIUnavailable
    case noBuiltInDisplay
    case multipleBuiltInDisplays([CGDirectDisplayID])
    case begin(CGError)
    case configure(CGError)
    case complete(CGError)
    case verification(CGDirectDisplayID, expectedOnline: Bool)
    case nothingToRestore
    case watchdogAlreadyRunning(Int32)
    case invalidArguments

    static let usage = """
    Usage:
      displayctl list
      displayctl off [--restore-after SECONDS]
      displayctl on

    `off` stays running as a dock supervisor. It restores the built-in panel when
    every physical external disappears and turns it off again after one reconnects.
    Press Control-C, or run `displayctl on`, to restore the panel and stop it.
    """

    var description: String {
        switch self {
        case .privateAPIUnavailable:
            return "The macOS soft-disconnect API is unavailable."
        case .noBuiltInDisplay:
            return "No built-in display was found."
        case let .multipleBuiltInDisplays(ids):
            return "Multiple built-in displays were found (\(ids.map(String.init).joined(separator: ", "))); refusing to guess."
        case let .begin(error):
            return "Could not begin display configuration (CoreGraphics error \(error.rawValue))."
        case let .configure(error):
            return "Could not change display state (CoreGraphics error \(error.rawValue))."
        case let .complete(error):
            return "Could not commit display configuration (CoreGraphics error \(error.rawValue))."
        case let .verification(id, expectedOnline):
            return "Display \(id) did not become \(expectedOnline ? "online" : "offline")."
        case .nothingToRestore:
            return "No offline built-in display or recovery record was found."
        case let .watchdogAlreadyRunning(pid):
            return "An external-only watchdog is already running (PID \(pid)). Use `displayctl on` or stop that command with Control-C."
        case .invalidArguments:
            return Self.usage
        }
    }
}

private final class PrivateDisplayAPI {
    private let handles: [UnsafeMutableRawPointer]
    let setEnabled: ConfigureDisplayEnabled?
    private let getAll: GetDisplayList?
    private let createInfo: CreateDisplayInfo?

    init() {
        let paths: [String?] = [
            nil,
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            "/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics",
            "/System/Library/Frameworks/CoreDisplay.framework/CoreDisplay",
        ]
        handles = paths.compactMap { dlopen($0, RTLD_LAZY | RTLD_LOCAL) }
        setEnabled = Self.resolve(
            handles,
            names: ["SLSConfigureDisplayEnabled", "CGSConfigureDisplayEnabled"],
            as: ConfigureDisplayEnabled.self
        )
        getAll = Self.resolve(
            handles,
            names: ["SLSGetDisplayList", "CGSGetDisplayList"],
            as: GetDisplayList.self
        )
        createInfo = Self.resolve(
            handles,
            names: ["CoreDisplay_DisplayCreateInfoDictionary"],
            as: CreateDisplayInfo.self
        )
    }

    deinit {
        handles.forEach { dlclose($0) }
    }

    private static func resolve<Function>(
        _ handles: [UnsafeMutableRawPointer],
        names: [String],
        as type: Function.Type
    ) -> Function? {
        for handle in handles {
            for name in names {
                if let symbol = dlsym(handle, name) {
                    return unsafeBitCast(symbol, to: type)
                }
            }
        }
        return nil
    }

    func displayIDs() -> [CGDirectDisplayID] {
        if let getAll {
            var count: UInt32 = 0
            if getAll(0, nil, &count) == .success, count > 0 {
                var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
                if getAll(count, &ids, &count) == .success {
                    return Array(ids.prefix(Int(count)))
                }
            }
        }

        var count: UInt32 = 0
        guard CGGetOnlineDisplayList(0, nil, &count) == .success, count > 0 else { return [] }
        var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
        guard CGGetOnlineDisplayList(count, &ids, &count) == .success else { return [] }
        return Array(ids.prefix(Int(count)))
    }

    func onlineDisplayIDs() -> [CGDirectDisplayID] {
        var count: UInt32 = 0
        guard CGGetOnlineDisplayList(0, nil, &count) == .success, count > 0 else { return [] }
        var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
        guard CGGetOnlineDisplayList(count, &ids, &count) == .success else { return [] }
        return Array(ids.prefix(Int(count)))
    }

    func physicalExternalName(displayID: CGDirectDisplayID) -> String? {
        guard CGDisplayIsBuiltin(displayID) == 0,
              let dictionary = createInfo?(displayID)?.takeRetainedValue() as? [String: Any],
              dictionary["kCGDisplayIsVirtualDevice"] as? Bool != true,
              dictionary["kCGDisplayIsAirPlay"] as? Bool != true else {
            return nil
        }
        let names = dictionary["DisplayProductName"] as? [String: String]
        let name = names?[Locale.current.identifier]
            ?? names?["en_US"]
            ?? names?.values.first
        guard let name = name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty else {
            return nil
        }
        return name
    }
}

private struct RecoveryRecord: Codable {
    let builtInDisplayID: CGDirectDisplayID
    let watchdogPID: Int32?
}

private func displayReconfigurationCallback(
    _: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    userInfo: UnsafeMutableRawPointer?
) {
    guard let userInfo, !flags.contains(.beginConfigurationFlag) else { return }
    let controller = Unmanaged<DisplayController>.fromOpaque(userInfo).takeUnretainedValue()
    controller.handleDisplayReconfiguration()
}

private final class DisplayController {
    private let api = PrivateDisplayAPI()
    private let watchdogQueue = DispatchQueue(label: "displayctl.watchdog")
    private var restoring = false
    private var signalSources: [DispatchSourceSignal] = []
    private var externalMonitor: DispatchSourceTimer?
    private var displayCallbackRegistered = false
    private var observedExternalPresent: Bool?
    private var externalPresentSince = Date.distantPast
    private let reconnectSettleSeconds: TimeInterval = 2

    private var recoveryURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".local/state/displayctl", isDirectory: true)
            .appendingPathComponent("recovery.json")
    }

    func list() {
        print("Soft-disconnect API: \(api.setEnabled == nil ? "unavailable" : "available")")
        for id in api.displayIDs() {
            let bounds = CGDisplayBounds(id)
            let mirror = CGDisplayMirrorsDisplay(id)
            var traits = [CGDisplayIsBuiltin(id) != 0 ? "built-in" : "external"]
            traits.append(CGDisplayIsActive(id) != 0 ? "active" : "inactive")
            traits.append(CGDisplayIsOnline(id) != 0 ? "online" : "offline")
            if CGDisplayIsMain(id) != 0 { traits.append("main") }
            if mirror != kCGNullDirectDisplay { traits.append("mirrors=\(mirror)") }
            if let name = api.physicalExternalName(displayID: id) { traits.append("physical=\(name)") }
            print("\(id): \(Int(bounds.width))x\(Int(bounds.height)) @ \(Int(bounds.origin.x)),\(Int(bounds.origin.y)) [\(traits.joined(separator: ", "))]")
        }
        if let record = try? loadRecovery(), let pid = record.watchdogPID, processIsRunning(pid) {
            print("Watchdog: running (PID \(pid), built-in display \(record.builtInDisplayID))")
        }
    }

    func probeExternalDisplay() -> Bool {
        hasActiveExternalDisplay()
    }

    func turnOff(restoreAfter: TimeInterval?) throws -> Never {
        try recoverInterruptedRun()

        let ids = api.displayIDs()
        let builtIns = ids.filter { CGDisplayIsBuiltin($0) != 0 }
        guard !builtIns.isEmpty else { throw ToolError.noBuiltInDisplay }
        guard builtIns.count == 1 else { throw ToolError.multipleBuiltInDisplays(builtIns) }

        let builtInID = builtIns[0]
        let externalPresent = hasActiveExternalDisplay()
        let builtInAlreadyOffline = CGDisplayIsOnline(builtInID) == 0
        try saveRecovery(displayID: builtInID, watchdogPID: getpid())
        if externalPresent && !builtInAlreadyOffline {
            do {
                try configure(displayID: builtInID, online: false)
            } catch {
                try? clearRecovery()
                throw error
            }
        }

        observedExternalPresent = externalPresent
        externalPresentSince = Date()

        if !externalPresent {
            print("No physical external display is connected. Dock supervisor active; waiting for a monitor to connect.")
        } else if builtInAlreadyOffline {
            print("Built-in display \(builtInID) is already off. Dock supervisor active; press Control-C to stop and restore it.")
        } else {
            print("Built-in display \(builtInID) is off. Dock supervisor active; press Control-C to stop and restore it.")
        }
        fflush(stdout)

        installSignal(SIGINT, displayID: builtInID)
        installSignal(SIGTERM, displayID: builtInID)
        installSignal(SIGHUP, displayID: builtInID)
        registerDisplayCallback()

        let externalMonitor = DispatchSource.makeTimerSource(queue: watchdogQueue)
        externalMonitor.schedule(deadline: .now() + 1, repeating: 1)
        externalMonitor.setEventHandler { [self] in
            reconcileDisplayState(displayID: builtInID)
        }
        externalMonitor.resume()
        self.externalMonitor = externalMonitor

        if let restoreAfter {
            watchdogQueue.asyncAfter(deadline: .now() + restoreAfter) { [self] in
                restoreAndExit(displayID: builtInID, message: "Timed test finished; built-in display restored.")
            }
        }

        dispatchMain()
    }

    func turnOn() throws {
        let record = try loadRecovery()
        let recordedID = record?.builtInDisplayID
        let offlineBuiltIn = api.displayIDs().first {
            CGDisplayIsBuiltin($0) != 0 && CGDisplayIsOnline($0) == 0
        }
        guard let id = recordedID ?? offlineBuiltIn else { throw ToolError.nothingToRestore }

        if let pid = record?.watchdogPID, pid != getpid(), processIsRunning(pid) {
            _ = kill(pid, SIGTERM)
            for _ in 0..<50 {
                if CGDisplayIsOnline(id) != 0 && !processIsRunning(pid) { break }
                Thread.sleep(forTimeInterval: 0.1)
            }
        }
        if CGDisplayIsOnline(id) == 0 {
            try configure(displayID: id, online: true)
        }
        try clearRecovery()
        print("Built-in display \(id) is on.")
    }

    fileprivate func handleDisplayReconfiguration() {
        watchdogQueue.async { [self] in
            guard let record = try? loadRecovery() else { return }
            let displayID = record.builtInDisplayID
            // Give WindowServer a moment to publish the final online display list.
            watchdogQueue.asyncAfter(deadline: .now() + 0.15) { [self] in
                reconcileDisplayState(displayID: displayID)
            }
        }
    }

    private func hasActiveExternalDisplay() -> Bool {
        api.onlineDisplayIDs().contains {
            CGDisplayIsBuiltin($0) == 0 && CGDisplayIsActive($0) != 0 &&
                api.physicalExternalName(displayID: $0) != nil
        }
    }

    private func reconcileDisplayState(displayID: CGDirectDisplayID) {
        guard !restoring else { return }
        let externalPresent = freshProcessHasActiveExternalDisplay()
        let now = Date()

        if observedExternalPresent != externalPresent {
            observedExternalPresent = externalPresent
            externalPresentSince = now
            if externalPresent {
                printStatus("Physical external display connected; waiting for it to settle.")
            }
        }

        if !externalPresent {
            if CGDisplayIsOnline(displayID) == 0 {
                do {
                    try configure(displayID: displayID, online: true)
                    printStatus("External display disappeared; built-in display restored. Waiting for a monitor to reconnect.")
                } catch {
                    printStatus("displayctl: could not restore the built-in display yet: \(error)", toError: true)
                }
            }
            return
        }

        guard CGDisplayIsOnline(displayID) != 0,
              now.timeIntervalSince(externalPresentSince) >= reconnectSettleSeconds else {
            return
        }
        do {
            try configure(displayID: displayID, online: false)
            printStatus("External display is ready; built-in display turned off again.")
        } catch {
            printStatus("displayctl: could not turn off the built-in display yet: \(error)", toError: true)
        }
    }

    private func printStatus(_ message: String, toError: Bool = false) {
        if toError {
            fputs("\(message)\n", stderr)
            fflush(stderr)
        } else {
            print(message)
            fflush(stdout)
        }
    }

    private func freshProcessHasActiveExternalDisplay() -> Bool {
        var pathBufferSize: UInt32 = 0
        _NSGetExecutablePath(nil, &pathBufferSize)
        var pathBuffer = [CChar](repeating: 0, count: Int(pathBufferSize))
        guard _NSGetExecutablePath(&pathBuffer, &pathBufferSize) == 0 else { return false }

        let probe = Process()
        probe.executableURL = URL(fileURLWithPath: String(cString: pathBuffer))
        probe.arguments = ["probe-external"]
        probe.standardOutput = FileHandle.nullDevice
        probe.standardError = FileHandle.nullDevice
        do {
            try probe.run()
            probe.waitUntilExit()
            return probe.terminationReason == .exit && probe.terminationStatus == 0
        } catch {
            // Safety wins: if current hardware presence cannot be established,
            // bring the built-in display back rather than risk a black desktop.
            return false
        }
    }

    private func registerDisplayCallback() {
        guard !displayCallbackRegistered else { return }
        let pointer = Unmanaged.passUnretained(self).toOpaque()
        if CGDisplayRegisterReconfigurationCallback(displayReconfigurationCallback, pointer) == .success {
            displayCallbackRegistered = true
        }
    }

    private func configure(displayID: CGDirectDisplayID, online: Bool) throws {
        guard let setEnabled = api.setEnabled else { throw ToolError.privateAPIUnavailable }
        var config: CGDisplayConfigRef?
        let begun = CGBeginDisplayConfiguration(&config)
        guard begun == .success else { throw ToolError.begin(begun) }

        let changed = setEnabled(config, displayID, online ? 1 : 0)
        guard changed == .success else {
            CGCancelDisplayConfiguration(config)
            throw ToolError.configure(changed)
        }

        let completed = CGCompleteDisplayConfiguration(config, .forSession)
        // The private API can report a commit error even though WindowServer
        // applies the requested state. Treat the observed display state as the
        // source of truth so a successful transition is not retried and logged.
        let deadline = Date().addingTimeInterval(1.5)
        repeat {
            if (CGDisplayIsOnline(displayID) != 0) == online { return }
            Thread.sleep(forTimeInterval: 0.1)
        } while Date() < deadline

        guard completed == .success else { throw ToolError.complete(completed) }
        throw ToolError.verification(displayID, expectedOnline: online)
    }

    private func recoverInterruptedRun() throws {
        guard let record = try loadRecovery() else { return }
        if let pid = record.watchdogPID, pid != getpid(), processIsRunning(pid) {
            throw ToolError.watchdogAlreadyRunning(pid)
        }
        if CGDisplayIsOnline(record.builtInDisplayID) == 0 {
            try configure(displayID: record.builtInDisplayID, online: true)
            print("Recovered built-in display \(record.builtInDisplayID) from an interrupted run.")
        }
        try clearRecovery()
    }

    private func saveRecovery(displayID: CGDirectDisplayID, watchdogPID: Int32) throws {
        let directory = recoveryURL.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let data = try JSONEncoder().encode(RecoveryRecord(builtInDisplayID: displayID, watchdogPID: watchdogPID))
        try data.write(to: recoveryURL, options: .atomic)
    }

    private func loadRecovery() throws -> RecoveryRecord? {
        guard FileManager.default.fileExists(atPath: recoveryURL.path) else { return nil }
        return try JSONDecoder().decode(RecoveryRecord.self, from: Data(contentsOf: recoveryURL))
    }

    private func clearRecovery() throws {
        guard FileManager.default.fileExists(atPath: recoveryURL.path) else { return }
        try FileManager.default.removeItem(at: recoveryURL)
    }

    private func processIsRunning(_ pid: Int32) -> Bool {
        guard pid > 1 else { return false }
        if kill(pid, 0) == 0 { return true }
        return errno == EPERM
    }

    private func installSignal(_ number: Int32, displayID: CGDirectDisplayID) {
        signal(number, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: number, queue: watchdogQueue)
        source.setEventHandler { [self] in
            restoreAndExit(displayID: displayID, message: "Built-in display restored.")
        }
        source.resume()
        signalSources.append(source)
    }

    private func restoreAndExit(displayID: CGDirectDisplayID, message: String) -> Never {
        guard !restoring else { exit(EXIT_SUCCESS) }
        restoring = true
        do {
            if displayCallbackRegistered {
                CGDisplayRemoveReconfigurationCallback(
                    displayReconfigurationCallback,
                    Unmanaged.passUnretained(self).toOpaque()
                )
                displayCallbackRegistered = false
            }
            if CGDisplayIsOnline(displayID) == 0 {
                try configure(displayID: displayID, online: true)
            }
            try clearRecovery()
            print(message)
            fflush(stdout)
            exit(EXIT_SUCCESS)
        } catch {
            fputs("displayctl: restore failed: \(error)\n", stderr)
            fflush(stderr)
            exit(EXIT_FAILURE)
        }
    }
}

private func restoreDelay(from arguments: [String]) throws -> TimeInterval? {
    guard !arguments.isEmpty else { return nil }
    guard arguments.count == 2, arguments[0] == "--restore-after",
          let seconds = TimeInterval(arguments[1]), seconds > 0 else {
        throw ToolError.invalidArguments
    }
    return seconds
}

do {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard let command = arguments.first else { throw ToolError.invalidArguments }
    let controller = DisplayController()

    switch command {
    case "list" where arguments.count == 1,
         "status" where arguments.count == 1:
        controller.list()
    case "off", "external-only":
        try controller.turnOff(restoreAfter: restoreDelay(from: Array(arguments.dropFirst())))
    case "on" where arguments.count == 1,
         "restore" where arguments.count == 1:
        try controller.turnOn()
    case "probe-external" where arguments.count == 1:
        exit(controller.probeExternalDisplay() ? EXIT_SUCCESS : EXIT_FAILURE)
    default:
        throw ToolError.invalidArguments
    }
} catch {
    fputs("displayctl: \(error)\n", stderr)
    exit(EXIT_FAILURE)
}
