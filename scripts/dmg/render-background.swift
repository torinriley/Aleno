// Renders the DMG window background (light, an arrow between the two icons, a hint line) to <out>/background.png and
// <out>/background@2x.png. The background is light because Finder always draws icon labels in black.
// Icon positions must match scripts/dmg/settings.py: both icons sit on y = ICON_Y, and the arrow is drawn on the same line.
// Usage: swiftc -O scripts/dmg/render-background.swift -o /tmp/render-dmg-bg && /tmp/render-dmg-bg <out-dir>
import AppKit

// Layout, in points.
let WIDTH: CGFloat = 660, HEIGHT: CGFloat = 340      // the window's content area; see window_rect in settings.py
let ICON_Y: CGFloat = 140, APP_X: CGFloat = 180, FOLDER_X: CGFloat = 480
let ICON_SIZE: CGFloat = 112

let args = CommandLine.arguments
guard args.count == 2 else { print("usage: render-background <out-dir>"); exit(1) }

func render(scale: CGFloat, to path: String) {
    let w = Int(WIDTH * scale), h = Int(HEIGHT * scale)
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8, samplesPerPixel: 4,
                               hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 32)!
    let ctx = NSGraphicsContext(bitmapImageRep: rep)!.cgContext
    ctx.translateBy(x: 0, y: CGFloat(h)); ctx.scaleBy(x: scale, y: -scale)      // top-left origin, points
    NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)

    NSColor(srgbRed: 0.965, green: 0.965, blue: 0.973, alpha: 1).setFill()      // #f6f6f8
    NSRect(x: 0, y: 0, width: WIDTH, height: HEIGHT).fill()

    // Arrow: a shaft and an open arrowhead, on the icons' center line.
    let ax0 = APP_X + ICON_SIZE / 2 + 24, ax1 = FOLDER_X - ICON_SIZE / 2 - 24
    let arrow = NSBezierPath()
    arrow.move(to: NSPoint(x: ax0, y: ICON_Y)); arrow.line(to: NSPoint(x: ax1, y: ICON_Y))
    arrow.move(to: NSPoint(x: ax1 - 14, y: ICON_Y - 14)); arrow.line(to: NSPoint(x: ax1, y: ICON_Y)); arrow.line(to: NSPoint(x: ax1 - 14, y: ICON_Y + 14))
    arrow.lineWidth = 4; arrow.lineCapStyle = .round; arrow.lineJoinStyle = .round
    NSColor(white: 0.12, alpha: 1).setStroke(); arrow.stroke()

    // Hint line.
    let style = NSMutableParagraphStyle(); style.alignment = .center
    NSAttributedString(string: "Drag Aleno to Applications to install", attributes: [
        .font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor(white: 0.45, alpha: 1), .paragraphStyle: style])
        .draw(in: NSRect(x: 0, y: 288, width: WIDTH, height: 24))

    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
}

try? FileManager.default.createDirectory(atPath: args[1], withIntermediateDirectories: true)
render(scale: 1, to: args[1] + "/background.png")
render(scale: 2, to: args[1] + "/background@2x.png")
print("wrote background.png and background@2x.png")
