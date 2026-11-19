import AppKit
import MetalKit

protocol TerminalViewDelegate: AnyObject {
    func terminalViewTitleChanged(_ v: TerminalView)
    func terminalViewExited(_ v: TerminalView)
    func terminalViewDidFocus(_ v: TerminalView)
    func terminalViewRequestsClose(_ v: TerminalView)
    func terminalViewProgressChanged(_ v: TerminalView)
    func terminalView(_ v: TerminalView, resizedToCols cols: Int, rows: Int)
}

/// Retained until the pane's last event (DISCONNECTED) so the Rust reader thread never sees a dangling pointer.
final class PaneEventBox { weak var view: TerminalView? }

private let paneNotify: AlenoNotify = { ud, ev in
    guard let ud else { return }
    let box = Unmanaged<PaneEventBox>.fromOpaque(ud).takeUnretainedValue()
    DispatchQueue.main.async { box.view?.handlePaneEvent(ev) }
    if ev == UInt32(ALENO_EVENT_DISCONNECTED) { Unmanaged<PaneEventBox>.fromOpaque(ud).release() }
}

struct GridPos: Equatable, Comparable {
    var row: Int   // absolute line index
    var col: Int
    static func < (a: GridPos, b: GridPos) -> Bool { (a.row, a.col) < (b.row, b.col) }
}

final class TerminalView: MTKView, MTKViewDelegate, NSTextInputClient {
    let sessionId: UInt32
    let paneId: UInt32
    weak var terminalDelegate: TerminalViewDelegate?

    private(set) var stream: OpaquePointer?
    private var box: PaneEventBox?
    private var renderer: PaneRenderer
    private(set) var settings: Settings
    private var theme: Theme
    private var attachedSize: (Int, Int) = (0, 0)
    private var exited = false
    private var reattachAttempts = 0
    private var pendingInitialInput: [UInt8]?

    // Selection (absolute rows)
    private var selAnchor: GridPos?
    private var selHead: GridPos?
    private var selMode = 0   // 0 char, 1 word, 2 line
    private var selWordAnchor: (GridPos, GridPos)?
    private var lastEvicted: UInt64 = 0

    // Search
    private var matches: [(row: Int, c0: Int, c1: Int)] = []
    private var matchesByRow: [Int: [(c0: Int, c1: Int, idx: Int)]] = [:]
    private var currentMatch = -1
    private var findBar: FindBar?
    private var findTopConstraint: NSLayoutConstraint?
    private var didAnimateFind = false

    // Cursor blink / focus
    private var cursorOn = true
    private var blinkTimer: Timer?
    private var isFocused: Bool { window?.isKeyWindow == true && window?.firstResponder === self }

    // IME
    private var markedText = NSMutableAttributedString()
    private var markedLabel: NSTextField?

    private var scrollAccum: CGFloat = 0

    // Line caret: a compositor-driven layer, so blinking costs the app no CPU (no redraws while idle).
    private let caretLayer = CALayer()
    private var caretBlinking = false
    private var caretSolidUntil: CFTimeInterval = 0

    // Command blocks (OSC 133): failure bars in the left margin and a status badge under each noteworthy command.
    private var barLayers: [CALayer] = []
    private var badgeLayers: [CATextLayer] = []
    private var badgeHits: [(rect: CGRect, output: Int, end: Int)] = []

    /// Title bar / tab strip height covering the top of this view; the grid starts below it.
    var gridTopInset: CGFloat = 0 {
        didSet { if oldValue != gridTopInset { relayout(force: false) } }
    }

    var topInset: CGFloat = 0 {
        didSet {
            guard oldValue != topInset else { return }
            findTopConstraint?.constant = topInset + 8
            renderer.topInsetPoints = topInset
            renderer.markAllDirty()
            needsDisplay = true
        }
    }

    var title: String {
        guard let s = stream, let p = aleno_pane_title(s) else { return "" }
        defer { aleno_string_free(p) }
        return String(cString: p)
    }

    var cwd: String {
        guard let s = stream, let p = aleno_pane_cwd(s) else { return "" }
        defer { aleno_string_free(p) }
        return String(cString: p)
    }

    init(session: UInt32, pane: UInt32, settings: Settings) {
        self.sessionId = session
        self.paneId = pane
        self.settings = settings
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        let dark = TerminalView.isDark(settings: settings, appearance: NSApp.effectiveAppearance)
        theme = Theme.make(settings: settings, dark: dark)
        renderer = PaneRenderer(settings: settings, theme: theme, scale: scale)
        super.init(frame: .zero, device: MetalContext.shared.device)
        colorPixelFormat = .bgra8Unorm
        isPaused = true
        enableSetNeedsDisplay = true       // event-driven: draw only when something changed
        framebufferOnly = true
        autoResizeDrawable = true
        delegate = self
        layerContentsRedrawPolicy = .onSetNeedsDisplay
        (layer as? CAMetalLayer)?.isOpaque = false
        (layer as? CAMetalLayer)?.maximumDrawableCount = 2     // lower input-to-photon latency than the default 3
        caretLayer.isHidden = true
        caretLayer.actions = ["position": NSNull(), "bounds": NSNull(), "hidden": NSNull(), "backgroundColor": NSNull()]
        layer?.addSublayer(caretLayer)
        registerForDraggedTypes([.fileURL, .string])
        renderer.highlightProvider = { [weak self] row, col in self?.highlightFlags(row: row, col: col) ?? 0 }
        applyBlinkTimer()
        NotificationCenter.default.addObserver(self, selector: #selector(windowKeyChanged), name: NSWindow.didBecomeKeyNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(windowKeyChanged), name: NSWindow.didResignKeyNotification, object: nil)
    }

    required init(coder: NSCoder) { fatalError() }

    deinit {
        detachStream()
        blinkTimer?.invalidate()
        NotificationCenter.default.removeObserver(self)
    }

    static func isDark(settings: Settings, appearance: NSAppearance) -> Bool {
        switch settings.appearance.theme {
        case "dark": return true
        case "light": return false
        default: return appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        }
    }

    // MARK: Connection

    private func attachIfPossible() {
        guard stream == nil, !exited, window != nil else { return }
        let scale = window?.backingScaleFactor ?? 2
        let px = CGSize(width: bounds.width * scale, height: bounds.height * scale)
        guard px.width > 8, px.height > 8 else { return }
        let (cols, rows) = renderer.gridSize(forPixels: px, scale: scale, topInset: gridTopInset)
        let b = PaneEventBox(); b.view = self
        box = b
        let ud = Unmanaged.passRetained(b).toOpaque()
        stream = aleno_pane_attach(DaemonClient.shared.socketPath, sessionId, paneId, UInt16(cols), UInt16(rows), UInt32(settings.terminal.scrollback), paneNotify, ud)
        if stream == nil {
            Unmanaged<PaneEventBox>.fromOpaque(ud).release()
            box = nil
            scheduleReattach()
            return
        }
        attachedSize = (cols, rows)
        reattachAttempts = 0
        renderer.markAllDirty()
        if let data = pendingInitialInput { pendingInitialInput = nil; send(data) }
        needsDisplay = true
    }

    private func detachStream() {
        if let s = stream {
            stream = nil
            box?.view = nil
            aleno_pane_free(s)
        }
        box = nil
    }

    private func scheduleReattach() {
        guard !exited else { return }
        reattachAttempts += 1
        // Never close the pane (which would kill the shell) just because attaching keeps failing: keep trying,
        // slowly. The user can close the pane or tab explicitly.
        DispatchQueue.main.asyncAfter(deadline: .now() + min(0.25 * Double(reattachAttempts), 5)) { [weak self] in
            self?.attachIfPossible()
        }
    }

    /// Text to type into the shell as soon as the stream is attached (e.g. "nvim settings.json\r").
    func typeWhenReady(_ text: String) {
        let bytes = Array(text.utf8)
        if stream != nil { send(bytes) } else { pendingInitialInput = bytes }
    }

    /// Progress the program reports about itself (OSC 9;4, e.g. brew, cargo): state 0 none, 1 normal, 2 error, 3 indeterminate, 4 paused.
    private(set) var progress: (state: Int, percent: Int) = (0, 0)

    private func updateProgress() {
        guard let s = stream else { return }
        var st: UInt32 = 0, pc: UInt32 = 0
        aleno_pane_progress(s, &st, &pc)
        let next = (state: Int(st), percent: Int(pc))
        guard next != progress else { return }
        progress = next
        terminalDelegate?.terminalViewProgressChanged(self)
    }

    func handlePaneEvent(_ ev: UInt32) {
        switch Int(ev) {
        case Int(ALENO_EVENT_OUTPUT): needsDisplay = true
        case Int(ALENO_EVENT_TITLE): terminalDelegate?.terminalViewTitleChanged(self)
        case Int(ALENO_EVENT_EXITED):
            exited = true
            if progress.state != 0 { progress = (0, 0); terminalDelegate?.terminalViewProgressChanged(self) }
            terminalDelegate?.terminalViewExited(self)
        case Int(ALENO_EVENT_BELL): NSApp.requestUserAttention(.informationalRequest)
        case Int(ALENO_EVENT_COMMAND_FINISHED): handleFinishedCommands()
        case Int(ALENO_EVENT_PROGRESS): updateProgress()
        case Int(ALENO_EVENT_DISCONNECTED):
            // Transient failure: drop the stale stream and re-attach (the daemon sends a fresh snapshot).
            if let s = stream, aleno_pane_is_exited(s) == 0 {
                detachStream()
                scheduleReattach()
            }
        default: break
        }
    }

    // MARK: Settings & appearance

    func apply(settings new: Settings) {
        let old = settings
        settings = new
        theme = Theme.make(settings: new, dark: TerminalView.isDark(settings: new, appearance: effectiveAppearance))
        let scale = window?.backingScaleFactor ?? 2
        renderer.reconfigure(settings: new, theme: theme, scale: scale)
        (layer as? CAMetalLayer)?.isOpaque = false
        if let s = stream, old.terminal.scrollback != new.terminal.scrollback { aleno_pane_set_scrollback_limit(s, UInt32(new.terminal.scrollback)) }
        applyBlinkTimer()
        relayout(force: true)
        needsDisplay = true
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        theme = Theme.make(settings: settings, dark: TerminalView.isDark(settings: settings, appearance: effectiveAppearance))
        renderer.reconfigure(settings: settings, theme: theme, scale: window?.backingScaleFactor ?? 2)
        needsDisplay = true
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        renderer.reconfigure(settings: settings, theme: theme, scale: window?.backingScaleFactor ?? 2)
        relayout(force: true)
        needsDisplay = true
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil {
            renderer.reconfigure(settings: settings, theme: theme, scale: window!.backingScaleFactor)
            // Attaching here used the placeholder view size before WorkspaceView
            // placed the pane, provoking an immediate shell prompt redraw.
            DispatchQueue.main.async { [weak self] in self?.attachIfPossible() }
        }
    }

    @objc private func windowKeyChanged(_ n: Notification) {
        guard (n.object as? NSWindow) === window else { return }
        sendFocusEvent()
        cursorOn = true
        needsDisplay = true
    }

    /// Like a native text caret: stays solid while you type or move, then resumes blinking.
    private func keepCaretSolid() {
        guard settings.terminal.cursorBlink else { return }
        if caretBlinking { caretLayer.removeAnimation(forKey: "blink"); caretBlinking = false }
        if !cursorOn { cursorOn = true; needsDisplay = true }
        caretSolidUntil = CACurrentMediaTime() + 0.5
        let mark = caretSolidUntil
        // Always re-arm: a later keystroke moves `caretSolidUntil`, which turns this resume into a no-op.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.52) { [weak self] in
            guard let self, self.caretSolidUntil == mark else { return }
            self.resumeCaretBlink()
        }
        applyBlinkTimer(delay: 0.5)
    }

    private func applyBlinkTimer(delay: TimeInterval = 0) {
        blinkTimer?.invalidate(); blinkTimer = nil
        cursorOn = true
        if settings.terminal.cursorBlink && settings.terminal.cursorStyle != "bar" {
            let t = Timer(fire: Date().addingTimeInterval(0.53 + delay), interval: 0.53, repeats: true) { [weak self] _ in
                guard let self, self.isFocused else { return }
                self.cursorOn.toggle(); self.needsDisplay = true
            }
            RunLoop.main.add(t, forMode: .common)
            blinkTimer = t
        }
    }

    // MARK: Layout

    override func layout() {
        super.layout()
        relayout(force: false)
    }

    private func relayout(force: Bool) {
        guard window != nil else { return }
        if stream == nil { attachIfPossible(); return }
        guard let s = stream else { return }
        let scale = window?.backingScaleFactor ?? 2
        let px = CGSize(width: bounds.width * scale, height: bounds.height * scale)
        guard px.width > 8, px.height > 8 else { return }
        let (cols, rows) = renderer.gridSize(forPixels: px, scale: scale, topInset: gridTopInset)
        if force || cols != attachedSize.0 || rows != attachedSize.1 {
            // Lines re-wrap when the width changes, so row-based selection / search results no longer point at the same text.
            if cols != attachedSize.0 && attachedSize.0 != 0 { clearSelection(); matches = []; matchesByRow = [:] }
            let wasSized = attachedSize.0 != 0
            attachedSize = (cols, rows)
            if wasSized { terminalDelegate?.terminalView(self, resizedToCols: cols, rows: rows) }
            aleno_pane_resize(s, UInt16(cols), UInt16(rows))
            renderer.markAllDirty()
            needsDisplay = true
        }
    }

    // MARK: MTKViewDelegate

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) { }

    func draw(in view: MTKView) {
        guard let s = stream else {
            clearOnly(); return
        }
        let benchStart = CACurrentMediaTime()
        renderer.sync(pane: s)
        let benchSync = CACurrentMediaTime()
        let info = renderer.info
        if info.evicted != lastEvicted {
            let d = Int(info.evicted &- lastEvicted)
            lastEvicted = info.evicted
            if selAnchor != nil { selAnchor!.row -= d; selHead!.row -= d; if selAnchor!.row < 0 && selHead!.row < 0 { clearSelection() } }
        }
        var style: UInt32
        switch (settings.terminal.cursorStyle, info.cursor_style) {
        case (_, 1), (_, 2): style = 1
        case (_, 3), (_, 4): style = 4
        case (_, 5), (_, 6): style = 3
        case ("bar", _): style = 3
        case ("underline", _): style = 4
        default: style = 1
        }
        if !isFocused && style == 1 { style = 2 }
        if style == 3, info.cursor_visible != 0 {
            CATransaction.begin(); CATransaction.setDisableActions(true)
            updateCaretLayer(info: info)
            CATransaction.commit()
            style = 0                       // the caret layer draws it; the shader must not
        } else {
            if !caretLayer.isHidden { caretLayer.isHidden = true; caretLayer.removeAnimation(forKey: "blink"); caretBlinking = false }
            if settings.terminal.cursorBlink && isFocused && !cursorOn { style = 0 }
        }
        CATransaction.begin(); CATransaction.setDisableActions(true)
        updateBlockMarks(info: info, pane: s)
        CATransaction.commit()
        renderer.draw(in: self, cursorMode: style, scale: window?.backingScaleFactor ?? 2)
        if Bench.on { Bench.firstFrame(); Bench.recordCPU((CACurrentMediaTime() - benchStart) * 1000, syncMs: (benchSync - benchStart) * 1000, rebuiltRows: renderer.rebuiltRows) }
    }

    /// The pane's screen and recent history as replayable output (for reopening a closed tab); nil when unavailable.
    func snapshotText(maxHistory: Int) -> String? {
        guard let s = stream, let p = aleno_pane_snapshot(s, UInt32(maxHistory)) else { return nil }
        defer { aleno_string_free(p) }
        return String(cString: p)
    }

    // MARK: Command blocks

    private func updateBlockMarks(info: AlenoViewInfo, pane s: OpaquePointer) {
        var blocks = [AlenoBlock](repeating: AlenoBlock(), count: 48)
        var n = 0
        if info.modes & (1 << 8) == 0 {      // not on the alternate screen
            n = aleno_pane_blocks(s, info.top_abs, info.top_abs + info.rows, &blocks, blocks.count)
        }
        badgeHits.removeAll(keepingCapacity: true)
        var bars = 0, badges = 0
        let scale = window?.backingScaleFactor ?? 2
        let ch = CGFloat(renderer.fonts.cellHeight) / scale
        let pad = CGFloat(settings.appearance.padding)
        let top = Int(info.top_abs), rows = Int(info.rows)
        func rowY(_ r: Int) -> CGFloat { bounds.height - pad - topInset - CGFloat(r + 1) * ch }
        for b in blocks.prefix(n) {
            let failed = b.exit_code != Int32.min && b.exit_code != 0
            let finished = b.end_row >= 0
            guard finished, b.output_row >= 0 else { continue }
            let first = max(Int(b.prompt_row) - top, 0), last = min(Int(b.end_row) - top, rows)   // [first, last)
            if failed, last > first {
                if bars == barLayers.count {
                    let l = CALayer(); l.cornerRadius = 1.25; layer?.addSublayer(l); barLayers.append(l)
                }
                let l = barLayers[bars]; bars += 1
                l.backgroundColor = NSColor.systemRed.withAlphaComponent(0.85).cgColor
                l.frame = CGRect(x: 1, y: rowY(last - 1) + 1, width: 2.5, height: CGFloat(last - first) * ch - 2)
                l.isHidden = false
            }
            // Badge on the last output line: failures and slow commands only.
            let slow = b.dur_ms >= 3000
            let badgeRow = Int(b.end_row) - 1
            guard (failed || slow), badgeRow >= Int(b.output_row), badgeRow - top >= 0, badgeRow - top < rows else { continue }
            var text = failed ? "\u{2717} \(b.exit_code)" : "\u{2713}"
            if b.dur_ms >= 1000 { text += " \u{00B7} " + TerminalView.formatDuration(Int(b.dur_ms)) }
            if badges == badgeLayers.count {
                let l = CATextLayer()
                l.contentsScale = scale; l.alignmentMode = .center; l.cornerRadius = 4; l.masksToBounds = true
                layer?.addSublayer(l); badgeLayers.append(l)
            }
            let l = badgeLayers[badges]; badges += 1
            let size = max(9, ch * 0.62)
            let font = NSFont.monospacedSystemFont(ofSize: size, weight: .medium)
            let w = (text as NSString).size(withAttributes: [.font: font]).width + 12
            let h = ch - 4
            l.string = text
            l.font = font; l.fontSize = size
            l.foregroundColor = (failed ? NSColor.systemRed : NSColor(srgbRed: 0.55, green: 0.55, blue: 0.58, alpha: 1)).cgColor
            let bg = theme.background
            l.backgroundColor = NSColor(srgbRed: CGFloat((bg >> 16) & 0xFF) / 255, green: CGFloat((bg >> 8) & 0xFF) / 255, blue: CGFloat(bg & 0xFF) / 255, alpha: 0.88).cgColor
            let rect = CGRect(x: bounds.width - pad - w, y: rowY(badgeRow - top) + 2, width: w, height: h)
            l.frame = rect
            l.isHidden = false
            badgeHits.append((rect, Int(b.output_row), Int(b.end_row)))
        }
        for i in bars..<barLayers.count { barLayers[i].isHidden = true }
        for i in badges..<badgeLayers.count { badgeLayers[i].isHidden = true }
    }

    static func formatDuration(_ ms: Int) -> String {
        if ms < 10_000 { return String(format: "%.1fs", Double(ms) / 1000) }
        let s = ms / 1000
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m \(s % 60)s" }
        return "\(s / 3600)h \((s % 3600) / 60)m"
    }

    private func outputText(rows o: Int, end e: Int) -> String? {
        guard let s = stream, e > o else { return nil }
        guard let c = aleno_pane_text(s, UInt32(o), 0, UInt32(e - 1), UInt32.max) else { return nil }
        defer { aleno_string_free(c) }
        return String(cString: c)
    }

    /// Output of the most recent finished command, with trailing blank lines removed.
    func lastOutput() -> String? {
        guard let s = stream else { return nil }
        var b = AlenoBlock()
        guard aleno_pane_last_block(s, 1, &b) == 1, b.output_row >= 0, b.end_row > b.output_row else { return nil }
        guard var t = outputText(rows: Int(b.output_row), end: Int(b.end_row)) else { return nil }
        while t.hasSuffix("\n") { t.removeLast() }
        return t
    }

    func copyLastOutput() {
        guard let t = lastOutput(), !t.isEmpty else { NSSound.beep(); return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(t, forType: .string)
    }

    /// Highlight the most recent command's output as the selection (⌘C then copies it).
    func selectLastOutput() {
        guard let s = stream else { return }
        var b = AlenoBlock()
        guard aleno_pane_last_block(s, 1, &b) == 1, b.end_row > b.output_row else { NSSound.beep(); return }
        selAnchor = GridPos(row: Int(b.output_row), col: 0)
        selHead = GridPos(row: Int(b.end_row) - 1, col: Int(renderer.info.cols) - 1)
        selMode = 0
        markSelectionDirty(); needsDisplay = true
    }

    private func resumeCaretBlink() {
        guard !caretLayer.isHidden, settings.terminal.cursorBlink, isFocused, !caretBlinking else { return }
        // Skip if the user typed again within the grace period (a newer resume is already scheduled).
        guard CACurrentMediaTime() >= caretSolidUntil else { return }
        startCaretBlink()
    }

    private func startCaretBlink() {
        let a = CAKeyframeAnimation(keyPath: "opacity")
        a.values = [1.0, 1.0, 0.0, 0.0]
        a.keyTimes = [0, 0.5, 0.5, 1]
        a.calculationMode = .discrete
        a.duration = 1.06
        a.repeatCount = .infinity
        caretLayer.add(a, forKey: "blink")
        caretBlinking = true
    }

    private func updateCaretLayer(info: AlenoViewInfo) {
        let scale = window?.backingScaleFactor ?? 2
        let f = renderer.fonts
        let pad = CGFloat(settings.appearance.padding)
        let cw = CGFloat(f.cellWidth) / scale, ch = CGFloat(f.cellHeight) / scale
        var col = Int(info.cursor_col)
        if let cell = renderer.cellAt(row: Int(info.cursor_row), col: col), cell.flags & 512 != 0, col > 0 { col -= 1 }
        let x = pad + CGFloat(col) * cw
        let y = bounds.height - pad - topInset - CGFloat(info.cursor_row + 1) * ch
        caretLayer.frame = CGRect(x: x, y: y, width: 1.5, height: ch)
        caretLayer.backgroundColor = NSColor(srgbRed: CGFloat((theme.cursor >> 16) & 0xFF) / 255, green: CGFloat((theme.cursor >> 8) & 0xFF) / 255,
                                             blue: CGFloat(theme.cursor & 0xFF) / 255, alpha: 1).cgColor
        caretLayer.isHidden = false
        let focused = isFocused
        caretLayer.opacity = focused ? 1 : 0.5
        if focused && settings.terminal.cursorBlink && !caretBlinking && CACurrentMediaTime() >= caretSolidUntil { startCaretBlink() }
        if (!focused || !settings.terminal.cursorBlink) && caretBlinking { caretLayer.removeAnimation(forKey: "blink"); caretBlinking = false }
    }

    private func clearOnly() {
        guard let rpd = currentRenderPassDescriptor, let d = currentDrawable, let cb = MetalContext.shared.queue.makeCommandBuffer() else { return }
        rpd.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        rpd.colorAttachments[0].loadAction = .clear
        cb.makeRenderCommandEncoder(descriptor: rpd)?.endEncoding()
        cb.present(d); cb.commit()
    }

    // MARK: Input

    func send(_ bytes: [UInt8]) {
        guard let s = stream, !bytes.isEmpty else { return }
        keepCaretSolid()
        bytes.withUnsafeBufferPointer { aleno_pane_input(s, $0.baseAddress, bytes.count) }
    }
    func send(_ str: String) { send(Array(str.utf8)) }

    private var modes: UInt32 { renderer.info.modes }

    private func snapToBottomOnInput() {
        if renderer.info.display_offset != 0, let s = stream { aleno_pane_set_display_offset(s, 0); needsDisplay = true }
    }

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func becomeFirstResponder() -> Bool {
        terminalDelegate?.terminalViewDidFocus(self)
        sendFocusEvent(true)
        cursorOn = true
        needsDisplay = true
        return true
    }
    override func resignFirstResponder() -> Bool {
        sendFocusEvent(false)
        needsDisplay = true
        return true
    }
    private func sendFocusEvent(_ focus: Bool? = nil) {
        guard modes & KeyEncoder.focusEvents != 0 else { return }
        let f = focus ?? isFocused
        send(f ? "\u{1b}[I" : "\u{1b}[O")
    }

    override func keyDown(with event: NSEvent) {
        let hasMarked = markedText.length > 0
        if !hasMarked, let bytes = KeyEncoder.special(event: event, modes: modes, optionAsMeta: settings.terminal.optionAsMeta) {
            snapToBottomOnInput()
            clearSelection()
            send(bytes)
            return
        }
        if event.modifierFlags.contains(.command) { return }  // unclaimed command shortcuts are not terminal input
        interpretKeyEvents([event])
    }

    // Menu-bar shortcut keys are resolved by AppKit before keyDown; swallow those that are not.
    override func performKeyEquivalent(with event: NSEvent) -> Bool { false }

    override func doCommand(by selector: Selector) { /* special keys are handled in keyDown */ }

    // MARK: NSTextInputClient

    func insertText(_ string: Any, replacementRange: NSRange) {
        let s = (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
        unmark()
        guard !s.isEmpty else { return }
        snapToBottomOnInput()
        clearSelection()
        send(s.replacingOccurrences(of: "\n", with: "\r"))
    }
    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let a = (string as? NSAttributedString) ?? NSAttributedString(string: (string as? String) ?? "")
        markedText = NSMutableAttributedString(attributedString: a)
        showMarked()
    }
    func unmarkText() { unmark() }
    private func unmark() { markedText = NSMutableAttributedString(); markedLabel?.removeFromSuperview(); markedLabel = nil }
    func selectedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
    func markedRange() -> NSRange { markedText.length > 0 ? NSRange(location: 0, length: markedText.length) : NSRange(location: NSNotFound, length: 0) }
    func hasMarkedText() -> Bool { markedText.length > 0 }
    func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? { nil }
    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [.underlineStyle] }
    func characterIndex(for point: NSPoint) -> Int { 0 }
    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        let r = cursorRectInView()
        guard let w = window else { return r }
        return w.convertToScreen(convert(r, to: nil))
    }

    private func cursorRectInView() -> NSRect {
        let scale = window?.backingScaleFactor ?? 2
        let f = renderer.fonts
        let pad = CGFloat(settings.appearance.padding)
        let cw = CGFloat(f.cellWidth) / scale, ch = CGFloat(f.cellHeight) / scale
        let x = pad + CGFloat(renderer.info.cursor_col) * cw
        let y = bounds.height - pad - topInset - CGFloat(renderer.info.cursor_row + 1) * ch   // view is not flipped
        return NSRect(x: x, y: y, width: cw, height: ch)
    }

    private func showMarked() {
        if markedLabel == nil {
            let l = NSTextField(labelWithString: "")
            l.drawsBackground = true
            l.backgroundColor = NSColor.textBackgroundColor
            l.isBordered = false
            addSubview(l)
            markedLabel = l
        }
        markedLabel?.font = NSFont.monospacedSystemFont(ofSize: CGFloat(settings.font.size), weight: .regular)
        markedLabel?.attributedStringValue = markedText
        markedLabel?.sizeToFit()
        let r = cursorRectInView()
        markedLabel?.setFrameOrigin(NSPoint(x: r.minX, y: r.minY))
    }

    // MARK: Clipboard

    @objc func copy(_ sender: Any?) {
        guard let t = selectedText(), !t.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(t, forType: .string)
    }

    @objc func paste(_ sender: Any?) {
        guard let s = NSPasteboard.general.string(forType: .string), !s.isEmpty else { return }
        pasteText(s)
    }

    func pasteText(_ text: String) {
        snapToBottomOnInput()
        var t = text.replacingOccurrences(of: "\r\n", with: "\r").replacingOccurrences(of: "\n", with: "\r")
        if modes & KeyEncoder.bracketedPaste != 0 {
            // A paste must not be able to terminate bracketed-paste mode early.
            t = t.replacingOccurrences(of: "\u{1b}[201~", with: "")
            send("\u{1b}[200~" + t + "\u{1b}[201~")
        } else {
            send(t)
        }
    }

    override func selectAll(_ sender: Any?) {
        let i = renderer.info
        selAnchor = GridPos(row: 0, col: 0)
        selHead = GridPos(row: Int(i.scrollback_len) + Int(i.rows) - 1, col: Int(i.cols) - 1)
        markSelectionDirty(); needsDisplay = true
    }

    func selectedText() -> String? {
        guard let a = selAnchor, let h = selHead, let s = stream else { return nil }
        let (p, q) = a <= h ? (a, h) : (h, a)
        guard let c = aleno_pane_text(s, UInt32(max(0, p.row)), UInt32(p.col), UInt32(max(0, q.row)), UInt32(q.col)) else { return nil }
        defer { aleno_string_free(c) }
        return String(cString: c)
    }

    // MARK: Mouse

    /// Absolute line under viewport row `r` (a blank filler row counts as the first visible line).
    private func mouseLine(_ r: Int) -> Int {
        let l = renderer.line(forRow: r)
        return l >= 0 ? l : Int(renderer.info.top_abs)
    }

    private func cellPos(for event: NSEvent) -> (col: Int, row: Int) { cellPos(windowPoint: event.locationInWindow) }

    private func cellPos(windowPoint: NSPoint) -> (col: Int, row: Int) {
        let p = convert(windowPoint, from: nil)
        let scale = window?.backingScaleFactor ?? 2
        let f = renderer.fonts
        let pad = CGFloat(settings.appearance.padding)
        let x = (p.x - pad) * scale / CGFloat(f.cellWidth)
        let y = (bounds.height - p.y - pad - topInset) * scale / CGFloat(f.cellHeight)
        let i = renderer.info
        return (min(max(Int(floor(x)), 0), Int(i.cols) - 1), min(max(Int(floor(y)), 0), Int(i.rows) - 1))
    }

    private var mouseReporting: Bool { modes & (KeyEncoder.mousePress | KeyEncoder.mouseDrag | KeyEncoder.mouseMotion) != 0 }
    private var pressedButton: Int? = nil
    private var caretDrag = false
    private var virtualCol = 0

    private func caretLineEligible(row: Int) -> Bool {
        modes & KeyEncoder.altScreen == 0 && renderer.info.display_offset == 0 && row == Int(renderer.info.cursor_row)
    }

    /// Moves the shell's caret horizontally on the current line by sending arrow keys.
    private func moveCaret(to col: Int) {
        let delta = col - virtualCol
        guard delta != 0 else { return }
        let seq = delta > 0 ? (modes & KeyEncoder.appCursor != 0 ? "\u{1b}OC" : "\u{1b}[C") : (modes & KeyEncoder.appCursor != 0 ? "\u{1b}OD" : "\u{1b}[D")
        send(String(repeating: seq, count: min(abs(delta), 500)))
        virtualCol = col
    }

    private func report(_ event: NSEvent, button: Int, press: Bool, motion: Bool) {
        let (c, r) = cellPos(for: event)
        if let b = KeyEncoder.mouse(button: button, col: c + 1, row: r + 1, press: press, motion: motion, flags: event.modifierFlags, modes: modes) { send(b) }
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        if mouseReporting && !event.modifierFlags.contains(.shift) {
            pressedButton = 0; report(event, button: 0, press: true, motion: false); return
        }
        let local = convert(event.locationInWindow, from: nil)
        if let hit = badgeHits.first(where: { $0.rect.contains(local) }), let t = outputText(rows: hit.output, end: hit.end) {
            var text = t
            while text.hasSuffix("\n") { text.removeLast() }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
            return
        }
        let (c, r) = cellPos(for: event)
        let pos = GridPos(row: mouseLine(r), col: c)
        // Click or drag on the line holding the caret: the caret follows the pointer (like a text field).
        caretDrag = false
        if !event.modifierFlags.contains(.shift), !event.modifierFlags.contains(.command), event.clickCount == 1,
           caretLineEligible(row: r) {
            caretDrag = true
            virtualCol = Int(renderer.info.cursor_col)
            moveCaret(to: c)
            markSelectionDirty()
            selAnchor = pos; selHead = pos; selMode = 0; selWordAnchor = nil
            markSelectionDirty(); needsDisplay = true
            return
        }
        if event.modifierFlags.contains(.command), let link = hyperlink(at: r, col: c) {
            NSWorkspace.shared.open(link); return
        }
        if event.modifierFlags.contains(.command), let path = existingPath(at: r, col: c) {
            openPath(path); return
        }
        markSelectionDirty()
        if event.modifierFlags.contains(.shift), selAnchor != nil {
            selHead = pos
        } else {
            switch event.clickCount {
            case 2:
                let (a, b) = wordBounds(row: r, col: c)
                selAnchor = GridPos(row: pos.row, col: a); selHead = GridPos(row: pos.row, col: b)
                selMode = 1; selWordAnchor = (selAnchor!, selHead!)
            case 3:
                selAnchor = GridPos(row: pos.row, col: 0); selHead = GridPos(row: pos.row, col: Int(renderer.info.cols) - 1)
                selMode = 2; selWordAnchor = (selAnchor!, selHead!)
            default:
                selAnchor = pos; selHead = pos; selMode = 0; selWordAnchor = nil
            }
        }
        markSelectionDirty(); needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        if mouseReporting && !event.modifierFlags.contains(.shift) {
            if modes & (KeyEncoder.mouseDrag | KeyEncoder.mouseMotion) != 0 { report(event, button: pressedButton ?? 0, press: true, motion: true) }
            return
        }
        guard selAnchor != nil else { return }
        if caretDrag {
            let (c, r) = cellPos(for: event)
            _ = r
            markSelectionDirty()
            selHead = GridPos(row: selAnchor!.row, col: c)
            moveCaret(to: c)
            markSelectionDirty(); needsDisplay = true
            return
        }
        let p = convert(event.locationInWindow, from: nil)
        // Autoscroll while dragging beyond the edges.
        if let s = stream {
            if p.y > bounds.height { aleno_pane_scroll(s, 1) } else if p.y < 0 { aleno_pane_scroll(s, -1) }
        }
        let (c, r) = cellPos(for: event)
        var head = GridPos(row: mouseLine(r), col: c)
        if selMode != 0, let (wa, wb) = selWordAnchor {
            // Extend by whole words / lines.
            let (a, b) = selMode == 1 ? wordBounds(row: r, col: c) : (0, Int(renderer.info.cols) - 1)
            if head < wa { head = GridPos(row: head.row, col: a); selAnchor = wb } else { head = GridPos(row: head.row, col: b); selAnchor = wa }
        }
        markSelectionDirty()
        selHead = head
        markSelectionDirty(); needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        if mouseReporting && !event.modifierFlags.contains(.shift) {
            report(event, button: 0, press: false, motion: false); pressedButton = nil; return
        }
        caretDrag = false
        if selMode == 0, selAnchor == selHead { clearSelection() }
    }

    override func rightMouseDown(with event: NSEvent) {
        if mouseReporting && !event.modifierFlags.contains(.shift) { report(event, button: 2, press: true, motion: false); pressedButton = 2; return }
        super.rightMouseDown(with: event)
    }
    override func rightMouseUp(with event: NSEvent) {
        if mouseReporting && !event.modifierFlags.contains(.shift) { report(event, button: 2, press: false, motion: false); pressedButton = nil }
    }
    override func otherMouseDown(with event: NSEvent) {
        if mouseReporting && !event.modifierFlags.contains(.shift) { report(event, button: 1, press: true, motion: false); pressedButton = 1 }
        else if event.buttonNumber == 2, let s = NSPasteboard.general.string(forType: .string) { pasteText(s) }
    }
    override func otherMouseUp(with event: NSEvent) {
        if mouseReporting && !event.modifierFlags.contains(.shift) { report(event, button: 1, press: false, motion: false); pressedButton = nil }
    }
    override func mouseMoved(with event: NSEvent) {
        if modes & KeyEncoder.mouseMotion != 0 && !event.modifierFlags.contains(.shift) { report(event, button: 3, press: true, motion: true) }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for a in trackingAreas { removeTrackingArea(a) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil))
    }

    override func scrollWheel(with event: NSEvent) {
        guard let s = stream else { return }
        let f = renderer.fonts
        let scale = window?.backingScaleFactor ?? 2
        let lineH = CGFloat(f.cellHeight) / scale
        var dy = event.scrollingDeltaY
        if event.hasPreciseScrollingDeltas { dy /= lineH } else { dy *= 3 }
        scrollAccum += dy
        let lines = Int(scrollAccum.rounded(.towardZero))
        guard lines != 0 else { return }
        scrollAccum -= CGFloat(lines)
        if mouseReporting && !event.modifierFlags.contains(.shift) {
            for _ in 0..<min(abs(lines), 20) { report(event, button: lines > 0 ? 64 : 65, press: true, motion: false) }
        } else if modes & KeyEncoder.altScreen != 0 && modes & KeyEncoder.altScroll != 0 {
            let seq = lines > 0 ? (modes & KeyEncoder.appCursor != 0 ? "\u{1b}OA" : "\u{1b}[A") : (modes & KeyEncoder.appCursor != 0 ? "\u{1b}OB" : "\u{1b}[B")
            for _ in 0..<min(abs(lines), 20) { send(seq) }
        } else {
            aleno_pane_scroll(s, Int32(lines))
            needsDisplay = true
        }
    }

    // MARK: Words & links

    private func wordBounds(row: Int, col: Int) -> (Int, Int) {
        let i = renderer.info
        let cols = Int(i.cols)
        func isWord(_ c: Int) -> Bool {
            guard let ch = renderer.cellChar(row: row, col: c) else { return false }
            if ch & 0x8000_0000 != 0 { return true }
            guard let u = UnicodeScalar(ch) else { return false }
            return CharacterSet.alphanumerics.contains(u) || "_-./~:@%+=#?&".unicodeScalars.contains(u)
        }
        if !isWord(col) { return (col, col) }
        var a = col, b = col
        while a > 0, isWord(a - 1) { a -= 1 }
        while b < cols - 1, isWord(b + 1) { b += 1 }
        return (a, b)
    }

    private func hyperlink(at row: Int, col: Int) -> URL? {
        // Prompt themes (Powerlevel10k) wrap the directory in a hyperlink; the prompt is never clickable.
        guard !inPromptZone(row: row) else { return nil }
        guard let s = stream, let id = renderer.cellLink(row: row, col: col), id != 0, let p = aleno_pane_link(s, id) else { return nil }
        defer { aleno_string_free(p) }
        guard let url = URL(string: String(cString: p)), let scheme = url.scheme?.lowercased(),
              ["http", "https", "mailto", "file"].contains(scheme) else { return nil }   // never launch arbitrary schemes
        if scheme == "file" {
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue else { return nil }
        }
        return url
    }

    // MARK: Paths (⌘-click)

    /// The file or folder under (row, col), if the text there names something that exists. Relative paths resolve
    /// against the shell's reported working directory.
    func existingPath(at row: Int, col: Int) -> URL? { pathTarget(at: row, col: col)?.url }

    /// True for the prompt and the command being typed (from shell integration marks): nothing there is clickable.
    private func inPromptZone(row: Int) -> Bool {
        guard let s = stream, renderer.info.modes & (1 << 8) == 0 else { return false }
        let line = renderer.line(forRow: row)
        guard line >= 0 else { return false }
        let a = UInt32(line)
        var b = [AlenoBlock](repeating: AlenoBlock(), count: 2)
        let n = aleno_pane_blocks(s, a, a + 1, &b, b.count)
        guard n > 0 else { return false }
        let blk = b[n - 1]
        if blk.prompt_row > a { return false }
        return blk.output_row < 0 ? true : Int32(a) < blk.output_row
    }

    /// Like `existingPath`, plus the columns the clickable text covers.
    func pathTarget(at row: Int, col: Int) -> (url: URL, c0: Int, c1: Int)? {
        guard !inPromptZone(row: row), let tok = tokenParts(row: row, col: col) else { return nil }
        let hit = tok.parts
        let fm = FileManager.default
        let base = cwdPath.isEmpty ? NSHomeDirectory() : cwdPath
        for cand in hit {
            var p = cand
            if p.hasPrefix("~") { p = (p as NSString).expandingTildeInPath }
            else if !p.hasPrefix("/") { p = (base as NSString).appendingPathComponent(p) }
            p = (p as NSString).standardizingPath
            var isDir: ObjCBool = false
            if fm.fileExists(atPath: p, isDirectory: &isDir), isDir.boolValue, !(p as NSString).pathExtension.lowercased().elementsEqual("app") { return (URL(fileURLWithPath: p), tok.c0, tok.c1) }
        }
        return nil
    }

    /// Candidate path strings for the word at (row, col): the word itself, then without a trailing `:line:col`.
    private func tokenParts(row: Int, col: Int) -> (parts: [String], c0: Int, c1: Int)? {
        let cols = Int(renderer.info.cols)
        guard row >= 0, row < Int(renderer.info.rows), col >= 0, col < cols else { return nil }
        // One Character per column (wide-character spacers and non-text cells act as separators).
        var chars = [Character?](repeating: nil, count: cols)
        for c in 0..<cols {
            guard let cell = renderer.cellAt(row: row, col: c), cell.flags & 512 == 0 else { continue }
            if cell.ch & 0x8000_0000 != 0 { chars[c] = "\u{FFFC}"; continue }
            if let u = Unicode.Scalar(cell.ch) { chars[c] = Character(u) }
        }
        let stops: Set<Character> = [" ", "\t", "\"", "'", "`", "<", ">", "|", "(", ")", "[", "]", "{", "}", ",", ";", "=", "*", "\u{FFFC}"]
        func isSep(_ c: Int) -> Bool {
            guard let ch = chars[c] else { return true }
            if stops.contains(ch) || ch == "\0" { return true }
            if let u = ch.unicodeScalars.first, (0xE000...0xF8FF).contains(u.value) { return true }   // icon glyphs
            return false
        }
        guard !isSep(col) else { return nil }
        var a = col, b = col
        while a > 0, !isSep(a - 1) { a -= 1 }
        while b + 1 < cols, !isSep(b + 1) { b += 1 }
        var word = String(chars[a...b].compactMap { $0 })
        while let l = word.last, ".,:!?".contains(l) { word.removeLast(); b -= 1 }     // sentence punctuation
        guard word.count >= 2 else { return nil }
        var out = [word]
        if let r = word.range(of: #":\d+"#, options: .regularExpression) { out.append(String(word[..<r.lowerBound])) }
        if word.hasPrefix("file://"), let u = URL(string: word) { out.insert(u.path, at: 0) }
        return (out, a, max(a, b))
    }

    /// Folders open in Finder, documents in their default app. Programs and app bundles are only revealed:
    /// terminal output is untrusted and a click must not launch something.
    private func openPath(_ url: URL) {
        var isDir: ObjCBool = false
        FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir)
        let runnable = url.pathExtension.lowercased() == "app" || (!isDir.boolValue && FileManager.default.isExecutableFile(atPath: url.path))
        if runnable { NSWorkspace.shared.activateFileViewerSelecting([url]) } else { NSWorkspace.shared.open(url) }
    }

    // MARK: Selection & highlight

    func clearSelection() {
        if selAnchor != nil { markSelectionDirty() }
        selAnchor = nil; selHead = nil; selWordAnchor = nil; selMode = 0
        needsDisplay = true
    }

    private func markSelectionDirty() {
        guard let a = selAnchor, let h = selHead else { return }
        let lo = min(a.row, h.row), hi = max(a.row, h.row)
        renderer.markRowsDirty((0..<Int(renderer.info.rows)).filter { let l = renderer.line(forRow: $0); return l >= lo && l <= hi })
    }

    private func highlightFlags(row: Int, col: Int) -> UInt16 {
        var f: UInt16 = 0
        if let a = selAnchor, let h = selHead {
            let (p, q) = a <= h ? (a, h) : (h, a)
            if row >= p.row && row <= q.row {
                let from = row == p.row ? p.col : 0
                let to = row == q.row ? q.col : Int.max
                if col >= from && col <= to { f |= CellFlag.selected }
            }
        }
        if !matchesByRow.isEmpty, let list = matchesByRow[row] {
            for m in list where col >= m.c0 && col <= m.c1 { f |= (m.idx == currentMatch ? CellFlag.current : CellFlag.match) }
        }
        return f
    }

    // MARK: Search

    func showFind() {
        if findBar == nil {
            let fb = FindBar(frame: .zero)
            fb.onQuery = { [weak self] q in self?.runSearch(q) }
            fb.onNext = { [weak self] in self?.stepMatch(1) }
            fb.onPrev = { [weak self] in self?.stepMatch(-1) }
            fb.onClose = { [weak self] in self?.hideFind() }
            addSubview(fb)
            findBar = fb
            fb.translatesAutoresizingMaskIntoConstraints = false
            let top = fb.topAnchor.constraint(equalTo: topAnchor, constant: topInset + 8)   // below the title bar / tab strip
            findTopConstraint = top
            NSLayoutConstraint.activate([
                top,
                fb.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
                fb.widthAnchor.constraint(equalToConstant: 300), fb.heightAnchor.constraint(equalToConstant: 28),
            ])
        }
        if let fb = findBar, let top = findTopConstraint, !didAnimateFind {
            didAnimateFind = true
            fb.alphaValue = 0
            top.constant = topInset - 4
            layoutSubtreeIfNeeded()
            NSAnimationContext.runAnimationGroup { ctx in
                ctx.duration = 0.22
                ctx.timingFunction = CAMediaTimingFunction(name: .easeOut)
                ctx.allowsImplicitAnimation = true
                fb.animator().alphaValue = 1
                top.animator().constant = topInset + 8
                layoutSubtreeIfNeeded()
            }
        }
        findBar?.focus()
    }

    func hideFind() {
        didAnimateFind = false
        if let fb = findBar {
            findTopConstraint = nil; findBar = nil
            NSAnimationContext.runAnimationGroup({ ctx in
                ctx.duration = 0.15
                fb.animator().alphaValue = 0
            }, completionHandler: { fb.removeFromSuperview() })
        }
        matches = []; matchesByRow = [:]; currentMatch = -1
        renderer.markAllDirty(); needsDisplay = true
        window?.makeFirstResponder(self)
    }

    func stepMatch(_ dir: Int) {
        if findBar == nil { showFind(); return }
        guard !matches.isEmpty else { return }
        currentMatch = (currentMatch + dir + matches.count) % matches.count
        revealMatch(currentMatch)
        findBar?.setStatus("\(currentMatch + 1) of \(matches.count)")
    }

    private func runSearch(_ q: String) {
        guard let s = stream else { return }
        matches = []; matchesByRow = [:]; currentMatch = -1
        if !q.isEmpty {
            var buf = [UInt32](repeating: 0, count: 3 * 5000)
            let n = aleno_pane_search(s, q, 1, &buf, 5000)
            for k in 0..<min(n, 5000) { matches.append((Int(buf[k * 3]), Int(buf[k * 3 + 1]), Int(buf[k * 3 + 2]))) }
            for (i, m) in matches.enumerated() { matchesByRow[m.row, default: []].append((m.c0, m.c1, i)) }
            // Start from the match nearest the bottom of the current view.
            if !matches.isEmpty {
                let bottom = renderer.line(forRow: Int(renderer.info.rows) - 1) + 1
                currentMatch = matches.lastIndex(where: { $0.row < bottom }) ?? 0
                revealMatch(currentMatch)
            }
            findBar?.setStatus(matches.isEmpty ? "No matches" : "\(currentMatch + 1) of \(n)")
        } else { findBar?.setStatus("") }
        renderer.markAllDirty(); needsDisplay = true
    }

    private func revealMatch(_ i: Int) {
        guard let s = stream else { return }
        let info = renderer.info
        let row = matches[i].row
        if renderer.row(forLine: row) == nil {
            let off = max(0, min(Int(info.scrollback_len), Int(info.scrollback_len) - row + Int(info.rows) / 2))
            aleno_pane_set_display_offset(s, UInt32(off))
        }
        renderer.markAllDirty(); needsDisplay = true
    }

    // MARK: Scrolling commands

    func scrollPages(_ pages: Int) {
        guard let s = stream else { return }
        aleno_pane_scroll(s, Int32(pages * max(1, Int(renderer.info.rows) - 1)))
        needsDisplay = true
    }
    func scrollToTop() { if let s = stream { aleno_pane_set_display_offset(s, renderer.info.scrollback_len); needsDisplay = true } }
    func scrollToBottom() { if let s = stream { aleno_pane_set_display_offset(s, 0); needsDisplay = true } }
    func clearScrollbackAndScreen() { send([0x0c]); if let s = stream { aleno_pane_set_display_offset(s, 0) } }

    // MARK: Drag and drop

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let pb = sender.draggingPasteboard
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            pasteText(urls.map { KeyEncoder.escapePath($0.path) }.joined(separator: " ") + " ")
            return true
        }
        if let s = pb.string(forType: .string) { pasteText(s); return true }
        return false
    }

    // MARK: Teardown

    func shutdown() {
        detachStream()
        blinkTimer?.invalidate()
    }

    var cellSizePoints: CGSize {
        let scale = window?.backingScaleFactor ?? 2
        return CGSize(width: CGFloat(renderer.fonts.cellWidth) / scale, height: CGFloat(renderer.fonts.cellHeight) / scale)
    }
    var gridSize: (Int, Int) { (Int(renderer.info.cols), Int(renderer.info.rows)) }
}

// MARK: - Helpers on the renderer for cell inspection

extension PaneRenderer {
    func cellChar(row: Int, col: Int) -> UInt32? { cellAt(row: row, col: col)?.ch }
    func cellLink(row: Int, col: Int) -> UInt16? { cellAt(row: row, col: col)?.link }
}

/// Floating find field: just the search pill (no backing plate) and a match counter.
final class FindBar: NSView, NSSearchFieldDelegate {
    private let field = NSSearchField()
    private let status = NSTextField(labelWithString: "")
    var onQuery: ((String) -> Void)?
    var onNext: (() -> Void)?
    var onPrev: (() -> Void)?
    var onClose: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        field.placeholderString = "Find"
        field.delegate = self
        field.focusRingType = .none
        field.sendsSearchStringImmediately = true
        status.font = .systemFont(ofSize: 11); status.textColor = .secondaryLabelColor
        for v in [field, status] { v.translatesAutoresizingMaskIntoConstraints = false; addSubview(v) }
        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4), field.centerYAnchor.constraint(equalTo: centerYAnchor),
            field.widthAnchor.constraint(equalToConstant: 200),
            status.leadingAnchor.constraint(equalTo: field.trailingAnchor, constant: 8), status.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }
    func focus() { window?.makeFirstResponder(field) }
    func setStatus(_ s: String) { status.stringValue = s }
    func controlTextDidChange(_ obj: Notification) { onQuery?(field.stringValue) }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        if sel == #selector(NSResponder.insertNewline(_:)) {
            if NSApp.currentEvent?.modifierFlags.contains(.shift) == true { onPrev?() } else { onNext?() }
            return true
        }
        if sel == #selector(NSResponder.cancelOperation(_:)) { onClose?(); return true }
        return false
    }
}
