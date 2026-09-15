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

private typealias CreateDisplayUUID = @convention(c) (CGDirectDisplayID) -> Unmanaged<CFUUID>?

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
      displayctl brightness-install
      displayctl brightness-watch

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
    let createUUID: CreateDisplayUUID?

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
        createUUID = Self.resolve(handles, names: ["CGDisplayCreateUUIDFromDisplayID"], as: CreateDisplayUUID.self)
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

private enum ExternalDisplayState: Int32 {
    case ready = 0
    case absent = 1
    case inactive = 2
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

// Remember every warning in a failure episode, even if errors alternate.
// Only reaching the requested display state ends the episode; a topology
// notification alone does not mean that a failed configuration recovered.
private struct ReconcileErrorLog {
    private var reportedMessages: Set<String> = []

    mutating func shouldReport(_ message: String) -> Bool {
        reportedMessages.insert(message).inserted
    }

    mutating func reset() {
        reportedMessages.removeAll()
    }
}

private final class DisplayController {
    private let api = PrivateDisplayAPI()
    private let watchdogQueue = DispatchQueue(label: "displayctl.watchdog")
    private var restoring = false
    private var signalSources: [DispatchSourceSignal] = []
    private var externalMonitor: DispatchSourceTimer?
    private var displayCallbackRegistered = false
    private var observedExternalState: ExternalDisplayState?
    private var externalReadySince = Date.distantPast
    private var reconcileErrors = ReconcileErrorLog()
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
            if CGDisplayIsAsleep(id) != 0 { traits.append("asleep") }
            if CGDisplayIsMain(id) != 0 { traits.append("main") }
            if mirror != kCGNullDirectDisplay { traits.append("mirrors=\(mirror)") }
            if let name = api.physicalExternalName(displayID: id) { traits.append("physical=\(name)") }
            print("\(id): \(Int(bounds.width))x\(Int(bounds.height)) @ \(Int(bounds.origin.x)),\(Int(bounds.origin.y)) [\(traits.joined(separator: ", "))]")
        }
        if let record = try? loadRecovery(), let pid = record.watchdogPID, processIsRunning(pid) {
            print("Watchdog: running (PID \(pid), built-in display \(record.builtInDisplayID))")
        }
    }

    func probeExternalDisplay() -> ExternalDisplayState {
        let physicalExternals = api.onlineDisplayIDs().filter {
            api.physicalExternalName(displayID: $0) != nil
        }
        guard !physicalExternals.isEmpty else { return .absent }
        // Online includes sleeping displays. Active additionally requires the
        // display to be awake and drawable; inactivity is not a disconnect.
        return physicalExternals.contains { CGDisplayIsActive($0) != 0 }
            ? .ready : .inactive
    }

    func turnOff(restoreAfter: TimeInterval?) throws -> Never {
        try recoverInterruptedRun()

        let ids = api.displayIDs()
        let builtIns = ids.filter { CGDisplayIsBuiltin($0) != 0 }
        guard !builtIns.isEmpty else { throw ToolError.noBuiltInDisplay }
        guard builtIns.count == 1 else { throw ToolError.multipleBuiltInDisplays(builtIns) }

        let builtInID = builtIns[0]
        let externalState = probeExternalDisplay()
        let builtInAlreadyOffline = CGDisplayIsOnline(builtInID) == 0
        try saveRecovery(displayID: builtInID, watchdogPID: getpid())
        if externalState == .ready && !builtInAlreadyOffline {
            do {
                try configure(displayID: builtInID, online: false)
            } catch {
                try? clearRecovery()
                throw error
            }
        }

        observedExternalState = externalState
        externalReadySince = Date()

        if externalState == .absent {
            print("No physical external display is connected. Dock supervisor active; waiting for a monitor to connect.")
        } else if externalState == .inactive {
            print("Physical external display is connected but inactive. Dock supervisor active; waiting for it to wake.")
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

    private func reconcileDisplayState(displayID: CGDirectDisplayID) {
        guard !restoring else { return }
        let externalState = freshProcessExternalDisplayState()
        reconcileDisplayState(
            externalState: externalState,
            builtInOnline: CGDisplayIsOnline(displayID) != 0
        ) { online in
            try configure(displayID: displayID, online: online)
        }
    }

    private func reconcileDisplayState(
        externalState: ExternalDisplayState,
        builtInOnline: Bool,
        now: Date = Date(),
        configure: (Bool) throws -> Void
    ) {
        if observedExternalState != externalState {
            observedExternalState = externalState
            externalReadySince = now
            if externalState == .ready {
                printStatus("Physical external display connected or awake; waiting for it to settle.")
            }
        }

        // Leave the configuration alone while the external monitor sleeps.
        // On wake, require a fresh settle interval before disabling the panel.
        guard externalState != .inactive else { return }
        let externalPresent = externalState == .ready
        if builtInOnline == !externalPresent {
            reconcileErrors.reset()
            return
        }

        if !externalPresent {
            do {
                try configure(true)
                reconcileErrors.reset()
                printStatus("External display disappeared; built-in display restored. Waiting for a monitor to reconnect.")
            } catch {
                printReconcileErrorOnce("displayctl: could not restore the built-in display yet: \(error)")
            }
            return
        }

        guard now.timeIntervalSince(externalReadySince) >= reconnectSettleSeconds else {
            return
        }
        do {
            try configure(false)
            reconcileErrors.reset()
            printStatus("External display is ready; built-in display turned off again.")
        } catch {
            printReconcileErrorOnce("displayctl: could not turn off the built-in display yet: \(error)")
        }
    }

    private func printReconcileErrorOnce(_ message: String) {
        guard reconcileErrors.shouldReport(message) else { return }
        printStatus("\(message) Retrying automatically; duplicate warnings will be suppressed until recovery.", toError: true)
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

    private func freshProcessExternalDisplayState() -> ExternalDisplayState {
        var pathBufferSize: UInt32 = 0
        _NSGetExecutablePath(nil, &pathBufferSize)
        var pathBuffer = [CChar](repeating: 0, count: Int(pathBufferSize))
        guard _NSGetExecutablePath(&pathBuffer, &pathBufferSize) == 0 else { return .absent }

        let probe = Process()
        probe.executableURL = URL(fileURLWithPath: String(cString: pathBuffer))
        probe.arguments = ["probe-external-state"]
        probe.standardOutput = FileHandle.nullDevice
        probe.standardError = FileHandle.nullDevice
        do {
            try probe.run()
            probe.waitUntilExit()
            guard probe.terminationReason == .exit else { return .absent }
            return ExternalDisplayState(rawValue: probe.terminationStatus) ?? .absent
        } catch {
            // Safety wins: if current hardware presence cannot be established,
            // bring the built-in display back rather than risk a black desktop.
            return .absent
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
        let deadline = Date().addingTimeInterval(4)
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

// MARK: Daily external brightness rule (independent of the dock supervisor)

private struct BrightnessDisplay: Codable {
    let id: CGDirectDisplayID
    let uuid: String
    let useDefaultConnection: Bool

    var ddcArguments: [String] { useDefaultConnection ? [] : ["display", uuid] }
}

private struct DailyBrightnessRule {
    var completed: [String: String] = [:]
    private var pending: [String: TimeInterval] = [:]
    private var previousTick: TimeInterval?
    private var previousDate: Date?
    private var pendingDay: String?

    static func day(_ now: Date, calendar: Calendar) -> String {
        let parts = calendar.dateComponents([.year, .month, .day], from: now)
        return "\(parts.year!)-\(parts.month!)-\(parts.day!)"
    }

    mutating func due(displays: [String], now: Date, uptime: TimeInterval,
                      calendar: Calendar = .autoupdatingCurrent) -> [String] {
        let day = Self.day(now, calendar: calendar)
        // A scheduling gap (including sleep) requires a new five-second wait.
        if pendingDay != day || previousTick.map({ uptime - $0 > 2 }) == true
            || previousDate.map({ now.timeIntervalSince($0) > 2 || now < $0 }) == true {
            pending.removeAll()
        }
        pendingDay = day
        previousTick = uptime
        previousDate = now
        guard calendar.component(.hour, from: now) >= 8 else {
            pending.removeAll()
            return []
        }
        let eligible = Set(displays.filter { completed[$0] != day })
        pending = pending.filter { eligible.contains($0.key) }
        for uuid in eligible where pending[uuid] == nil { pending[uuid] = uptime }
        return eligible.filter { uptime - pending[$0]! >= 5 }.sorted()
    }

    mutating func succeeded(_ uuid: String, now: Date, calendar: Calendar = .autoupdatingCurrent) {
        completed[uuid] = Self.day(now, calendar: calendar)
        pending.removeValue(forKey: uuid)
    }

    mutating func retry(_ uuid: String, uptime: TimeInterval) {
        pending[uuid] = uptime + 25 // Retry failures after 30 seconds.
    }
}

private enum BrightnessError: Error {
    case commandFailed(String)
    case alreadyRunning
}

private func capture(_ executable: String, _ arguments: [String]) throws -> String {
    let process = Process()
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    try process.run()
    // All commands used here produce small output. Bound hung DDC/probe calls.
    let deadline = ProcessInfo.processInfo.systemUptime + 10
    while process.isRunning && ProcessInfo.processInfo.systemUptime < deadline {
        Thread.sleep(forTimeInterval: 0.02)
    }
    if process.isRunning { kill(process.processIdentifier, SIGKILL) }
    process.waitUntilExit()
    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        throw BrightnessError.commandFailed("\(executable) \(arguments.joined(separator: " "))")
    }
    return String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

private func brightnessProbe() throws {
    let api = PrivateDisplayAPI()
    let physical = api.onlineDisplayIDs().filter { api.physicalExternalName(displayID: $0) != nil }
    let displays = physical.compactMap { id -> BrightnessDisplay? in
        guard CGDisplayIsActive(id) != 0, CGDisplayIsAsleep(id) == 0,
              let uuid = api.createUUID?(id)?.takeRetainedValue() else { return nil }
        // The ASUS setup accepts the default IOAVService connection even when
        // UUID-selected writes silently do nothing. Avoid ambiguous defaults
        // with multiple physical monitors, including sleeping ones.
        return BrightnessDisplay(id: id, uuid: CFUUIDCreateString(nil, uuid) as String,
                                 useDefaultConnection: physical.count == 1 && CGDisplayIsMain(id) != 0)
    }
    print(String(decoding: try JSONEncoder().encode(displays), as: UTF8.self))
}

private func brightnessWatch() throws -> Never {
    let directory = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".local/state/displayctl", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let lock = open(directory.appendingPathComponent("brightness.lock").path, O_CREAT | O_RDWR, 0o600)
    guard lock >= 0, flock(lock, LOCK_EX | LOCK_NB) == 0 else { throw BrightnessError.alreadyRunning }
    defer { close(lock) }
    let stateURL = directory.appendingPathComponent("brightness.json")
    var rule = DailyBrightnessRule()
    if FileManager.default.fileExists(atPath: stateURL.path) {
        rule.completed = try JSONDecoder().decode([String: String].self, from: Data(contentsOf: stateURL))
    }
    var pathSize: UInt32 = 0
    _NSGetExecutablePath(nil, &pathSize)
    var path = [CChar](repeating: 0, count: Int(pathSize))
    guard _NSGetExecutablePath(&path, &pathSize) == 0 else {
        throw BrightnessError.commandFailed("Could not resolve displayctl executable")
    }
    let executable = String(cString: path)
    let testedHelper = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".local/libexec/displayctl/m1ddc").path
    guard let ddc = [testedHelper, "/opt/homebrew/bin/m1ddc", "/usr/local/bin/m1ddc"].first(where: {
        FileManager.default.isExecutableFile(atPath: $0)
    }) else { throw BrightnessError.commandFailed("Install m1ddc with `brew install m1ddc` first.") }
    var errors = ReconcileErrorLog()
    print("Daily brightness rule active: external monitors to maximum, five seconds after availability at/after 08:00 local time.")
    fflush(stdout)
    while true {
        do {
            // Fresh processes avoid stale WindowServer topology after reconnects.
            let json = try capture(executable, ["probe-brightness-displays"])
            let displays = try JSONDecoder().decode([BrightnessDisplay].self, from: Data(json.utf8))
            let due = rule.due(displays: displays.map(\.uuid), now: Date(),
                               uptime: ProcessInfo.processInfo.systemUptime)
            for uuid in due {
                do {
                    guard let display = displays.first(where: { $0.uuid == uuid }) else { continue }
                    let arguments = display.ddcArguments
                    let maxValue = Int(try capture(ddc, arguments + ["max", "luminance"])) ?? 0
                    // Some monitors accept writes but report 0 for every read.
                    let maximum = maxValue > 0 ? maxValue : 100
                    _ = try capture(ddc, arguments + ["set", "luminance", String(maximum)])
                    var saved = rule
                    saved.succeeded(uuid, now: Date())
                    try JSONEncoder().encode(saved.completed).write(to: stateURL, options: .atomic)
                    rule = saved
                    print("Sent maximum brightness (\(maximum)) to \(uuid) using \(display.useDefaultConnection ? "default connection" : "UUID selection") via \(ddc).")
                    fflush(stdout)
                    errors.reset()
                } catch {
                    rule.retry(uuid, uptime: ProcessInfo.processInfo.systemUptime)
                    let message = "displayctl brightness: \(error)"
                    if errors.shouldReport(message) { fputs(message + "\n", stderr) }
                }
            }
        } catch {
            let message = "displayctl brightness: \(error)"
            if errors.shouldReport(message) { fputs(message + "\n", stderr) }
            // A failed probe must break the continuous availability interval.
            _ = rule.due(displays: [], now: Date(), uptime: ProcessInfo.processInfo.systemUptime)
        }
        Thread.sleep(forTimeInterval: 0.5)
    }
}

private func installBrightnessRule() throws {
    let home = FileManager.default.homeDirectoryForCurrentUser
    let binary = home.appendingPathComponent(".local/bin/displayctl").path
    let logs = home.appendingPathComponent(".local/state/displayctl", isDirectory: true)
    let agents = home.appendingPathComponent("Library/LaunchAgents", isDirectory: true)
    try FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: agents, withIntermediateDirectories: true)
    let label = "com.jonahclarsen.displayctl-brightness"
    let plist = agents.appendingPathComponent(label + ".plist")
    let configuration: [String: Any] = [
        "Label": label, "ProgramArguments": [binary, "brightness-watch"],
        "RunAtLoad": true, "KeepAlive": true, "ThrottleInterval": 30,
        "StandardOutPath": logs.appendingPathComponent("brightness.log").path,
        "StandardErrorPath": logs.appendingPathComponent("brightness-error.log").path,
    ]
    let data = try PropertyListSerialization.data(fromPropertyList: configuration, format: .xml, options: 0)
    try data.write(to: plist, options: .atomic)
    let domain = "gui/\(getuid())"
    _ = try? capture("/bin/launchctl", ["bootout", domain + "/" + label])
    _ = try capture("/bin/launchctl", ["bootstrap", domain, plist.path])
    print("Installed and started daily brightness rule. It will start automatically at login.")
}

// MARK: Command-line entry point
do {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard let command = arguments.first else { throw ToolError.invalidArguments }
    let controller = DisplayController()

    switch command {
    case "brightness-install" where arguments.count == 1:
        try installBrightnessRule()
    case "brightness-watch" where arguments.count == 1:
        try brightnessWatch()
    case "probe-brightness-displays" where arguments.count == 1:
        try brightnessProbe()
    case "list" where arguments.count == 1,
         "status" where arguments.count == 1:
        controller.list()
    case "off", "external-only":
        try controller.turnOff(restoreAfter: restoreDelay(from: Array(arguments.dropFirst())))
    case "on" where arguments.count == 1,
         "restore" where arguments.count == 1:
        try controller.turnOn()
    case "probe-external" where arguments.count == 1:
        exit(controller.probeExternalDisplay() == .ready ? EXIT_SUCCESS : EXIT_FAILURE)
    case "probe-external-state" where arguments.count == 1:
        exit(controller.probeExternalDisplay().rawValue)
    default:
        throw ToolError.invalidArguments
    }
} catch {
    fputs("displayctl: \(error)\n", stderr)
    exit(EXIT_FAILURE)
}
