import AppKit

protocol TabBarDelegate: AnyObject {
    func tabBarSelect(_ i: Int)
    func tabBarClose(_ i: Int)
    func tabBarMove(from: Int, to: Int)
    func tabBarNewTab()
    func tabBarTearBegin(_ i: Int, at screenPoint: NSPoint)
    func tabBarTearMove(at screenPoint: NSPoint)
    func tabBarTearEnd(at screenPoint: NSPoint)
}

/// Translucent capsule behind the tab strip and "+" button. Deliberately plain (no NSGlassEffectView): Liquid Glass
/// on a translucent window left a flat grey block under the tab bar where the backdrop blur disappeared.
final class GlassView: NSView {
    private let radius: CGFloat
    init(cornerRadius: CGFloat) {
        radius = cornerRadius
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = cornerRadius
        layer?.borderWidth = 0.75
        layer?.masksToBounds = true
    }
    required init?(coder: NSCoder) { fatalError() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = NSColor.labelColor.withAlphaComponent(0.07).cgColor
            layer?.borderColor = NSColor.labelColor.withAlphaComponent(0.14).cgColor
        }
    }
    override func viewDidChangeEffectiveAppearance() { needsDisplay = true }
}

/// Draws the tab contents above the glass layers; transparent to events.
final class TabBarOverlay: NSView {
    weak var owner: TabBarView?
    var drawsTabs = false
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { if drawsTabs { owner?.drawTabs() } else { owner?.drawTitle() } }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// Native-style tab bar: title row on top, a glass capsule strip with ⌘N hints below, round glass "+" button.
final class TabBarView: NSView {
    weak var delegate: TabBarDelegate?
    var titles: [String] = [] { didSet { if titles.count != oldValue.count { startLayoutAnimation(from: oldValue.count) }; syncFades(); needsDisplay = true } }
    var active = 0 { didSet { needsDisplay = true } }
    /// Per-tab progress reported by the program (OSC 9;4): state 0 none, 1 normal, 2 error, 3 indeterminate, 4 paused.
    var progress: [(state: Int, percent: Int)] = [] {
        didSet {
            guard progress.count != oldValue.count || zip(progress, oldValue).contains(where: { $0 != $1 }) else { return }
            needsDisplay = true
            let sweeping = progress.contains { $0.state == 3 }
            if sweeping, progressTimer == nil {
                progressTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self] _ in self?.needsDisplay = true }
            } else if !sweeping { progressTimer?.invalidate(); progressTimer = nil }
        }
    }
    private var progressTimer: Timer?

    /// A thin progress track with a fill (or a sweeping segment while the amount is unknown).
    private func drawProgress(_ p: (state: Int, percent: Int), in track: NSRect) {
        guard p.state != 0, track.width > 4 else { return }
        let color: NSColor = p.state == 2 ? .systemRed : (p.state == 4 ? .systemYellow : .controlAccentColor)
        let clip = NSBezierPath(roundedRect: track, xRadius: track.height / 2, yRadius: track.height / 2)
        NSGraphicsContext.saveGraphicsState()
        clip.addClip()
        NSColor.labelColor.withAlphaComponent(0.12).setFill(); track.fill()
        color.setFill()
        if p.state == 3 {
            let w = track.width * 0.3
            let t = (CACurrentMediaTime() * 0.8).truncatingRemainder(dividingBy: 1)
            NSRect(x: track.minX - w + (track.width + w) * CGFloat(t), y: track.minY, width: w, height: track.height).fill()
        } else {
            let frac = p.state == 2 && p.percent == 0 ? 1 : CGFloat(p.percent) / 100
            NSRect(x: track.minX, y: track.minY, width: track.width * frac, height: track.height).fill()
        }
        NSGraphicsContext.restoreGraphicsState()
    }
    var windowTitle = "" { didSet { syncFades(); needsDisplay = true } }

    // Tab-label crossfades, keyed by tab index (the window title at the top is not animated).
    private var shown: [Int: String] = [:]
    private var previous: [Int: String] = [:]
    private var fadeStart: [Int: CFTimeInterval] = [:]
    private var fadeTimer: Timer?
    private let fadeDuration: CFTimeInterval = 0.22

    private var appeared = false

    /// Replays the fade-in the first time the window is actually on screen (titles may already be known at creation).
    private func playAppearFadeIfNeeded() {
        guard !appeared, window?.isVisible == true else { return }
        appeared = true
        let now = CACurrentMediaTime()
        for (k, t) in shown where !t.isEmpty && k >= 0 { previous[k] = ""; fadeStart[k] = now }
        startFadeTimer()
    }

    private func startFadeTimer() {
        guard fadeTimer == nil, !fadeStart.isEmpty else { return }
        fadeTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { [weak self] t in
            guard let self else { t.invalidate(); return }
            let now = CACurrentMediaTime()
            self.fadeStart = self.fadeStart.filter { now - $0.value < self.fadeDuration }
            self.needsDisplay = true
            if self.fadeStart.isEmpty { t.invalidate(); self.fadeTimer = nil }
        }
    }

    private func syncFades() {
        let now = CACurrentMediaTime()
        var current: [Int: String] = [:]
        for (i, t) in titles.enumerated() { current[i] = t }
        for (k, new) in current {
            let old = shown[k] ?? ""
            if new != old { previous[k] = old; fadeStart[k] = now; shown[k] = new }
        }
        for k in shown.keys where current[k] == nil { shown[k] = nil; previous[k] = nil; fadeStart[k] = nil }
        startFadeTimer()
    }

    private func fadeProgress(_ k: Int) -> CGFloat {
        guard let st = fadeStart[k] else { return 1 }
        let x = min(1, max(0, (CACurrentMediaTime() - st) / fadeDuration))
        return CGFloat(x * x * (3 - 2 * x))   // smoothstep
    }

    /// Draws `text`, crossfading from its previous value after a change.
    private func drawFading(key: Int, text: String, rect: NSRect, font: NSFont, color: NSColor, style: NSParagraphStyle) {
        let p = fadeProgress(key)
        func draw(_ t: String, _ a: CGFloat) {
            guard !t.isEmpty, a > 0.01 else { return }
            (t as NSString).draw(in: rect, withAttributes: [.font: font, .foregroundColor: color.withAlphaComponent(color.alphaComponent * a), .paragraphStyle: style])
        }
        if p < 1 { draw(previous[key] ?? "", 1 - p) }
        draw(text, p)
    }
    var leftInset: CGFloat = 78 { didSet { needsLayout = true; needsDisplay = true } }

    static let titleRowHeight: CGFloat = 32
    static let totalHeight: CGFloat = 32 + 40

    private let strip = GlassView(cornerRadius: 17)
    private let plus = GlassView(cornerRadius: 17)
    private var hover: Int?
    private var hoverClose = false
    private var hoverPlus = false
    private var dragIndex: Int?
    private var dragStartX: CGFloat = 0
    private var dragMoved = false
    private var lastMouse = NSPoint.zero

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { !tabsVisible || (dragIndex == nil && tabIndex(at: lastMouse) == nil && !plusRect.contains(lastMouse)) }

    override init(frame: NSRect) {
        super.init(frame: frame)
        addSubview(strip); addSubview(plus); addSubview(overlay); addSubview(tabsOverlay)
        overlay.owner = self; tabsOverlay.owner = self; tabsOverlay.drawsTabs = true
        for v in [strip, plus, tabsOverlay] { v.wantsLayer = true; v.alphaValue = 0 }
    }
    private let overlay = TabBarOverlay()
    private let tabsOverlay = TabBarOverlay()
    private(set) var tabsVisible = false
    override var needsDisplay: Bool { didSet { overlay.needsDisplay = true; tabsOverlay.needsDisplay = true } }

    /// Fades the tab strip and "+" button in or out (single-tab windows show only the title row).
    func setTabsVisible(_ on: Bool, animated: Bool) {
        guard on != tabsVisible else { return }
        tabsVisible = on
        let views: [NSView] = [strip, plus, tabsOverlay]
        if animated {
            NSAnimationContext.runAnimationGroup { ctx in
                ctx.duration = 0.28
                ctx.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
                for v in views { v.animator().alphaValue = on ? 1 : 0 }
            }
        } else { for v in views { v.alphaValue = on ? 1 : 0 } }
    }
    required init?(coder: NSCoder) { fatalError() }

    private var stripRect: NSRect { NSRect(x: 12, y: TabBarView.titleRowHeight + 2, width: max(0, bounds.width - 12 - 12 - 40), height: 34) }
    private var plusRect: NSRect { NSRect(x: bounds.width - 12 - 34, y: TabBarView.titleRowHeight + 2, width: 34, height: 34) }

    override func layout() {
        super.layout()
        strip.frame = stripRect
        plus.frame = plusRect
        overlay.frame = bounds
        tabsOverlay.frame = bounds
    }

    // Tab layout animation: tabs slide/resize to their new equal-width slots when one is added or closed, and a
    // new tab grows out of the right end of the strip while fading in. Positions are fractions of the strip width.
    private var animFrom: [(x: CGFloat, w: CGFloat)] = []
    private var animStart: CFTimeInterval = 0
    private var animFirstNew = Int.max
    private var layoutTimer: Timer?
    private let layoutDuration: CFTimeInterval = 0.32

    private var layoutProgress: CGFloat {
        guard animStart > 0 else { return 1 }
        let t = min(1, (CACurrentMediaTime() - animStart) / layoutDuration)
        let e = 1 - pow(1 - t, 3)    // ease-out
        return CGFloat(e)
    }

    private func targetFraction(_ i: Int) -> (x: CGFloat, w: CGFloat) {
        let n = CGFloat(max(1, titles.count))
        return (CGFloat(i) / n, 1 / n)
    }

    private func currentFraction(_ i: Int) -> (x: CGFloat, w: CGFloat) {
        let to = targetFraction(i)
        let p = layoutProgress
        if p >= 1 { return to }
        let from = i < animFrom.count ? animFrom[i] : (x: 1, w: 0)
        return (from.x + (to.x - from.x) * p, from.w + (to.w - from.w) * p)
    }

    private var tabWidth: CGFloat { stripRect.width / CGFloat(max(1, titles.count)) }

    private func startLayoutAnimation(from oldCount: Int) {
        guard oldCount > 0, window?.isVisible == true else { animStart = 0; return }
        // Start from wherever the tabs are right now (a new tab may be added mid-animation).
        animFrom = (0..<max(oldCount, 0)).map { i in animStart > 0 && layoutProgress < 1 && i < animFrom.count ? currentFraction(i) : (x: CGFloat(i) / CGFloat(oldCount), w: 1 / CGFloat(oldCount)) }
        animFirstNew = oldCount
        animStart = CACurrentMediaTime()
        layoutTimer?.invalidate()
        layoutTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { [weak self] t in
            guard let self else { t.invalidate(); return }
            self.needsDisplay = true
            if CACurrentMediaTime() - self.animStart >= self.layoutDuration {
                t.invalidate(); self.layoutTimer = nil; self.animStart = 0; self.animFirstNew = Int.max; self.needsDisplay = true
            }
        }
    }

    private func tabRect(_ i: Int) -> NSRect {
        let r = stripRect
        let f = currentFraction(i)
        return NSRect(x: r.minX + f.x * r.width, y: r.minY, width: f.w * r.width, height: r.height).insetBy(dx: 2, dy: 2)
    }
    private func closeRect(_ i: Int) -> NSRect { let r = tabRect(i); return NSRect(x: r.maxX - 30, y: r.midY - 9, width: 18, height: 18) }

    private func tabIndex(at p: NSPoint) -> Int? {
        for i in titles.indices where tabRect(i).insetBy(dx: -2, dy: -2).contains(p) { return i }
        return nil
    }

    func drawTitle() {
        playAppearFadeIfNeeded()
        // Title row: folder glyph + bold title, as in Terminal.app.
        let para = NSMutableParagraphStyle(); para.lineBreakMode = .byTruncatingTail
        let tx = leftInset + 22
        if let img = NSImage(systemSymbolName: "folder.fill", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 13, weight: .regular).applying(.init(paletteColors: [.systemBlue]))) {
            // The folder icon follows the title: invisible while empty, fading in with the first title.
            img.draw(in: NSRect(x: leftInset, y: 9, width: 18, height: 15))
        }
        if titles.count <= 1, let p = progress.first { drawProgress(p, in: NSRect(x: 12, y: TabBarView.titleRowHeight - 3, width: max(0, bounds.width - 24), height: 2.5)) }
        (windowTitle as NSString).draw(in: NSRect(x: tx, y: 8, width: max(0, bounds.width - tx - 16), height: 18),
            withAttributes: [.font: NSFont.systemFont(ofSize: 13, weight: .semibold), .foregroundColor: NSColor.labelColor, .paragraphStyle: para])

    }

    func drawTabs() {
        let para = NSMutableParagraphStyle(); para.lineBreakMode = .byTruncatingTail
        _ = para
        let multi = titles.count > 1
        for (i, t) in titles.enumerated() {
            let r = tabRect(i)
            if r.width < 1 { continue }
            let growing = animStart > 0 && i >= animFirstNew
            let cg = NSGraphicsContext.current?.cgContext
            if growing { cg?.saveGState(); cg?.setAlpha(layoutProgress) }
            defer { if growing { cg?.restoreGState() } }
            let isActive = i == active
            if isActive {
                let path = NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2)
                NSColor.labelColor.withAlphaComponent(0.13).setFill(); path.fill()
                NSColor.labelColor.withAlphaComponent(0.22).setStroke(); path.lineWidth = 0.75; path.stroke()
            } else if hover == i {
                NSColor.labelColor.withAlphaComponent(0.06).setFill()
                NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2).fill()
            }
            // separator between two inactive neighbours
            if i > 0, i != active, i - 1 != active {
                NSColor.labelColor.withAlphaComponent(0.18).setFill()
                NSRect(x: r.minX - 2, y: r.minY + 7, width: 1, height: r.height - 14).fill()
            }
            let color: NSColor = isActive ? .labelColor : .secondaryLabelColor
            let hint = i < 9 ? "⌘\(i + 1)" : ""
            let showClose = multi && hover == i
            let rightPad: CGFloat = 40
            let textRect = NSRect(x: r.minX + 40, y: r.midY - 9, width: max(0, r.width - 40 - rightPad), height: 18)
            let centered = NSMutableParagraphStyle(); centered.lineBreakMode = .byTruncatingTail; centered.alignment = .center
            if animStart > 0, layoutProgress < 1 {
                // While tabs slide to their new widths, lay the title out once at the width it will end up with (so the
                // truncation point does not creep from one character to the next every frame), keep it centred on the
                // moving tab, snap it to whole pixels, and clip it to the tab as it is now.
                let to = targetFraction(i)
                let laidW = max(0, to.w * stripRect.width - 4 - 40 - rightPad)
                let scale = window?.backingScaleFactor ?? 2
                let x = ((textRect.midX - laidW / 2) * scale).rounded() / scale
                let laid = NSRect(x: x, y: textRect.minY, width: laidW, height: textRect.height)
                NSGraphicsContext.saveGraphicsState()
                NSBezierPath(rect: textRect).addClip()
                drawFading(key: i, text: t, rect: laid, font: NSFont.systemFont(ofSize: 13), color: color, style: centered)
                NSGraphicsContext.restoreGraphicsState()
            } else {
                drawFading(key: i, text: t, rect: textRect, font: NSFont.systemFont(ofSize: 13), color: color, style: centered)
            }
            if i < progress.count { drawProgress(progress[i], in: NSRect(x: r.minX + 18, y: r.maxY - 6, width: max(0, r.width - 36), height: 2.5)) }
            if showClose {
                let c = closeRect(i)
                if hoverClose { NSColor.labelColor.withAlphaComponent(0.14).setFill(); NSBezierPath(ovalIn: c).fill() }
                NSColor.secondaryLabelColor.setStroke()
                let p = NSBezierPath(); p.lineWidth = 1.3
                p.move(to: NSPoint(x: c.minX + 5.5, y: c.minY + 5.5)); p.line(to: NSPoint(x: c.maxX - 5.5, y: c.maxY - 5.5))
                p.move(to: NSPoint(x: c.maxX - 5.5, y: c.minY + 5.5)); p.line(to: NSPoint(x: c.minX + 5.5, y: c.maxY - 5.5))
                p.stroke()
            } else if !hint.isEmpty {
                (hint as NSString).draw(in: NSRect(x: r.maxX - 44, y: r.midY - 8, width: 34, height: 16),
                    withAttributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.tertiaryLabelColor,
                                     .paragraphStyle: { let p = NSMutableParagraphStyle(); p.alignment = .right; return p }()])
            }
        }
        // "+" in its own round glass button
        let pr = plusRect
        if hoverPlus { NSColor.labelColor.withAlphaComponent(0.10).setFill(); NSBezierPath(ovalIn: pr).fill() }
        NSColor.labelColor.setStroke()
        let p = NSBezierPath(); p.lineWidth = 1.5
        p.move(to: NSPoint(x: pr.midX - 6, y: pr.midY)); p.line(to: NSPoint(x: pr.midX + 6, y: pr.midY))
        p.move(to: NSPoint(x: pr.midX, y: pr.midY - 6)); p.line(to: NSPoint(x: pr.midX, y: pr.midY + 6))
        p.stroke()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for a in trackingAreas { removeTrackingArea(a) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil))
    }

    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        lastMouse = p
        var h: Int? = nil; var hc = false
        if let i = tabIndex(at: p) { h = i; hc = closeRect(i).contains(p) }
        let hp = plusRect.contains(p)
        if h != hover || hc != hoverClose || hp != hoverPlus { hover = h; hoverClose = hc; hoverPlus = hp; needsDisplay = true }
    }
    override func mouseExited(with event: NSEvent) { hover = nil; hoverClose = false; hoverPlus = false; needsDisplay = true }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        lastMouse = p
        if !tabsVisible {
            if event.clickCount == 2 { window?.performZoom(nil) }
            return
        }
        if plusRect.contains(p) { delegate?.tabBarNewTab(); return }
        guard let i = tabIndex(at: p) else {
            if event.clickCount == 2 && p.y < TabBarView.titleRowHeight { window?.performZoom(nil) }
            return
        }
        if titles.count > 1 && closeRect(i).contains(p) { delegate?.tabBarClose(i); return }
        dragIndex = i; dragStartX = p.x; dragMoved = false
        if i != active { delegate?.tabBarSelect(i) }
    }

    override func mouseDragged(with event: NSEvent) {
        if tearing, let w = window { delegate?.tabBarTearMove(at: w.convertPoint(toScreen: event.locationInWindow)); return }
        guard let i = dragIndex else { return }
        let p = convert(event.locationInWindow, from: nil)
        if abs(p.x - dragStartX) > 4 { dragMoved = true }
        guard dragMoved else { return }
        // Dragged well clear of the strip: lift the tab out into its own window.
        if titles.count > 1, !stripRect.insetBy(dx: -40, dy: -36).contains(p), let w = window {
            dragIndex = nil; dragMoved = false; tearing = true
            delegate?.tabBarTearBegin(i, at: w.convertPoint(toScreen: event.locationInWindow))
            return
        }
        let target = max(0, min(titles.count - 1, Int((p.x - stripRect.minX) / tabWidth)))
        if target != i { delegate?.tabBarMove(from: i, to: target); dragIndex = target }
    }
    private var tearing = false
    override func mouseUp(with event: NSEvent) {
        if tearing, let w = window {
            tearing = false
            delegate?.tabBarTearEnd(at: w.convertPoint(toScreen: event.locationInWindow))
        }
        dragIndex = nil; dragMoved = false
    }

    override func otherMouseUp(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if event.buttonNumber == 2, let i = tabIndex(at: p) { delegate?.tabBarClose(i) }
    }
}

/// Transient message shown at the bottom of a window (configuration diagnostics).
final class ToastView: NSVisualEffectView {
    private let label = NSTextField(wrappingLabelWithString: "")
    private var hideWork: DispatchWorkItem?
    override init(frame: NSRect) {
        super.init(frame: frame)
        material = .hudWindow; blendingMode = .withinWindow; state = .active
        wantsLayer = true; layer?.cornerRadius = 8
        label.font = .systemFont(ofSize: 12); label.textColor = .labelColor
        label.maximumNumberOfLines = 6
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12), label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            label.topAnchor.constraint(equalTo: topAnchor, constant: 8), label.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
        ])
        isHidden = true
    }
    required init?(coder: NSCoder) { fatalError() }

    func show(_ text: String, sticky: Bool) {
        label.stringValue = text
        isHidden = false
        hideWork?.cancel()
        if !sticky {
            let w = DispatchWorkItem { [weak self] in self?.isHidden = true }
            hideWork = w
            DispatchQueue.main.asyncAfter(deadline: .now() + 6, execute: w)
        }
    }
    func dismiss() { hideWork?.cancel(); isHidden = true }
    override func mouseDown(with event: NSEvent) { dismiss() }
}


/// Small centered "cols × rows" pill shown while a pane is being resized, fading out shortly after it stops.
final class SizeBadge: NSVisualEffectView {
    private let label = NSTextField(labelWithString: "")
    private var hideWork: DispatchWorkItem?
    override init(frame: NSRect) {
        super.init(frame: frame)
        material = .hudWindow; blendingMode = .withinWindow; state = .active
        wantsLayer = true; layer?.cornerRadius = 9
        label.font = .monospacedDigitSystemFont(ofSize: 13, weight: .medium); label.textColor = .labelColor
        label.alignment = .center
        addSubview(label)
        alphaValue = 0; isHidden = true
    }
    required init?(coder: NSCoder) { fatalError() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func show(_ text: String, over rect: NSRect) {
        label.stringValue = text
        label.sizeToFit()
        let w = label.frame.width + 28, h: CGFloat = 30
        frame = NSRect(x: (rect.midX - w / 2).rounded(), y: (rect.midY - h / 2).rounded(), width: w, height: h)
        label.frame = NSRect(x: 0, y: (h - label.frame.height) / 2, width: w, height: label.frame.height)
        isHidden = false
        alphaValue = 1
        hideWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            NSAnimationContext.runAnimationGroup({ $0.duration = 0.25; self?.animator().alphaValue = 0 },
                                                 completionHandler: { if self?.alphaValue == 0 { self?.isHidden = true } })
        }
        hideWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.7, execute: work)
    }
}

