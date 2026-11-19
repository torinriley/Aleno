import Metal
import CoreText
import CoreGraphics

struct AtlasEntry {
    var x: UInt16, y: UInt16, w: UInt16, h: UInt16
    var isColor: Bool
}

/// Shelf-packed RGBA8 glyph atlas. Each glyph is rasterized into a cell-sized tile (two cells wide for
/// wide characters), so the renderer needs no per-glyph bearings: one cell = one textured quad.
final class GlyphAtlas {
    static let size = 2048
    let texture: MTLTexture
    let fonts: FontSet
    private(set) var generation = 0

    private var entries: [Key: AtlasEntry] = [:]
    private var penX = 0, penY = 0

    struct Key: Hashable { var text: String; var style: UInt8; var span: Int }

    /// Set when the atlas had to be flushed during a frame; callers must rebuild instances.
    init(device: MTLDevice, fonts: FontSet) {
        self.fonts = fonts
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: GlyphAtlas.size, height: GlyphAtlas.size, mipmapped: false)
        d.usage = [.shaderRead]
        d.storageMode = .shared
        texture = device.makeTexture(descriptor: d)!
        texture.label = "Aleno glyph atlas"
    }

    func entry(text: String, bold: Bool, italic: Bool, wide: Bool) -> AtlasEntry {
        entry(text: text, bold: bold, italic: italic, span: wide ? 2 : 1)
    }

    /// `span`: how many cells wide the tile is (2 for wide characters, N for an N-character ligature run).
    func entry(text: String, bold: Bool, italic: Bool, span: Int) -> AtlasEntry {
        let key = Key(text: text, style: (bold ? 1 : 0) | (italic ? 2 : 0), span: span)
        if let e = entries[key] { return e }
        let tw = fonts.cellWidth * span, th = fonts.cellHeight
        if penX + tw > GlyphAtlas.size { penX = 0; penY += th }
        if penY + th > GlyphAtlas.size { flush() }
        let (pixels, color) = rasterize(text: text, bold: bold, italic: italic, tw: tw, th: th)
        pixels.withUnsafeBytes { buf in
            texture.replace(region: MTLRegionMake2D(penX, penY, tw, th), mipmapLevel: 0, withBytes: buf.baseAddress!, bytesPerRow: tw * 4)
        }
        let e = AtlasEntry(x: UInt16(penX), y: UInt16(penY), w: UInt16(tw), h: UInt16(th), isColor: color)
        entries[key] = e
        penX += tw
        return e
    }

    func flush() {
        entries.removeAll(keepingCapacity: true)
        penX = 0; penY = 0
        generation += 1
    }

    var count: Int { entries.count }

    private func rasterize(text: String, bold: Bool, italic: Bool, tw: Int, th: Int) -> ([UInt8], Bool) {
        let font = fonts.resolve(text, base: fonts.font(bold: bold, italic: italic))
        let attrs: [NSAttributedString.Key: Any] = [kCTFontAttributeName as NSAttributedString.Key: font,
                                                    kCTForegroundColorFromContextAttributeName as NSAttributedString.Key: true]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attrs))
        // Detect color glyph runs (Apple Color Emoji).
        var isColor = false
        let runs = CTLineGetGlyphRuns(line) as! [CTRun]
        for run in runs {
            let ra = CTRunGetAttributes(run) as NSDictionary
            if let f = ra[kCTFontAttributeName] {
                let traits = CTFontGetSymbolicTraits(f as! CTFont)
                if traits.contains(.traitColorGlyphs) { isColor = true }
            }
        }
        let cs = CGColorSpaceCreateDeviceRGB()
        var out = [UInt8](repeating: 0, count: tw * th * 4)
        if isColor {
            out.withUnsafeMutableBytes { raw in
                guard let ctx = CGContext(data: raw.baseAddress, width: tw, height: th, bitsPerComponent: 8, bytesPerRow: tw * 4, space: cs,
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return }
                // Scale the emoji to fit inside the tile.
                let bounds = CTLineGetBoundsWithOptions(line, [])
                let sx = bounds.width > CGFloat(tw) ? CGFloat(tw) / bounds.width : 1
                let sy = bounds.height > CGFloat(th) ? CGFloat(th) / bounds.height : 1
                let s = min(sx, sy)
                ctx.scaleBy(x: s, y: s)
                ctx.textPosition = CGPoint(x: max(0, (CGFloat(tw) / s - bounds.width) / 2 - bounds.minX), y: fonts.baseline / s)
                CTLineDraw(line, ctx)
            }
            return (out, true)
        }
        var gray = [UInt8](repeating: 0, count: tw * th * 4)
        gray.withUnsafeMutableBytes { raw in
            guard let ctx = CGContext(data: raw.baseAddress, width: tw, height: th, bitsPerComponent: 8, bytesPerRow: tw * 4, space: cs,
                                      bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return }
            ctx.setFillColor(gray: 0, alpha: 1)
            ctx.fill(CGRect(x: 0, y: 0, width: tw, height: th))
            ctx.setShouldAntialias(true)
            ctx.setShouldSmoothFonts(true)
            ctx.setShouldSubpixelPositionFonts(false)
            ctx.setShouldSubpixelQuantizeFonts(false)
            ctx.setFillColor(gray: 1, alpha: 1)
            // Powerline separators (U+E0B0–E0BF) are stretched to fill the whole cell, edge to edge.
            if let u = text.unicodeScalars.first, text.unicodeScalars.count == 1, (0xE0B0...0xE0BF).contains(u.value) {
                let b = CTLineGetBoundsWithOptions(line, .useGlyphPathBounds)
                if b.width > 0, b.height > 0 {
                    ctx.scaleBy(x: CGFloat(tw) / b.width, y: CGFloat(th) / b.height)
                    ctx.textPosition = CGPoint(x: -b.minX, y: -b.minY)
                    CTLineDraw(line, ctx)
                    return
                }
            }
            // Private-use icons (Nerd Font): fit inside the tile and centre, never clip.
            if let u = text.unicodeScalars.first, text.unicodeScalars.count == 1, (0xE000...0xF8FF).contains(u.value) {
                let b = CTLineGetBoundsWithOptions(line, .useGlyphPathBounds)
                if b.width > 0, b.height > 0 {
                    let s = min(CGFloat(tw) * 0.8 / b.width, CGFloat(th) * 0.68 / b.height, 1.0)
                    ctx.scaleBy(x: s, y: s)
                    ctx.textPosition = CGPoint(x: (CGFloat(tw) / s - b.width) / 2 - b.minX, y: (CGFloat(th) / s - b.height) / 2 - b.minY)
                    CTLineDraw(line, ctx)
                    return
                }
            }
            // A ligature run is shaped as one line; stretch it to exactly the cells it covers so a character
            // spacing setting (or rounding of the cell width) cannot make it drift.
            if text.unicodeScalars.count >= 2, text.unicodeScalars.allSatisfy({ $0.value < 128 }) {
                let w = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
                if w > 0, abs(w - CGFloat(tw)) > 0.5 { ctx.scaleBy(x: CGFloat(tw) / w, y: 1) }
            }
            ctx.textPosition = CGPoint(x: 0, y: fonts.baseline)
            CTLineDraw(line, ctx)
        }
        // White-on-black coverage -> premultiplied white with alpha = coverage.
        var i = 0
        while i < out.count {
            let c = gray[i]
            out[i] = c; out[i + 1] = c; out[i + 2] = c; out[i + 3] = c
            i += 4
        }
        return (out, false)
    }
}
