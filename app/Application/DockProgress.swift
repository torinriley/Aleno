import AppKit

/// Draws a progress bar over the app's Dock icon while a program reports progress (OSC 9;4).
final class DockProgress {
    static let shared = DockProgress()
    private let view = DockProgressView()
    private var timer: Timer?

    func set(_ p: (state: Int, percent: Int)) {
        let tile = NSApp.dockTile
        view.state = p.state
        view.percent = p.percent
        if p.state == 0 {
            timer?.invalidate(); timer = nil
            tile.contentView = nil
            tile.display()
            return
        }
        if tile.contentView !== view { tile.contentView = view }
        if p.state == 3, timer == nil {
            timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 20, repeats: true) { _ in NSApp.dockTile.display() }
        } else if p.state != 3 { timer?.invalidate(); timer = nil }
        tile.display()
    }
}

private final class DockProgressView: NSView {
    var state = 0
    var percent = 0

    override func draw(_ dirtyRect: NSRect) {
        NSApp.applicationIconImage.draw(in: bounds)
        let track = NSRect(x: bounds.width * 0.14, y: bounds.height * 0.1, width: bounds.width * 0.72, height: bounds.height * 0.075)
        let path = NSBezierPath(roundedRect: track, xRadius: track.height / 2, yRadius: track.height / 2)
        NSColor.black.withAlphaComponent(0.55).setFill(); path.fill()
        NSGraphicsContext.saveGraphicsState()
        path.addClip()
        (state == 2 ? NSColor.systemRed : state == 4 ? NSColor.systemYellow : NSColor.controlAccentColor).setFill()
        if state == 3 {
            let w = track.width * 0.3
            let t = (CACurrentMediaTime() * 0.8).truncatingRemainder(dividingBy: 1)
            NSRect(x: track.minX - w + (track.width + w) * CGFloat(t), y: track.minY, width: w, height: track.height).fill()
        } else {
            NSRect(x: track.minX, y: track.minY, width: track.width * (state == 2 && percent == 0 ? 1 : CGFloat(percent) / 100), height: track.height).fill()
        }
        NSGraphicsContext.restoreGraphicsState()
    }
}
