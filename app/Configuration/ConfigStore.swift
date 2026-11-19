import AppKit

extension Notification.Name {
    static let alenoSettingsChanged = Notification.Name("AlenoSettingsChanged")
    static let alenoConfigDiagnostics = Notification.Name("AlenoConfigDiagnostics")
}

/// Loads ~/.config/aleno/settings.json, validates it atomically and live-reloads on change.
/// An invalid file never replaces the last valid configuration.
final class ConfigStore {
    static let shared = ConfigStore()

    private(set) var settings = Settings()
    private(set) var lastError: String?

    let directory: URL
    let fileURL: URL
    private var fileSource: DispatchSourceFileSystemObject?
    private var dirSource: DispatchSourceFileSystemObject?
    private var debounce: DispatchWorkItem?
    private var lastData: Data?

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        directory = home.appendingPathComponent(".config/aleno", isDirectory: true)
        fileURL = directory.appendingPathComponent("settings.json")
    }

    /// Creates the directory, schema copy and default file when they do not exist.
    func ensureFileExists() {
        let fm = FileManager.default
        try? fm.createDirectory(at: directory, withIntermediateDirectories: true)
        let schemaDst = directory.appendingPathComponent("settings.schema.json")
        if let src = Bundle.main.url(forResource: "settings.schema", withExtension: "json"),
           let data = try? Data(contentsOf: src) {
            if (try? Data(contentsOf: schemaDst)) != data { try? data.write(to: schemaDst, options: .atomic) }
        }
        if !fm.fileExists(atPath: fileURL.path) {
            try? Settings.defaultFileText.data(using: .utf8)?.write(to: fileURL, options: .atomic)
        }
    }

    func start() {
        ensureFileExists()
        reload(initial: true)
        watch()
    }

    private func watch() {
        fileSource?.cancel(); dirSource?.cancel()
        // File: catches in-place writes. Re-armed after delete/rename (atomic-save editors).
        let fd = open(fileURL.path, O_EVTONLY)
        if fd >= 0 {
            let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .extend, .delete, .rename, .attrib], queue: .main)
            src.setEventHandler { [weak self] in
                guard let self else { return }
                let ev = src.data
                self.scheduleReload()
                if ev.contains(.delete) || ev.contains(.rename) {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { self.watch() }
                }
            }
            src.setCancelHandler { close(fd) }
            src.resume()
            fileSource = src
        }
        // Directory: catches the file being created or replaced.
        let dfd = open(directory.path, O_EVTONLY)
        if dfd >= 0 {
            let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: dfd, eventMask: [.write], queue: .main)
            src.setEventHandler { [weak self] in
                guard let self else { return }
                self.scheduleReload()
                if self.fileSource == nil {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
                        guard let self, self.fileSource == nil else { return }
                        self.watch()
                    }
                }
            }
            src.setCancelHandler { close(dfd) }
            src.resume()
            dirSource = src
        }
    }

    private func scheduleReload() {
        debounce?.cancel()
        let w = DispatchWorkItem { [weak self] in self?.reload(initial: false) }
        debounce = w
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.12, execute: w)
    }

    func reload(initial: Bool) {
        let data: Data
        do { data = try Data(contentsOf: fileURL) } catch {
            if initial { return }
            // File vanished: keep the active configuration.
            report(errors: ["settings.json cannot be read: \(error.localizedDescription)"], warnings: [])
            return
        }
        if data == lastData && !initial { return }
        lastData = data
        let (parsed, diag) = Settings.parse(data: data)
        guard let new = parsed else {
            report(errors: diag.errors, warnings: diag.warnings)
            return
        }
        lastError = nil
        let changed = new != settings
        settings = new
        report(errors: [], warnings: diag.warnings)
        if changed || initial { NotificationCenter.default.post(name: .alenoSettingsChanged, object: self) }
    }

    private func report(errors: [String], warnings: [String]) {
        if !errors.isEmpty {
            lastError = errors.joined(separator: "\n")
            NSLog("Aleno: settings.json rejected, keeping previous configuration: %@", lastError!)
        }
        for w in warnings { NSLog("Aleno: settings.json warning: %@", w) }
        NotificationCenter.default.post(name: .alenoConfigDiagnostics, object: self,
                                        userInfo: ["errors": errors, "warnings": warnings])
    }
}

/// Opens settings.json in the user's editor: a GUI editor from $VISUAL/$EDITOR when available,
/// a terminal editor inside a new Aleno tab, else the default macOS text editor.
enum EditorLauncher {
    static var resolvedEditor: String?   // filled asynchronously from the user's login shell environment

    static func resolveFromLoginShell() {
        DispatchQueue.global(qos: .utility).async {
            let shell = ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh"
            let p = Process()
            p.executableURL = URL(fileURLWithPath: shell)
            p.arguments = ["-ilc", "printf '%s' \"${VISUAL:-$EDITOR}\""]
            let out = Pipe(); p.standardOutput = out; p.standardError = FileHandle.nullDevice; p.standardInput = FileHandle.nullDevice
            do { try p.run() } catch { return }
            let killer = DispatchWorkItem { if p.isRunning { p.terminate() } }
            DispatchQueue.global().asyncAfter(deadline: .now() + 4, execute: killer)
            p.waitUntilExit(); killer.cancel()
            let s = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
            // Take the last line: interactive shells may print banners before the value.
            let value = s?.split(separator: "\n").last.map(String.init) ?? ""
            DispatchQueue.main.async { resolvedEditor = value.isEmpty ? nil : value }
        }
    }

    static let guiEditors: [String: [String]] = [
        "code": [], "cursor": [], "subl": [], "zed": [], "mate": [], "bbedit": [], "atom": [], "windsurf": [], "idea": [], "xed": [],
        "code-insiders": [], "codium": [],
    ]

    enum Plan { case gui(URL, [String]); case terminal(String); case fallback }

    static func plan() -> Plan {
        let env = ProcessInfo.processInfo.environment
        let editor = (env["VISUAL"] ?? env["EDITOR"]).flatMap { $0.isEmpty ? nil : $0 } ?? resolvedEditor
        guard let editor else { return .fallback }
        let parts = editor.split(separator: " ").map(String.init)
        guard let cmd = parts.first else { return .fallback }
        let name = (cmd as NSString).lastPathComponent
        if guiEditors[name] != nil {
            for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/Applications/Visual Studio Code.app/Contents/Resources/app/bin"] {
                let path = cmd.hasPrefix("/") ? cmd : dir + "/" + cmd
                if FileManager.default.isExecutableFile(atPath: path) { return .gui(URL(fileURLWithPath: path), Array(parts.dropFirst())) }
            }
            return .fallback
        }
        // Anything else is treated as a terminal editor.
        if name.isEmpty || name.contains(where: { !($0.isLetter || $0.isNumber || "-_.".contains($0)) }) { return .fallback }
        return .terminal(name)
    }
}
