import AppKit

/// A quiet, flat button: hairline outline, no fill until hovered. Used for the links in the About window.
private final class LinkButton: NSControl {
    private let label: String
    private let symbol: String
    private var hovering = false { didSet { needsDisplay = true } }
    private var pressed = false { didSet { needsDisplay = true } }

    init(title: String, symbol: String, target: AnyObject, action: Selector) {
        self.label = title
        self.symbol = symbol
        super.init(frame: NSRect(x: 0, y: 0, width: 132, height: 32))
        self.target = target
        self.action = action
        setAccessibilityRole(.button)
        setAccessibilityLabel(title)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil))
    }
    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize { NSSize(width: 132, height: 32) }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseEntered(with event: NSEvent) { hovering = true }
    override func mouseExited(with event: NSEvent) { hovering = false; pressed = false }
    override func mouseDown(with event: NSEvent) { pressed = true }
    override func mouseUp(with event: NSEvent) {
        let inside = bounds.contains(convert(event.locationInWindow, from: nil))
        pressed = false
        if inside { sendAction(action, to: target) }
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 36 || event.keyCode == 49 { sendAction(action, to: target) } else { super.keyDown(with: event) }
    }

    override func draw(_ dirtyRect: NSRect) {
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: 9, yRadius: 9)
        NSColor.labelColor.withAlphaComponent(pressed ? 0.16 : (hovering ? 0.09 : 0.0)).setFill()
        path.fill()
        NSColor.labelColor.withAlphaComponent(hovering ? 0.30 : 0.18).setStroke()
        path.lineWidth = 1
        path.stroke()

        let color = NSColor.labelColor.withAlphaComponent(hovering ? 1.0 : 0.82)
        let font = NSFont.systemFont(ofSize: 12.5, weight: .medium)
        let text = NSAttributedString(string: label, attributes: [.font: font, .foregroundColor: color])
        var icon: NSImage?
        if let img = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 11.5, weight: .medium)) {
            icon = img
        }
        let iconW: CGFloat = icon == nil ? 0 : 15
        let gap: CGFloat = icon == nil ? 0 : 6
        let total = iconW + gap + text.size().width
        var x = (bounds.width - total) / 2
        if let icon {
            let tinted = icon.copy() as! NSImage
            tinted.lockFocus()
            color.set()
            NSRect(origin: .zero, size: tinted.size).fill(using: .sourceAtop)
            tinted.unlockFocus()
            let h = tinted.size.height
            tinted.draw(in: NSRect(x: x, y: (bounds.height - h) / 2, width: tinted.size.width, height: h))
            x += iconW + gap
        }
        text.draw(at: NSPoint(x: x, y: (bounds.height - text.size().height) / 2))
    }
}

private final class AboutWindow: NSWindow {
    override func cancelOperation(_ sender: Any?) { performClose(sender) }   // Esc closes it
}

/// The About window: icon, name, the exact version and build, a tagline and two links.
final class AboutWindowController: NSObject, NSWindowDelegate {
    static let shared = AboutWindowController()
    static let websiteURL = URL(string: "https://www.aleno.app")!
    static let githubURL = URL(string: "https://github.com/torinriley/Aleno")!
    static let tagline = "A fast, minimal, native terminal for macOS."

    private var window: NSWindow?

    /// "Version 0.1.0 (24)", always read from the running app's bundle.
    static var versionString: String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String
        if let build, !build.isEmpty { return "Version \(version) (\(build))" }
        return "Version \(version)"
    }

    func show() {
        if let w = window {
            w.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }
        let w = AboutWindow(contentRect: NSRect(x: 0, y: 0, width: 320, height: 338),
                         styleMask: [.titled, .closable, .fullSizeContentView], backing: .buffered, defer: false)
        w.titlebarAppearsTransparent = true
        w.titleVisibility = .hidden
        w.isMovableByWindowBackground = true
        w.isReleasedWhenClosed = false
        w.standardWindowButton(.miniaturizeButton)?.isHidden = true
        w.standardWindowButton(.zoomButton)?.isHidden = true
        w.delegate = self
        w.setAccessibilityLabel("About Aleno")

        let content = NSVisualEffectView()
        content.material = .windowBackground
        content.state = .active
        w.contentView = content

        let icon = NSImageView()
        icon.image = NSApp.applicationIconImage
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.setAccessibilityLabel("Aleno icon")

        func label(_ s: String, size: CGFloat, weight: NSFont.Weight, color: NSColor) -> NSTextField {
            let t = NSTextField(labelWithString: s)
            t.font = .systemFont(ofSize: size, weight: weight)
            t.textColor = color
            t.alignment = .center
            t.lineBreakMode = .byWordWrapping
            t.maximumNumberOfLines = 2
            t.isSelectable = true
            return t
        }
        let name = label("Aleno", size: 22, weight: .semibold, color: .labelColor)
        let version = label(AboutWindowController.versionString, size: 12, weight: .regular, color: .secondaryLabelColor)
        let tagline = label(AboutWindowController.tagline, size: 12.5, weight: .regular, color: .labelColor.withAlphaComponent(0.85))
        tagline.preferredMaxLayoutWidth = 296
        let site = LinkButton(title: "aleno.app", symbol: "globe", target: self, action: #selector(openWebsite))
        let github = LinkButton(title: "GitHub", symbol: "chevron.left.forwardslash.chevron.right", target: self, action: #selector(openGitHub))
        let buttons = NSStackView(views: [site, github])
        buttons.orientation = .horizontal
        buttons.spacing = 10
        let copyright = label("\u{00A9} 2026 Torin Etheridge \u{00B7} MIT License", size: 11, weight: .regular, color: .tertiaryLabelColor)

        let stack = NSStackView(views: [icon, name, version, tagline, buttons, copyright])
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 6
        stack.setCustomSpacing(10, after: icon)
        stack.setCustomSpacing(14, after: version)
        stack.setCustomSpacing(18, after: tagline)
        stack.setCustomSpacing(20, after: buttons)
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 96), icon.heightAnchor.constraint(equalToConstant: 96),
            site.widthAnchor.constraint(equalToConstant: 132), site.heightAnchor.constraint(equalToConstant: 32),
            github.widthAnchor.constraint(equalToConstant: 132), github.heightAnchor.constraint(equalToConstant: 32),
            stack.centerXAnchor.constraint(equalTo: content.centerXAnchor),
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 44),
            stack.widthAnchor.constraint(lessThanOrEqualToConstant: 300),
        ])

        w.center()
        window = w
        w.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func openWebsite() { NSWorkspace.shared.open(AboutWindowController.websiteURL) }
    @objc private func openGitHub() { NSWorkspace.shared.open(AboutWindowController.githubURL) }

    func windowWillClose(_ notification: Notification) { window = nil }
}
