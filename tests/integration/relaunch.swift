import AppKit

@main
struct RelaunchRegression {
    static func main() {
        _ = NSApplication.shared
        let daemon = DaemonClient.shared
        precondition(daemon.bootstrap())
        let session = daemon.createSession(name: "window-lifecycle")!
        let recovered = WindowController(sessionId: session)
        precondition(recovered.tabs.count == 1, "New window has no shell")
        // Inspect the daemon before pumping the AppKit run loop: this is the
        // PTY's spawn size, not a later resize performed by view attachment.
        func paneSizes() -> [[String: Any]] {
            let control = aleno_ctl_connect(daemon.socketPath)!
            defer { aleno_ctl_free(control) }
            let json = aleno_ctl_list_panes_json(control, session)!
            defer { aleno_string_free(json) }
            return try! JSONSerialization.jsonObject(with: Data(String(cString: json).utf8)) as! [[String: Any]]
        }
        let panes = paneSizes()
        recovered.window.contentView?.layoutSubtreeIfNeeded()
        let view = recovered.tabs[0].focused.view
        let scale = recovered.window.backingScaleFactor
        let settings = ConfigStore.shared.settings
        let fonts = MetalContext.shared.atlas(for: settings, scale: scale).fonts
        let expected = PaneRenderer.gridSize(forPixels: CGSize(width: view.bounds.width * scale, height: view.bounds.height * scale),
                                             scale: scale, settings: settings, fonts: fonts, topInset: view.topInset)
        precondition(panes[0]["cols"] as? Int == expected.cols, "PTY started at the wrong column count")
        precondition(panes[0]["rows"] as? Int == expected.rows, "PTY started at the wrong row count")
        print("PASS: initial PTY size matches laid-out pane (\(expected.cols)×\(expected.rows))")
        let pane = recovered.tabs[0].focused.view.paneId
        let added = recovered.newTab()!.focused.view
        let addedSize = paneSizes().first { $0["id"] as? Int == Int(added.paneId) }!
        let tabGrid = PaneRenderer.gridSize(forPixels: CGSize(width: view.bounds.width * scale, height: view.bounds.height * scale),
                                            scale: scale, settings: settings, fonts: fonts, topInset: TabBarView.totalHeight)
        precondition(addedSize["rows"] as? Int == tabGrid.rows, "Second tab spawned with single-tab height")
        precondition(added.gridTopInset == TabBarView.totalHeight)
        recovered.closeCurrentTab()
        precondition(recovered.tabs[0].focused.view.paneId == pane)
        precondition(view.gridTopInset == TabBarView.titleRowHeight)
        print("PASS: opening and closing a second tab applies final grid sizes")
        recovered.window.close()
        precondition(!sessionExists(daemon, session), "Closing the window left its session alive")
        let socket = daemon.socketPath
        daemon.shutdown()
        let deadline = Date().addingTimeInterval(4)
        while FileManager.default.fileExists(atPath: socket), Date() < deadline {
            Thread.sleep(forTimeInterval: 0.02)
        }
        precondition(!FileManager.default.fileExists(atPath: socket), "Helper outlived Aleno")
        precondition(daemon.bootstrap())
        defer { daemon.shutdown() }
        precondition(daemon.socketPath != socket)
        precondition(!sessionExists(daemon, session), "A new launch restored a session")
        print("PASS: closing a window ends its session, and relaunch starts fresh")
    }

    static func sessionExists(_ daemon: DaemonClient, _ session: UInt32) -> Bool {
        guard let control = aleno_ctl_connect(daemon.socketPath) else { preconditionFailure("Helper unavailable") }
        defer { aleno_ctl_free(control) }
        guard let json = aleno_ctl_list_panes_json(control, session) else { return false }
        aleno_string_free(json)
        return true
    }
}
