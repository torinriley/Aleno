import AppKit

final class RootView: NSView {
    var fill: NSColor = .windowBackgroundColor { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { fill.setFill(); bounds.fill(using: .copy) }
}

/// One macOS window == one daemon session. Owns tabs, split trees and the tab bar.
final class WindowController: NSObject, NSWindowDelegate, TerminalViewDelegate, TabBarDelegate {
    let window: NSWindow
    let sessionId: UInt32
    private(set) var tabs: [Tab] = []
    private(set) var activeIndex = 0

    private let container = NSView()
    private let root = RootView()
    private let blur = NSVisualEffectView()
    private var glass: NSView?   // Liquid Glass backdrop (macOS 26+), used instead of `blur` when appearance.blurStyle is "glass"
    private let tabBar = TabBarView()
    private let workspace = WorkspaceView()
    private let toast = ToastView()
    private let sizeBadge = SizeBadge()
    private var fontDelta: Double = 0
    private var closing = false

    var onClose: ((WindowController) -> Void)?
    var onProgress: ((WindowController) -> Void)?
    var onTearOff: ((Tab, WindowController, NSPoint) -> Void)?
    /// Windows sharing a daemon session (a torn-off tab keeps its panes in the original session).
    private static var liveWindows: [UInt32: Int] = [:]

    private var settings: Settings {
        var s = ConfigStore.shared.settings
        s.font.size = min(72, max(6, s.font.size + fontDelta))
        return s
    }

    init(sessionId: UInt32, frame: NSRect? = nil, adopting adopted: Tab? = nil, cwd: String = "", welcome: Bool = false) {
        self.sessionId = sessionId
        WindowController.liveWindows[sessionId, default: 0] += 1
        let style: NSWindow.StyleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window = NSWindow(contentRect: frame ?? NSRect(x: 0, y: 0, width: 900, height: 600), styleMask: style, backing: .buffered, defer: false)
        super.init()
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.tabbingMode = .disallowed
        window.setFrameAutosaveName("AlenoWindow")
        window.minSize = NSSize(width: 240, height: 120)
        window.contentView = container
        blur.material = .underWindowBackground
        blur.blendingMode = .behindWindow
        blur.state = .active
        blur.autoresizingMask = [.width, .height]
        blur.isHidden = true
        blur.frame = container.bounds
        container.addSubview(blur)
        root.frame = container.bounds
        root.autoresizingMask = [.width, .height]
        container.addSubview(root)   // tint layer: one surface for title bar, tabs and terminal
        root.addSubview(workspace)
        root.addSubview(tabBar)
        root.addSubview(toast)
        root.addSubview(sizeBadge)
        tabBar.delegate = self
        // Like Ghostty, re-apply the window blur whenever the window becomes visible or key (it has no effect before the window is on screen).
        for n in [NSWindow.didChangeOcclusionStateNotification, NSWindow.didBecomeKeyNotification, NSWindow.didBecomeMainNotification, NSWindow.didChangeScreenNotification] {
            NotificationCenter.default.addObserver(self, selector: #selector(refreshBackdrop), name: n, object: window)
        }
        NotificationCenter.default.addObserver(self, selector: #selector(fullScreenChanged), name: NSWindow.willEnterFullScreenNotification, object: window)
        NotificationCenter.default.addObserver(self, selector: #selector(fullScreenChanged), name: NSWindow.didExitFullScreenNotification, object: window)
        NotificationCenter.default.addObserver(self, selector: #selector(diagnostics(_:)), name: .alenoConfigDiagnostics, object: nil)

        if let adopted {
            tabs = [adopted]
            for l in adopted.leaves { l.view.terminalDelegate = self }
            select(0)
        } else if tabs.isEmpty { newTab(cwd: cwd, welcome: welcome) }
        applyChrome()
        layoutChrome()
        if let e = ConfigStore.shared.lastError { toast.show("settings.json has errors — keeping the previous configuration\n\(e)", sticky: true) }
    }

    deinit { NotificationCenter.default.removeObserver(self) }

    // MARK: Layout of chrome

    /// Single-tab windows show only the title row; the tab strip appears with a second tab.
    private var chromeHeight: CGFloat { tabs.count > 1 ? TabBarView.totalHeight : TabBarView.titleRowHeight }

    private var insetTimer: Timer?
    private var animatedChromeHeight: CGFloat?

    private func updateChrome(animated: Bool) {
        let multi = tabs.count > 1
        tabBar.setTabsVisible(multi, animated: animated)
        let b = root.bounds
        let target = chromeHeight
        let from = workspace.chromeInset
        workspace.frame = NSRect(x: 0, y: 0, width: b.width, height: b.height)
        if insetTimer?.isValid == true, animatedChromeHeight == target { return }
        insetTimer?.invalidate()
        animatedChromeHeight = target
        guard animated, from != target else {
            tabBar.frame = NSRect(x: 0, y: 0, width: b.width, height: target)
            workspace.chromeInset = target
            return
        }
        tabBar.frame = NSRect(x: 0, y: 0, width: b.width, height: max(from, target))
        let start = CACurrentMediaTime(), dur = 0.28
        insetTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { [weak self] t in
            guard let self else { t.invalidate(); return }
            let x = min(1, (CACurrentMediaTime() - start) / dur)
            let e = CGFloat(x < 0.5 ? 2 * x * x : 1 - pow(-2 * x + 2, 2) / 2)   // ease in-out
            self.workspace.chromeInset = from + (target - from) * e
            if x >= 1 {
                t.invalidate()
                self.tabBar.frame = NSRect(x: 0, y: 0, width: self.root.bounds.width, height: target)
            }
        }
    }

    private func layoutChrome() {
        let b = root.bounds
        let h = workspace.chromeInset > 0 ? max(workspace.chromeInset, chromeHeight) : chromeHeight
        tabBar.frame = NSRect(x: 0, y: 0, width: b.width, height: h)
        workspace.frame = NSRect(x: 0, y: 0, width: b.width, height: b.height)
        if insetTimer?.isValid != true { workspace.chromeInset = chromeHeight }
        let tw = min(560, b.width - 40)
        toast.frame = NSRect(x: (b.width - tw) / 2, y: b.height - 90, width: tw, height: 70)
        toast.autoresizingMask = [.minXMargin, .maxXMargin, .minYMargin]
        tabBar.autoresizingMask = [.width]
        workspace.autoresizingMask = [.width, .height]
    }

    func windowDidResize(_ notification: Notification) { layoutChrome() }

    @objc private func refreshBackdrop() {
        applyChrome()
    }

    @objc private func fullScreenChanged(_ n: Notification) {
        tabBar.leftInset = n.name == NSWindow.willEnterFullScreenNotification ? 12 : 78
    }

    @objc private func diagnostics(_ n: Notification) {
        let errors = n.userInfo?["errors"] as? [String] ?? []
        let warnings = n.userInfo?["warnings"] as? [String] ?? []
        if !errors.isEmpty {
            toast.show("settings.json is invalid — keeping the previous configuration\n" + errors.joined(separator: "\n"), sticky: true)
        } else if !warnings.isEmpty {
            toast.show("settings.json: " + warnings.joined(separator: "\n"), sticky: false)
        } else { toast.dismiss() }
    }

    // MARK: Settings

    func settingsChanged() {
        applyChrome()
        let s = settings
        for t in tabs { for l in t.leaves { l.view.apply(settings: s) } }
    }

    // Same call Ghostty uses for background-blur: an adjustable window-server blur behind the window.
    private static let blurSetter: ((Int, Int32) -> Void)? = {
        guard let conn = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGSDefaultConnectionForThread"),
              let set = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGSSetWindowBackgroundBlurRadius") else { return nil }
        typealias ConnFn = @convention(c) () -> UnsafeMutableRawPointer
        typealias SetFn = @convention(c) (UnsafeMutableRawPointer, Int, Int32) -> Int32
        let c = unsafeBitCast(conn, to: ConnFn.self), f = unsafeBitCast(set, to: SetFn.self)
        return { w, r in _ = f(c(), w, r) }
    }()

    private static func setBackgroundBlur(_ w: NSWindow, radius: Double) {
        guard w.isVisible, w.windowNumber > 0 else { return }   // no effect on a window that isn't on screen yet
        blurSetter?(w.windowNumber, Int32(radius.rounded()))
    }

    /// Like Ghostty, hide the system title bar's background and separator views. AppKit can bring them back when
    /// tabs change, so this is re-applied then (and a beat later, after AppKit's own update).
    private func hideTitlebarBackground() {
        func hide(_ v: NSView) {
            let name = String(describing: type(of: v))
            if name == "NSTitlebarBackgroundView" || name == "_NSTitlebarDecorationView" { v.isHidden = true; return }
            v.subviews.forEach(hide)
        }
        guard let frame = window.contentView?.superview else { return }
        for v in frame.subviews where String(describing: type(of: v)) == "NSTitlebarContainerView" { hide(v) }
    }

    private func applyChrome() {
        hideTitlebarBackground()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) { [weak self] in self?.hideTitlebarBackground() }
        let s = settings
        let dark = TerminalView.isDark(settings: s, appearance: NSApp.effectiveAppearance)
        window.appearance = s.appearance.theme == "system" ? nil : NSAppearance(named: s.appearance.theme == "dark" ? .darkAqua : .aqua)
        let theme = Theme.make(settings: s, dark: s.appearance.theme == "system" ? dark : s.appearance.theme == "dark")
        let bg = NSColor(srgbRed: CGFloat((theme.background >> 16) & 0xFF) / 255, green: CGFloat((theme.background >> 8) & 0xFF) / 255,
                         blue: CGFloat(theme.background & 0xFF) / 255, alpha: CGFloat(s.appearance.opacity))
        root.fill = bg
        window.isOpaque = s.appearance.opacity >= 1
        blur.frame = container.bounds
        let wantsBlur = s.appearance.opacity < 1 && s.appearance.blur
        var useGlass = false
        if #available(macOS 26.0, *), s.appearance.blurStyle == "glass" {
            useGlass = true
            let g = (glass as? NSGlassEffectView) ?? NSGlassEffectView()
            if glass == nil {
                g.cornerRadius = (window.value(forKey: "_cornerRadius") as? CGFloat) ?? 0
                g.autoresizingMask = [.width, .height]
                container.addSubview(g, positioned: .below, relativeTo: root)
                glass = g
            }
            g.frame = container.bounds
            g.isHidden = !wantsBlur
        }
        if !useGlass { glass?.removeFromSuperview(); glass = nil }
        // A set blurRadius uses the adjustable window-server blur instead of the fixed frosted material.
        let radius = (!useGlass && wantsBlur) ? s.appearance.blurRadius : nil
        blur.isHidden = !wantsBlur || useGlass || radius != nil
        WindowController.setBackgroundBlur(window, radius: radius ?? 0)
        window.backgroundColor = s.appearance.opacity >= 1 ? bg : NSColor.white.withAlphaComponent(0.001)   // not .clear: matches Terminal.app and keeps the window's shape for the blur
        tabBar.needsDisplay = true
    }

    // MARK: Pane / tab creation

    private func makeLeaf(cwd: String = "", frame: NSRect? = nil, inset: CGFloat? = nil, seed: String = "", welcome: Bool = false) -> LeafNode? {
        let s = settings
        let daemon = DaemonClient.shared
        // Use the same cell geometry as the renderer before the shell can emit a prompt.
        let scale = window.backingScaleFactor
        let paneFrame = frame ?? root.bounds
        let fonts = MetalContext.shared.atlas(for: s, scale: scale).fonts
        let size = PaneRenderer.gridSize(forPixels: CGSize(width: paneFrame.width * scale, height: paneFrame.height * scale),
                                         scale: scale, settings: s, fonts: fonts, topInset: inset ?? chromeHeight)
        func create() -> UInt32? {
            daemon.createPane(session: sessionId, cols: size.cols, rows: size.rows, shell: s.terminal.shell,
                              cwd: cwd, scrollback: s.terminal.scrollback, seed: seed, welcome: welcome)
        }
        guard let pane = create() else {
            let message = "Could not start a shell: \(daemon.lastError)"
            NSLog("Aleno: %@ (session %u)", message, sessionId)
            toast.show(message, sticky: true)
            return nil
        }
        return adopt(paneId: pane)
    }

    private func adopt(paneId: UInt32) -> LeafNode {
        let v = TerminalView(session: sessionId, pane: paneId, settings: settings)
        v.terminalDelegate = self
        return LeafNode(view: v)
    }

    @discardableResult
    func newTab(cwd: String? = nil, welcome: Bool = false) -> Tab? {
        let dir = cwd ?? tabs[safe: activeIndex]?.focused.view.cwdPath ?? ""
        let inset = tabs.isEmpty ? TabBarView.titleRowHeight : TabBarView.totalHeight
        guard let leaf = makeLeaf(cwd: dir, inset: inset, welcome: welcome) else { return nil }
        let t = Tab(root: leaf, focused: leaf)
        tabs.append(t)
        select(tabs.count - 1)
        return t
    }

    func select(_ i: Int) {
        guard tabs.indices.contains(i) else { return }
        for l in tabs[activeIndex.clamped(to: tabs.indices)].leaves { l.view.isHidden = true }
        activeIndex = i
        let t = tabs[i]
        workspace.gridChromeInset = chromeHeight
        workspace.tab = t
        for l in t.leaves { l.view.isHidden = false }
        refreshTabBar()
        DispatchQueue.main.async { [weak self] in
            guard let self, self.tabs[safe: self.activeIndex] === t else { return }
            self.focusLeaf(t.focused)
        }
    }

    private func focusLeaf(_ l: LeafNode) {
        guard let t = tabs[safe: activeIndex] else { return }
        if t.leaves.contains(where: { $0 === l }) { t.focused = l }
        window.makeFirstResponder(l.view)
        refreshTabBar()
    }

    /// Bring a pane into view (used when a notification is clicked).
    func reveal(pane: UInt32) {
        window.makeKeyAndOrderFront(nil)
        for (i, t) in tabs.enumerated() {
            if let l = t.leaves.first(where: { $0.view.paneId == pane }) {
                if i != activeIndex { select(i) }
                t.focused = l
                window.makeFirstResponder(l.view)
                return
            }
        }
    }

    func selectNext() { if tabs.count > 1 { select((activeIndex + 1) % tabs.count) } }
    func selectPrevious() { if tabs.count > 1 { select((activeIndex - 1 + tabs.count) % tabs.count) } }
    func selectTab(number n: Int) { if n >= 1 { select(min(n, tabs.count) - 1) } }

    private func refreshTabBar() {
        hideTitlebarBackground()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) { [weak self] in self?.hideTitlebarBackground() }
        tabBar.titles = tabs.map { $0.title }
        refreshProgress()
        updateChrome(animated: window.isVisible)
        tabBar.active = activeIndex
        let t = tabs[safe: activeIndex]?.title ?? ""
        window.title = t.isEmpty ? "Aleno" : t
        tabBar.windowTitle = t
    }

    // MARK: Splits

    func split(sideBySide: Bool) {
        guard let t = tabs[safe: activeIndex] else { return }
        let cur = t.focused
        var newFrame = cur.view.frame
        if sideBySide {
            let leftWidth = floor((newFrame.width - 1) * 0.5)
            newFrame.origin.x += leftWidth + 1
            newFrame.size.width -= leftWidth + 1
        } else {
            let topHeight = floor((newFrame.height - 1) * 0.5)
            newFrame.origin.y += topHeight + 1
            newFrame.size.height -= topHeight + 1
        }
        guard let new = makeLeaf(cwd: cur.view.cwdPath, frame: newFrame,
                                 inset: newFrame.minY < 0.5 ? chromeHeight : 0) else { return }
        let parent = cur.parent
        let sp = SplitNode(sideBySide: sideBySide, ratio: 0.5, a: cur, b: new)
        if let p = parent {
            if p.a === cur { p.a = sp } else { p.b = sp }
            sp.parent = p
        } else {
            t.root = sp; sp.parent = nil
        }
        t.focused = new
        workspace.refreshStructure()
        workspace.layoutTree()
        focusLeaf(new)
    }

    func closeFocusedPane() { if let t = tabs[safe: activeIndex] { closePane(t.focused, tab: t) } }

    private func closePane(_ leaf: LeafNode, tab t: Tab) {
        if t.leaves.count == 1 { closeTab(tabs.firstIndex { $0 === t } ?? activeIndex); return }
        guard let p = leaf.parent else { return }
        let sibling = p.a === leaf ? p.b : p.a
        if let gp = p.parent {
            if gp.a === p { gp.a = sibling } else { gp.b = sibling }
        } else {
            t.root = sibling; sibling.parent = nil
        }
        DaemonClient.shared.killPane(session: sessionId, pane: leaf.view.paneId)
        leaf.view.shutdown()
        leaf.view.removeFromSuperview()
        if t.focused === leaf { t.focused = Tab.leaves(of: sibling).first! }
        workspace.refreshStructure()
        focusLeaf(t.focused)
    }

    // MARK: Reopen closed tab

    /// Recently closed tabs, newest last, shared by all windows.
    private static var closedTabs: [ClosedTab] = []

    private func capture(_ t: Tab) -> ClosedTab {
        let leaves = t.leaves
        let history = leaves.count > 4 ? 300 : 1500
        func node(_ n: PaneNode) -> ClosedNode {
            if let l = n as? LeafNode { return .leaf(cwd: l.view.cwdPath, seed: l.view.snapshotText(maxHistory: history) ?? "") }
            let s = n as! SplitNode
            return .split(sideBySide: s.sideBySide, ratio: s.ratio, a: node(s.a), b: node(s.b))
        }
        return ClosedTab(root: node(t.root), customTitle: t.customTitle, focusedIndex: leaves.firstIndex { $0 === t.focused } ?? 0)
    }

    private func remember(_ t: Tab) {
        WindowController.closedTabs.append(capture(t))
        if WindowController.closedTabs.count > 10 { WindowController.closedTabs.removeFirst() }
    }

    /// Brings back the most recently closed tab: same splits and folders, with its old screen and history above a fresh shell.
    @discardableResult
    func reopenClosedTab() -> Bool {
        guard let closed = WindowController.closedTabs.popLast() else { NSSound.beep(); return false }
        let inset = tabs.isEmpty ? TabBarView.titleRowHeight : TabBarView.totalHeight
        func build(_ n: ClosedNode) -> PaneNode? {
            switch n {
            case let .leaf(cwd, seed):
                return makeLeaf(cwd: cwd, inset: inset, seed: seed)
            case let .split(side, ratio, a, b):
                guard let na = build(a), let nb = build(b) else { return nil }
                return SplitNode(sideBySide: side, ratio: ratio, a: na, b: nb)
            }
        }
        guard let root = build(closed.root) else { return false }
        let leaves = Tab.leaves(of: root)
        let t = Tab(root: root, focused: leaves[min(closed.focusedIndex, leaves.count - 1)])
        t.customTitle = closed.customTitle
        tabs.append(t)
        select(tabs.count - 1)
        return true
    }

    func closeTab(_ i: Int) {
        guard tabs.indices.contains(i) else { return }
        let t = tabs[i]
        remember(t)
        for l in t.leaves {
            DaemonClient.shared.killPane(session: sessionId, pane: l.view.paneId)
            l.view.shutdown(); l.view.removeFromSuperview()
        }
        tabs.remove(at: i)
        if tabs.isEmpty { terminateSessionAndClose(); return }
        activeIndex = min(activeIndex, tabs.count - 1)
        if i < activeIndex { activeIndex -= 1 }
        workspace.tab = nil
        select(min(i, tabs.count - 1))
    }

    func closeCurrentTab() { closeTab(activeIndex) }

    // MARK: Focus & resize

    func focus(_ d: WorkspaceView.Direction) {
        guard let t = tabs[safe: activeIndex], let n = workspace.neighbor(of: t.focused, direction: d) else { return }
        focusLeaf(n)
    }

    func resizeFocused(_ d: WorkspaceView.Direction) {
        guard let t = tabs[safe: activeIndex] else { return }
        let wantSide = (d == .left || d == .right)
        var node: PaneNode = t.focused
        while let p = node.parent {
            if p.sideBySide == wantSide {
                let delta: CGFloat = 0.03
                p.ratio = min(0.92, max(0.08, p.ratio + ((d == .right || d == .down) ? delta : -delta)))
                workspace.layoutTree()
                return
            }
            node = p
        }
    }

    // MARK: Font zoom

    func changeFont(delta: Double?) {
        if let d = delta { fontDelta += d } else { fontDelta = 0 }
        settingsChanged()
    }

    // MARK: Commands on the focused pane

    var focusedView: TerminalView? { tabs[safe: activeIndex]?.focused.view }

    func showFind() { focusedView?.showFind() }

    /// Opens a command in a new tab by typing it into a fresh shell (used for terminal editors).
    func runInNewTab(command: String) {
        guard let t = newTab() else { return }
        t.focused.view.typeWhenReady("exec " + command + "\r")
    }

    func terminateSessionAndClose() {
        closing = true
        window.close()
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if closing { return true }
        if ConfigStore.shared.settings.workspace.confirmOnClose {
            let a = NSAlert()
            a.messageText = "Close this window?"
            a.informativeText = "Closing this window ends its shells and running commands."
            a.addButton(withTitle: "Close Window"); a.addButton(withTitle: "Cancel")
            if a.runModal() != .alertFirstButtonReturn { return false }
        }
        closing = true
        return true
    }

    func windowWillClose(_ notification: Notification) {
        closing = true
        let left = (WindowController.liveWindows[sessionId] ?? 1) - 1
        WindowController.liveWindows[sessionId] = left > 0 ? left : nil
        if left <= 0 { DaemonClient.shared.killSession(sessionId) }
        for t in tabs { remember(t) }   // closing a whole window can be undone tab by tab
        for t in tabs { for l in t.leaves { l.view.shutdown() } }
        onClose?(self)
    }

    // MARK: TerminalViewDelegate

    func terminalViewTitleChanged(_ v: TerminalView) { refreshTabBar() }

    /// Progress of the most noteworthy pane of a tab: a failure beats running work (highest percentage first),
    /// which beats paused, which beats indeterminate.
    static func progress(of leaves: [LeafNode]) -> (state: Int, percent: Int) {
        func rank(_ p: (state: Int, percent: Int)) -> Int {
            switch p.state {
            case 2: return 4000 + p.percent
            case 1: return 3000 + p.percent
            case 4: return 2000 + p.percent
            case 3: return 1000
            default: return 0
            }
        }
        return leaves.map { $0.view.progress }.max { rank($0) < rank($1) } ?? (0, 0)
    }

    /// Strongest progress across every tab in this window (for the Dock icon).
    var overallProgress: (state: Int, percent: Int) { WindowController.progress(of: tabs.flatMap { $0.leaves }) }

    func terminalViewProgressChanged(_ v: TerminalView) { refreshProgress() }

    private func refreshProgress() {
        tabBar.progress = tabs.map { WindowController.progress(of: $0.leaves) }
        onProgress?(self)
    }

    func terminalView(_ v: TerminalView, resizedToCols cols: Int, rows: Int) {
        guard window.isVisible, tabs[safe: activeIndex]?.leaves.contains(where: { $0.view === v }) == true else { return }
        sizeBadge.show("\(cols) × \(rows)", over: v.convert(v.bounds, to: root))
    }

    func terminalViewExited(_ v: TerminalView) {
        for t in tabs {
            if let leaf = t.leaves.first(where: { $0.view === v }) { closePane(leaf, tab: t); return }
        }
    }

    func terminalViewDidFocus(_ v: TerminalView) {
        for t in tabs { if let l = t.leaves.first(where: { $0.view === v }) { t.focused = l } }
        refreshTabBar()
    }

    func terminalViewRequestsClose(_ v: TerminalView) { terminalViewExited(v) }

    // MARK: TabBarDelegate

    func tabBarSelect(_ i: Int) { select(i) }
    func tabBarClose(_ i: Int) { closeTab(i) }
    func tabBarNewTab() { newTab() }
    // MARK: Tab tear-off (drag a tab out to make a window)

    private var tearTab: Tab?
    private var tearPanel: NSPanel?

    /// A miniature of the window as it will look once dropped: a title row over a snapshot of the tab's contents.
    private func tearPreview(title: String) -> NSImage? {
        let size = window.frame.size
        let scale: CGFloat = min(0.34, 340 / size.width)
        let w = (size.width * scale).rounded(), h = (size.height * scale).rounded()
        // The SDK marks CGWindowListCreateImage unavailable, but it still exists at runtime; fall back to a plain card without it.
        typealias Fn = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
        var content: CGImage?
        if let sym = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") {
            let fn = unsafeBitCast(sym, to: Fn.self)
            content = fn(.null, 8 /* optionIncludingWindow */, UInt32(window.windowNumber), 1 /* boundsIgnoreFraming */)?.takeRetainedValue()
        }
        let img = NSImage(size: NSSize(width: w, height: h), flipped: false) { rect in
            let path = NSBezierPath(roundedRect: rect, xRadius: 6, yRadius: 6)
            path.addClip()
            (self.root.fill ?? NSColor.windowBackgroundColor).withAlphaComponent(1).setFill()
            rect.fill()
            let titleH = TabBarView.titleRowHeight * scale
            if let c = content, let ctx = NSGraphicsContext.current?.cgContext {
                // Real title row (traffic lights, icon, title) from the window itself, then the pane area; the tab strip is left out.
                let px = CGFloat(c.width) / size.width
                let top = TabBarView.titleRowHeight * px, skip = TabBarView.totalHeight * px
                let W = CGFloat(c.width), H = CGFloat(c.height)
                if let head = c.cropping(to: CGRect(x: 0, y: 0, width: W, height: top)),
                   let body = c.cropping(to: CGRect(x: 0, y: skip, width: W, height: H - skip)) {
                    ctx.interpolationQuality = .high
                    ctx.draw(head, in: CGRect(x: 0, y: h - titleH, width: w, height: titleH))
                    ctx.draw(body, in: CGRect(x: 0, y: 0, width: w, height: h - titleH))
                }
            } else {
                for (i, col) in [NSColor.systemRed, .systemYellow, .systemGreen].enumerated() {
                    col.setFill()
                    NSBezierPath(ovalIn: NSRect(x: 5 + CGFloat(i) * 7, y: h - titleH / 2 - 2.5, width: 5, height: 5)).fill()
                }
                let t = NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: 7, weight: .semibold), .foregroundColor: NSColor.white.withAlphaComponent(0.8)])
                t.draw(at: NSPoint(x: 40, y: h - titleH / 2 - t.size().height / 2))
            }
            NSColor.white.withAlphaComponent(0.18).setStroke()
            path.lineWidth = 1; path.stroke()
            return true
        }
        return img
    }

    func tabBarTearBegin(_ i: Int, at screenPoint: NSPoint) {
        guard tabs.count > 1, tabs.indices.contains(i) else { return }
        let t = tabs[i]
        let image = tearPreview(title: t.title)
        let panel = NSPanel(contentRect: NSRect(origin: .zero, size: image?.size ?? NSSize(width: 300, height: 200)),
                            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isOpaque = false; panel.backgroundColor = .clear; panel.hasShadow = true
        panel.level = .floating; panel.ignoresMouseEvents = true; panel.isReleasedWhenClosed = false
        let iv = NSImageView(frame: NSRect(origin: .zero, size: panel.frame.size))
        iv.image = image
        panel.contentView = iv
        tearPanel = panel
        workspace.tab = nil
        for l in t.leaves { l.view.removeFromSuperview() }
        tabs.remove(at: i)
        tearTab = t
        activeIndex = min(activeIndex, tabs.count - 1)
        select(min(i, tabs.count - 1))
        tabBarTearMove(at: screenPoint)
        panel.orderFront(nil)
    }

    func tabBarTearMove(at p: NSPoint) {
        guard let panel = tearPanel else { return }
        panel.setFrameOrigin(NSPoint(x: p.x - panel.frame.width / 2, y: p.y - panel.frame.height + 12))
    }

    func tabBarTearEnd(at p: NSPoint) {
        guard let t = tearTab else { return }
        tearTab = nil
        tearPanel?.orderOut(nil); tearPanel = nil
        // Dropped back on this window's own tab strip: put the tab back.
        let strip = NSRect(x: window.frame.minX, y: window.frame.maxY - TabBarView.totalHeight - 20, width: window.frame.width, height: TabBarView.totalHeight + 20)
        if strip.contains(p) {
            tabs.append(t)
            select(tabs.count - 1)
            return
        }
        onTearOff?(t, self, p)
    }

    func tabBarMove(from: Int, to: Int) {
        guard tabs.indices.contains(from), tabs.indices.contains(to) else { return }
        let t = tabs.remove(at: from)
        tabs.insert(t, at: to)
        activeIndex = tabs.firstIndex { $0 === workspace.tab } ?? to
        refreshTabBar()
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
extension Int {
    func clamped(to r: Range<Int>) -> Int { Swift.min(Swift.max(self, r.lowerBound), Swift.max(r.lowerBound, r.upperBound - 1)) }
}
extension TerminalView {
    /// Working directory reported by the shell via OSC 7 (file://host/path), if any.
    var cwdPath: String {
        let c = cwd
        guard !c.isEmpty else { return "" }
        if let u = URL(string: c), u.isFileURL { return u.path }
        return c.hasPrefix("/") ? c : ""
    }
}
