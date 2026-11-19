// Renders the Aleno wordmark SVG to the 384x120 coverage bitmap the welcome animation embeds.
// Usage: swiftc -O scripts/render-welcome-logo.swift -o /tmp/render-logo && /tmp/render-logo design/wordmark/aleno_black_transparent.svg crates/aleno-daemon/src/welcome_logo.bin
import AppKit
let args = CommandLine.arguments
guard args.count == 3, let img = NSImage(contentsOf: URL(fileURLWithPath: args[1])) else { print("load failed"); exit(1) }
print("svg size:", img.size)
let W = 384, H = 120
let s = CGFloat(W) / 1536        // the wordmark's bounding box is x 80..1616, y 80..560 in a 1696x640 viewBox
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: W, pixelsHigh: H, bitsPerSample: 8, samplesPerPixel: 4,
                           hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: W * 4, bitsPerPixel: 32)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor.clear.set(); NSRect(x: 0, y: 0, width: W, height: H).fill()
img.draw(in: NSRect(x: -80 * s, y: -80 * s, width: 1696 * s, height: 640 * s), from: .zero, operation: .sourceOver, fraction: 1)
NSGraphicsContext.restoreGraphicsState()
var out = [UInt8](repeating: 0, count: W * H)
let p = rep.bitmapData!
var ink = 0
for y in 0..<H { for x in 0..<W { let a = p[(y * W + x) * 4 + 3]; out[y * W + x] = a; if a > 127 { ink += 1 } } }
try! Data(out).write(to: URL(fileURLWithPath: args[2]))
print("wrote \(W)x\(H), ink pixels \(ink)")
// ASCII preview, top row first (bitmap rows are top-down in memory)
for y in stride(from: 0, to: H, by: 6) { print(String((0..<W).step(3).map { out[y * W + $0] > 127 ? "#" : "." })) }
extension Range where Bound == Int { func step(_ n: Int) -> [Int] { Array(stride(from: lowerBound, to: upperBound, by: n)) } }
