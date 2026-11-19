import Foundation

/// Swift wrapper over the control connection to aleno-daemon.
final class DaemonClient {
    static let shared = DaemonClient()
    private var ctl: OpaquePointer?

    private var lifetime: FileHandle?
    private(set) var socketPath = ""

    /// Each app launch owns a private helper. Closing its stdin ends its lifetime.
    func bootstrap() -> Bool {
        let directory = URL(fileURLWithPath: "/tmp/aleno-" + UUID().uuidString, isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                                    attributes: [.posixPermissions: 0o700])
        } catch {
            NSLog("Aleno: could not create helper directory: %@", String(describing: error))
            return false
        }
        socketPath = directory.appendingPathComponent("daemon.sock").path
        let child = Process()
        let pipe = Pipe()
        child.executableURL = ProcessInfo.processInfo.environment["ALENO_DAEMON_PATH"].map { URL(fileURLWithPath: $0) }
            ?? Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/aleno-daemon")
        child.arguments = ["run-app"]
        var environment = ProcessInfo.processInfo.environment
        environment["ALENO_SOCKET"] = socketPath
        child.environment = environment
        child.standardInput = pipe
        do {
            try child.run()
        } catch {
            NSLog("Aleno: could not start helper: %@", String(describing: error))
            try? FileManager.default.removeItem(at: directory)
            return false
        }
        lifetime = pipe.fileHandleForWriting
        let deadline = ProcessInfo.processInfo.systemUptime + 3
        while child.isRunning, ProcessInfo.processInfo.systemUptime < deadline {
            if connect() { return true }
            Thread.sleep(forTimeInterval: 0.01)
        }
        shutdown()
        return false
    }

    func shutdown() {
        if let ctl { aleno_ctl_free(ctl); self.ctl = nil }
        try? lifetime?.close()
        lifetime = nil
    }

    func connect() -> Bool {
        if let c = ctl { aleno_ctl_free(c); ctl = nil }
        ctl = aleno_ctl_connect(socketPath)
        return ctl != nil
    }

    private func withCtl<T>(_ body: (OpaquePointer) -> T?) -> T? {
        if ctl == nil, !connect() { return nil }
        if let r = body(ctl!) { return r }
        // One reconnect attempt covers a transient control-connection failure.
        if connect() { return body(ctl!) }
        return nil
    }

    func createSession(name: String) -> UInt32? {
        withCtl { c in let r = aleno_ctl_create_session(c, name); return r >= 0 ? UInt32(r) : nil }
    }

    func killSession(_ id: UInt32) { _ = withCtl { c in aleno_ctl_kill_session(c, id) == 1 ? true : nil } }

    var lastError: String {
        guard let p = aleno_ctl_last_error() else { return "Unknown daemon error" }
        defer { aleno_string_free(p) }
        return String(cString: p)
    }

    /// `seed`: output of an earlier pane (`TerminalView.snapshotText`) to replay above the new shell's first prompt.
    func createPane(session: UInt32, cols: Int, rows: Int, shell: String, cwd: String, scrollback: Int, seed: String = "") -> UInt32? {
        withCtl { c in
            let r = seed.isEmpty
                ? aleno_ctl_create_pane(c, session, UInt16(cols), UInt16(rows), shell, cwd, UInt32(scrollback))
                : aleno_ctl_create_pane_seeded(c, session, UInt16(cols), UInt16(rows), shell, cwd, UInt32(scrollback), seed)
            return r >= 0 ? UInt32(r) : nil
        }
    }

    func killPane(session: UInt32, pane: UInt32) { _ = withCtl { c in aleno_ctl_kill_pane(c, session, pane) == 1 ? true : nil } }
}
