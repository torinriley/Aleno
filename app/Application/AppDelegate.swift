import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    private var controllers: [WindowController] = []
    private var started = Date()

    // MARK: Lifecycle

    func applicationDidFinishLaunching(_ notification: Notification) {
        started = Date()
        Bench.mark("didFinishLaunching")
        ConfigStore.shared.start()
        CommandNotifier.shared.install()
        CommandNotifier.shared.onActivate = { [weak self] s, p in self?.revealPane(session: s, pane: p) }
        Bench.mark("config loaded")
        EditorLauncher.resolveFromLoginShell()
        NotificationCenter.default.addObserver(self, selector: #selector(settingsChanged), name: .alenoSettingsChanged, object: nil)
        rebuildMenus()

        guard DaemonClient.shared.bootstrap() else {
            let a = NSAlert()
            a.messageText = "Aleno could not start its session daemon"
            a.informativeText = "Expected aleno-daemon next to the application executable."
            a.runModal()
            NSApp.terminate(nil)
            return
        }
        Bench.mark("daemon connected")
        NSApp.servicesProvider = self      // Finder right-click services (see NSServices in Info.plist)
        NSUpdateDynamicServices()
        if !openedFromURLs { newWindow() }
        Bench.mark("window shown")
        NSApp.activate(ignoringOtherApps: true)
        if ProcessInfo.processInfo.environment["ALENO_TRACE_STARTUP"] != nil {
            NSLog("Aleno: startup to first window %.1f ms", Date().timeIntervalSince(started) * 1000)
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { newWindow() }
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if !controllers.isEmpty {
            let alert = NSAlert()
            alert.messageText = "Are you sure you want to quit Aleno?"
            alert.informativeText = "Quitting closes all terminal windows and ends the shells and commands running in them."
            alert.addButton(withTitle: "Quit")
            alert.addButton(withTitle: "Cancel").keyEquivalent = "\u{1b}"
            if alert.runModal() != .alertFirstButtonReturn { return .terminateCancel }
        }
        return .terminateNow
    }

    func applicationWillTerminate(_ notification: Notification) {
        DaemonClient.shared.shutdown()
    }

    // MARK: Windows

    var current: WindowController? {
        if let w = NSApp.keyWindow, let c = controllers.first(where: { $0.window === w }) { return c }
        return controllers.last
    }

    @discardableResult
    func openWindow(session: UInt32, adopting tab: Tab? = nil, cwd: String = "") -> WindowController {
        let c = WindowController(sessionId: session, adopting: tab, cwd: cwd)
        c.onProgress = { [weak self] _ in self?.updateDockProgress() }
        c.onTearOff = { [weak self] tab, from, point in
            guard let self else { return }
            let n = self.openWindow(session: from.sessionId, adopting: tab)
            let size = from.window.frame.size
            n.window.setFrame(NSRect(x: point.x - size.width / 2, y: point.y - size.height + 24, width: size.width, height: size.height), display: true)
        }
        c.onClose = { [weak self] c in self?.controllers.removeAll { $0 === c }; self?.updateDockProgress() }
        controllers.append(c)
        if controllers.count > 1, let last = controllers.dropLast().last?.window {
            // Cascade new windows.
            let f = last.frame
            c.window.setFrameTopLeftPoint(NSPoint(x: f.minX + 28, y: f.maxY - 28))
        } else {
            c.window.center()
        }
        c.window.makeKeyAndOrderFront(nil)
        return c
    }

    // MARK: Dock progress

    /// Shows the strongest progress of any window on the Dock icon (like a Safari download).
    private func updateDockProgress() {
        let best = controllers.map { $0.overallProgress }.max { a, b in
            func rank(_ p: (state: Int, percent: Int)) -> Int { p.state == 2 ? 4000 + p.percent : p.state == 1 ? 3000 + p.percent : p.state == 4 ? 2000 + p.percent : p.state == 3 ? 1000 : 0 }
            return rank(a) < rank(b)
        } ?? (0, 0)
        DockProgress.shared.set(best)
    }

    // MARK: Open from Finder (Open With, Services, Dock drop)

    private var openedFromURLs = false

    /// Folders open at themselves; a file opens in its folder.
    private func directory(for url: URL) -> String {
        var isDir: ObjCBool = false
        FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir)
        return isDir.boolValue ? url.path : url.deletingLastPathComponent().path
    }

    func openDirectories(_ paths: [String], asTabs: Bool) {
        for (i, path) in paths.enumerated() {
            if asTabs, let c = current {
                c.newTab(cwd: path)
                c.window.makeKeyAndOrderFront(nil)
            } else {
                guard let id = DaemonClient.shared.createSession(name: "") else { NSSound.beep(); continue }
                openWindow(session: id, cwd: path)
            }
            _ = i
        }
        NSApp.activate(ignoringOtherApps: true)
    }

    func application(_ app: NSApplication, open urls: [URL]) {
        openedFromURLs = true
        let paths = urls.filter { $0.isFileURL }.map { directory(for: $0) }
        guard !paths.isEmpty else { return }
        if DaemonClient.shared.bootstrap() { openDirectories(Array(NSOrderedSet(array: paths)) as? [String] ?? paths, asTabs: false) }
    }

    private func servicePaths(_ pboard: NSPasteboard) -> [String] {
        var urls = pboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if urls.isEmpty, let names = pboard.propertyList(forType: NSPasteboard.PasteboardType("NSFilenamesPboardType")) as? [String] {
            urls = names.map { URL(fileURLWithPath: $0) }
        }
        if urls.isEmpty, let text = pboard.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines), text.hasPrefix("/") || text.hasPrefix("~") {
            urls = [URL(fileURLWithPath: (text as NSString).expandingTildeInPath)]   // selected text that is a path
        }
        return urls.map { directory(for: $0) }
    }
    @objc func openWindowHere(_ pboard: NSPasteboard, userData: String, error: AutoreleasingUnsafeMutablePointer<NSString>) {
        openDirectories(servicePaths(pboard), asTabs: false)
    }
    @objc func openTabHere(_ pboard: NSPasteboard, userData: String, error: AutoreleasingUnsafeMutablePointer<NSString>) {
        openDirectories(servicePaths(pboard), asTabs: true)
    }

    @objc func showAbout() { AboutWindowController.shared.show() }

    @objc func newWindow() {
        if Bench.on { Bench.windowStart = CACurrentMediaTime() }
        guard let id = DaemonClient.shared.createSession(name: "") else { NSSound.beep(); return }
        openWindow(session: id)
    }

    // MARK: Menus

    @objc private func settingsChanged() {
        rebuildMenus()
        for c in controllers { c.settingsChanged() }
    }

    private struct Item { let title: String; let action: String; let selector: Selector }

    private func add(_ menu: NSMenu, _ title: String, _ action: String, _ sel: Selector, target: AnyObject? = nil) {
        let item = NSMenuItem(title: title, action: sel, keyEquivalent: "")
        item.target = target ?? self
        item.representedObject = action
        if let sc = ConfigStore.shared.settings.shortcut(for: action), let chord = KeyChord.parse(sc) {
            item.keyEquivalent = chord.keyEquivalent
            var mods = chord.modifiers
            // Uppercase letters imply shift for NSMenuItem.
            if chord.key.count == 1, chord.key.first!.isLetter, mods.contains(.shift) { item.keyEquivalent = chord.key.uppercased(); mods.remove(.shift) }
            item.keyEquivalentModifierMask = mods
        }
        menu.addItem(item)
    }

    private func rebuildMenus() {
        let main = NSMenu()
        func submenu(_ title: String, _ build: (NSMenu) -> Void) {
            let item = NSMenuItem(); main.addItem(item)
            let m = NSMenu(title: title); build(m); item.submenu = m
        }
        submenu("Aleno") { m in
            m.addItem(withTitle: "About Aleno", action: #selector(showAbout), keyEquivalent: "")
            m.addItem(.separator())
            add(m, "Settings…", "openSettings", #selector(openSettings))
            m.addItem(.separator())
            m.addItem(withTitle: "Hide Aleno", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
            let h = m.addItem(withTitle: "Hide Others", action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h")
            h.keyEquivalentModifierMask = [.command, .option]
            m.addItem(withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
            m.addItem(.separator())
            m.addItem(withTitle: "Quit Aleno", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        }
        submenu("Shell") { m in
            add(m, "New Window", "newWindow", #selector(newWindowAction))
            add(m, "New Tab", "newTab", #selector(newTab))
            m.addItem(.separator())
            add(m, "Split Right", "splitRight", #selector(splitRight))
            add(m, "Split Down", "splitDown", #selector(splitDown))
            m.addItem(.separator())
            add(m, "Close Pane", "closePane", #selector(closePane))
            add(m, "Close Tab", "closeTab", #selector(closeTab))
            add(m, "Reopen Closed Tab", "reopenClosedTab", #selector(reopenClosedTab))
        }
        submenu("Edit") { m in
            m.addItem(withTitle: "Copy", action: #selector(TerminalView.copy(_:)), keyEquivalent: "c")
            m.addItem(withTitle: "Paste", action: #selector(TerminalView.paste(_:)), keyEquivalent: "v")
            m.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
            m.addItem(.separator())
            add(m, "Find…", "find", #selector(find))
            add(m, "Find Next", "findNext", #selector(findNext))
            add(m, "Find Previous", "findPrevious", #selector(findPrevious))
            m.addItem(.separator())
            add(m, "Clear Screen", "clearScrollback", #selector(clearScreen))
            m.addItem(.separator())
            add(m, "Select Last Command Output", "selectLastOutput", #selector(selectLastOutput))
            add(m, "Copy Last Command Output", "copyLastOutput", #selector(copyLastOutput))
        }
        submenu("View") { m in
            add(m, "Increase Font Size", "increaseFontSize", #selector(fontUp))
            add(m, "Decrease Font Size", "decreaseFontSize", #selector(fontDown))
            add(m, "Reset Font Size", "resetFontSize", #selector(fontReset))
            m.addItem(.separator())
            add(m, "Scroll Page Up", "scrollPageUp", #selector(pageUp))
            add(m, "Scroll Page Down", "scrollPageDown", #selector(pageDown))
            add(m, "Scroll to Top", "scrollToTop", #selector(toTop))
            add(m, "Scroll to Bottom", "scrollToBottom", #selector(toBottom))
            m.addItem(.separator())
            add(m, "Toggle Full Screen", "toggleFullScreen", #selector(fullScreen))
        }
        submenu("Pane") { m in
            add(m, "Focus Pane Left", "focusLeft", #selector(focusLeft))
            add(m, "Focus Pane Right", "focusRight", #selector(focusRight))
            add(m, "Focus Pane Up", "focusUp", #selector(focusUp))
            add(m, "Focus Pane Down", "focusDown", #selector(focusDown))
            m.addItem(.separator())
            add(m, "Resize Pane Left", "resizeLeft", #selector(resizeLeft))
            add(m, "Resize Pane Right", "resizeRight", #selector(resizeRight))
            add(m, "Resize Pane Up", "resizeUp", #selector(resizeUp))
            add(m, "Resize Pane Down", "resizeDown", #selector(resizeDown))
        }
        submenu("Tab") { m in
            add(m, "Next Tab", "nextTab", #selector(nextTab))
            add(m, "Previous Tab", "previousTab", #selector(previousTab))
            m.addItem(.separator())
            for n in 1...9 {
                let it = m.addItem(withTitle: "Tab \(n)", action: #selector(selectTabNumber(_:)), keyEquivalent: "\(n)")
                it.tag = n; it.target = self
            }
        }
        submenu("Window") { m in
            m.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m")
            m.addItem(withTitle: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: "")
            NSApp.windowsMenu = m
        }
        NSApp.mainMenu = main
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(TerminalView.copy(_:)), #selector(TerminalView.paste(_:)): return current?.focusedView != nil
        default: return true
        }
    }

    // MARK: Actions

    func revealPane(session: UInt32, pane: UInt32) {
        NSApp.activate(ignoringOtherApps: true)
        if let c = controllers.first(where: { $0.sessionId == session }) { c.reveal(pane: pane) }
    }

    @objc func newWindowAction() { newWindow() }
    @objc func newTab() { if let c = current { c.newTab() } else { newWindow() } }
    @objc func splitRight() { current?.split(sideBySide: true) }
    @objc func splitDown() { current?.split(sideBySide: false) }
    @objc func closePane() { current?.closeFocusedPane() }
    @objc func closeTab() { current?.closeCurrentTab() }
    @objc func reopenClosedTab() { current?.reopenClosedTab() }
    @objc func nextTab() { current?.selectNext() }
    @objc func previousTab() { current?.selectPrevious() }
    @objc func selectTabNumber(_ s: NSMenuItem) { current?.selectTab(number: s.tag) }
    @objc func focusLeft() { current?.focus(.left) }
    @objc func focusRight() { current?.focus(.right) }
    @objc func focusUp() { current?.focus(.up) }
    @objc func focusDown() { current?.focus(.down) }
    @objc func resizeLeft() { current?.resizeFocused(.left) }
    @objc func resizeRight() { current?.resizeFocused(.right) }
    @objc func resizeUp() { current?.resizeFocused(.up) }
    @objc func resizeDown() { current?.resizeFocused(.down) }
    @objc func find() { current?.showFind() }
    @objc func findNext() { current?.focusedView?.stepMatch(1) }
    @objc func findPrevious() { current?.focusedView?.stepMatch(-1) }
    @objc func clearScreen() { current?.focusedView?.clearScrollbackAndScreen() }
    @objc func fontUp() { current?.changeFont(delta: 1) }
    @objc func fontDown() { current?.changeFont(delta: -1) }
    @objc func fontReset() { current?.changeFont(delta: nil) }
    @objc func pageUp() { current?.focusedView?.scrollPages(1) }
    @objc func pageDown() { current?.focusedView?.scrollPages(-1) }
    @objc func selectLastOutput() { current?.focusedView?.selectLastOutput() }
    @objc func copyLastOutput() { current?.focusedView?.copyLastOutput() }
    @objc func toTop() { current?.focusedView?.scrollToTop() }
    @objc func toBottom() { current?.focusedView?.scrollToBottom() }
    @objc func fullScreen() { current?.window.toggleFullScreen(nil) }
    /// ⌘, — open settings.json in its own TextEdit window. Saving (⌘S) live-reloads the configuration.
    @objc func openSettings() {
        let store = ConfigStore.shared
        store.ensureFileExists()
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        p.arguments = ["-a", "TextEdit", store.fileURL.path]
        if (try? p.run()) == nil { openWithDefault(store.fileURL) }
    }

    private func openWithDefault(_ url: URL) {
        // `open -t` uses the default macOS text editor.
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        p.arguments = ["-t", url.path]
        if (try? p.run()) == nil { NSWorkspace.shared.open(url) }
    }
}
