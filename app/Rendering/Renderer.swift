import Metal
import MetalKit
import simd

/// Mirrors `CellInstance` in Terminal.metal (24 bytes).
struct CellInstance {
    var x: UInt16 = 0, y: UInt16 = 0
    var flags: UInt16 = 0
    var pad: UInt16 = 0
    var fg: UInt32 = 0
    var bg: UInt32 = 0
    var rx: UInt16 = 0, ry: UInt16 = 0, rw: UInt16 = 0, rh: UInt16 = 0
}

enum CellFlag {
    static let glyph: UInt16 = 1, color: UInt16 = 2, wide: UInt16 = 4, underline: UInt16 = 8, strike: UInt16 = 16
    static let doubleUnderline: UInt16 = 32, selected: UInt16 = 64, match: UInt16 = 128, current: UInt16 = 256
    static let dim: UInt16 = 512, skip: UInt16 = 1024, noExtendVertical: UInt16 = 2048
}

/// Mirrors `Uniforms` in Terminal.metal.
struct TerminalUniforms {
    var viewport = SIMD2<Float>(0, 0)
    var origin = SIMD2<Float>(0, 0)
    var cell = SIMD2<Float>(0, 0)
    var atlasSize = SIMD2<Float>(Float(GlyphAtlas.size), Float(GlyphAtlas.size))
    var cursorColor = SIMD4<Float>(1, 1, 1, 1)
    var cursorTextColor = SIMD4<Float>(0, 0, 0, 1)
    var selectionColor = SIMD4<Float>(0.3, 0.4, 0.7, 1)
    var cursorPos = SIMD2<UInt32>(0, 0)
    var cursorMode: UInt32 = 0
    var gridCols: UInt32 = 0
    var scale: Float = 2
    var underlinePos: Float = 0
    var underlineThickness: Float = 1
    var gridRows: UInt32 = 0
}

/// Process-wide Metal objects.
final class MetalContext {
    static let shared = MetalContext()
    let device: MTLDevice
    let queue: MTLCommandQueue
    let pipeline: MTLRenderPipelineState
    private var atlases: [String: GlyphAtlas] = [:]
    private var fontSets: [String: FontSet] = [:]

    private init() {
        device = MTLCreateSystemDefaultDevice()!
        queue = device.makeCommandQueue()!
        let lib = device.makeDefaultLibrary()!
        let d = MTLRenderPipelineDescriptor()
        d.vertexFunction = lib.makeFunction(name: "cell_vertex")
        d.fragmentFunction = lib.makeFunction(name: "cell_fragment")
        d.colorAttachments[0].pixelFormat = .bgra8Unorm
        d.label = "Aleno cells"
        pipeline = try! device.makeRenderPipelineState(descriptor: d)
    }

    func atlas(for s: Settings, scale: CGFloat) -> GlyphAtlas {
        let key = "\(s.font.family)|\(s.font.size)|\(s.font.lineHeight)|\(s.font.characterSpacing)|\(scale)"
        if let a = atlases[key] { return a }
        // Only keep atlases for the current configuration: drop stale ones (font changed).
        atlases.removeAll()
        let fs = FontSet(family: s.font.family, size: s.font.size, lineHeight: s.font.lineHeight, spacing: s.font.characterSpacing, scale: scale)
        let a = GlyphAtlas(device: device, fonts: fs)
        atlases[key] = a
        return a
    }
}

/// Per-pane render state: persistent cell mirror, instance buffer, dirty tracking.
final class PaneRenderer {
    let ctx = MetalContext.shared
    private(set) var atlas: GlyphAtlas
    var theme: Theme
    var settings: Settings

    private(set) var cols = 0, rows = 0
    private var cells: [AlenoCell] = []
    private var dirty: [UInt8] = []
    private var instanceBuffer: MTLBuffer?
    private var uniforms = TerminalUniforms()
    private var atlasGeneration = -1
    /// ASCII characters that programming fonts combine into ligatures.
    static let ligatureChars: [Bool] = {
        var t = [Bool](repeating: false, count: 128)
        for u in "-<>=!|&:*+./#_~?\\^%@$;{}[]()".unicodeScalars { t[Int(u.value)] = true }
        return t
    }()

    private var scalarCache: [UInt64: AtlasEntry] = [:]
    private var clusterCache: [UInt64: AtlasEntry] = [:]
    var forceFullRebuild = true
    /// Points reserved at the top for the title bar / tab strip (the terminal's background still extends behind it).
    var topInsetPoints: CGFloat = 0
    private var extraDirtyRows = Set<Int>()

    /// Selection / search highlighting supplied by the view (absolute rows).
    var highlightProvider: ((_ absRow: Int, _ col: Int) -> UInt16)?

    private(set) var info = AlenoViewInfo()
    private(set) var rebuiltRows = 0

    init(settings: Settings, theme: Theme, scale: CGFloat) {
        self.settings = settings
        self.theme = theme
        self.atlas = MetalContext.shared.atlas(for: settings, scale: scale)
    }

    var fonts: FontSet { atlas.fonts }

    func reconfigure(settings: Settings, theme: Theme, scale: CGFloat) {
        self.settings = settings
        self.theme = theme
        let a = ctx.atlas(for: settings, scale: scale)
        if a !== atlas { atlas = a; atlasGeneration = -1 }
        forceFullRebuild = true
    }

    func cellAt(row: Int, col: Int) -> AlenoCell? {
        guard row >= 0, row < rows, col >= 0, col < cols else { return nil }
        return cells[row * cols + col]
    }

    func markRowsDirty(_ rows: some Sequence<Int>) { for r in rows { extraDirtyRows.insert(r) } }
    func markAllDirty() { forceFullRebuild = true }

    /// Cells that fit into `size` (in pixels) given padding.
    func gridSize(forPixels size: CGSize, scale: CGFloat, topInset: CGFloat) -> (cols: Int, rows: Int) {
        Self.gridSize(forPixels: size, scale: scale, settings: settings, fonts: fonts, topInset: topInset)
    }

    static func gridSize(forPixels size: CGSize, scale: CGFloat, settings: Settings, fonts: FontSet, topInset: CGFloat) -> (cols: Int, rows: Int) {
        let pad = CGFloat(settings.appearance.padding) * scale
        let c = Int(max(1, floor((size.width - pad * 2) / CGFloat(fonts.cellWidth))))
        let r = Int(max(1, floor((size.height - pad * 2 - topInset * scale) / CGFloat(fonts.cellHeight))))
        return (min(c, 1000), min(r, 1000))
    }

    /// Absolute line index (the coordinates `aleno_pane_text` and `aleno_pane_blocks` use) shown on viewport row `r`.
    func line(forRow r: Int) -> Int { Int(info.top_abs) + r }

    /// The viewport row currently showing absolute line `l`, if any.
    func row(forLine l: Int) -> Int? {
        let r = l - Int(info.top_abs)
        return (0..<Int(info.rows)).contains(r) ? r : nil
    }

    /// Pulls the latest state from the Rust mirror and updates dirty instances. Returns false if nothing to draw yet.
    func sync(pane: OpaquePointer) {
        if atlas.generation != atlasGeneration {
            atlasGeneration = atlas.generation
            scalarCache.removeAll(); clusterCache.removeAll()
            forceFullRebuild = true
        }
        var vi = AlenoViewInfo()
        // First call sizes the buffers from the current terminal dimensions.
        aleno_pane_copy_view(pane, UInt32(cols), UInt32(rows), &cells, cells.count, &dirty, dirty.count, &vi)
        if vi.needs_realloc != 0 || cells.count != Int(vi.cols * vi.rows) {
            cols = Int(vi.cols); rows = Int(vi.rows)
            cells = [AlenoCell](repeating: AlenoCell(ch: 32, fg: 0, bg: 0, flags: 0, link: 0), count: cols * rows)
            dirty = [UInt8](repeating: 1, count: rows)
            instanceBuffer = ctx.device.makeBuffer(length: max(1, cols * rows) * MemoryLayout<CellInstance>.stride, options: .storageModeShared)
            instanceBuffer?.label = "Aleno cell instances"
            forceFullRebuild = true
            aleno_pane_copy_view(pane, UInt32(cols), UInt32(rows), &cells, cells.count, &dirty, dirty.count, &vi)
        }
        info = vi
        guard let buf = instanceBuffer, cols > 0 else { return }
        let inst = buf.contents().bindMemory(to: CellInstance.self, capacity: cols * rows)
        rebuiltRows = 0
        for r in 0..<rows {
            if forceFullRebuild || dirty[r] != 0 || extraDirtyRows.contains(r) {
                buildRow(r, into: inst, pane: pane)
                rebuiltRows += 1
            }
        }
        forceFullRebuild = false
        extraDirtyRows.removeAll(keepingCapacity: true)
    }

    private func buildRow(_ r: Int, into inst: UnsafeMutablePointer<CellInstance>, pane: OpaquePointer) {
        let base = r * cols
        let absRow = line(forRow: r)
        let defaultBgA: Float = 0   // the window's tint layer provides the default background
        // Top/bottom edge rows only extend into the padding when the whole row is a single background
        // colour (a full-screen app); a shell prompt with coloured segments must not smear upward.
        var uniformRow = true
        if r == 0 || r == rows - 1 {
            func eff(_ c: AlenoCell) -> UInt32 { c.flags & 32 != 0 ? c.fg : c.bg }
            let first = eff(cells[base])
            for c in 1..<max(cols, 1) where eff(cells[base + c]) != first && cells[base + c].flags & 512 == 0 { uniformRow = false; break }
        }
        var skipRemaining = 0
        let ligaturesOn = settings.font.ligatures
        for c in 0..<cols {
            let cell = cells[base + c]
            var i = CellInstance()
            if skipRemaining > 0 { skipRemaining -= 1; i.x = UInt16(c); i.y = UInt16(r); i.flags = CellFlag.skip; inst[base + c] = i; continue }
            i.x = UInt16(c); i.y = UInt16(r)
            let f = cell.flags
            if f & 512 != 0 { i.flags = CellFlag.skip; i.bg = 0; inst[base + c] = i; continue } // WIDE_CONT
            var fg = theme.resolve(cell.fg, isFg: true)
            var bg = theme.resolve(cell.bg, isFg: false)
            var bgIsDefault = (cell.bg & 0xFF00_0000) == 0
            if f & 32 != 0 { swap(&fg, &bg); bgIsDefault = false }
            let a: Float = bgIsDefault ? defaultBgA : 1
            i.fg = RGBA(fg).packed
            i.bg = RGBA(bg, a: a).packed
            var flags: UInt16 = 0
            if f & 8 != 0 { flags |= CellFlag.underline }
            if f & 1024 != 0 { flags |= CellFlag.doubleUnderline }
            if f & 128 != 0 { flags |= CellFlag.strike }
            if f & 2 != 0 { flags |= CellFlag.dim }
            if f & 256 != 0 { flags |= CellFlag.wide }
            if let h = highlightProvider { flags |= h(absRow, c) }
            if !uniformRow { flags |= CellFlag.noExtendVertical }
            let ch = cell.ch
            let blank = ch == 32 || ch == 0 || (f & 64 != 0)
            if !blank {
                let bold = f & 1 != 0, italic = f & 4 != 0
                var wide = f & 256 != 0
                // Icons (private use, not Powerline separators) may borrow a following blank cell of the same background.
                if !wide, (0xE000...0xF8FF).contains(ch), !(0xE0B0...0xE0BF).contains(ch), c + 1 < cols {
                    let nx = cells[base + c + 1]
                    if nx.ch == 32, nx.bg == cell.bg, nx.flags & 0x2E0 == 0 {
                        wide = true; skipRemaining = 1; flags |= CellFlag.wide
                    }
                }
                // Ligatures: a run of symbol characters with identical styling that the font shapes differently as a
                // whole is drawn as one multi-cell glyph.
                if ligaturesOn, !wide, ch < 128, PaneRenderer.ligatureChars[Int(ch)] {
                    var n = 1
                    let h0 = highlightProvider?(absRow, c) ?? 0
                    while c + n < cols, n < 12 {
                        let nx = cells[base + c + n]
                        guard nx.ch < 128, PaneRenderer.ligatureChars[Int(nx.ch)], nx.fg == cell.fg, nx.bg == cell.bg, nx.flags == cell.flags else { break }
                        if let hp = highlightProvider, hp(absRow, c + n) != h0 { break }
                        n += 1
                    }
                    if n >= 2 {
                        let t = String(decoding: (0..<n).map { UInt8(cells[base + c + $0].ch) }, as: UTF8.self)
                        if fonts.ligates(t, bold: bold, italic: italic) {
                            let gen = atlas.generation
                            let e = atlas.entry(text: t, bold: bold, italic: italic, span: n)
                            if atlas.generation != gen {
                                scalarCache.removeAll(); clusterCache.removeAll()
                                atlasGeneration = atlas.generation
                                forceFullRebuild = true
                            }
                            flags |= CellFlag.glyph
                            i.pad = UInt16(n)
                            i.rx = e.x; i.ry = e.y; i.rw = e.w; i.rh = e.h
                            skipRemaining = n - 1
                            i.flags = flags
                            inst[base + c] = i
                            continue
                        }
                    }
                }
                let styleKey = UInt64((bold ? 1 : 0) | (italic ? 2 : 0) | (wide ? 4 : 0))
                let key = (UInt64(ch) << 8) | styleKey
                let isCluster = ch & 0x8000_0000 != 0
                var e: AtlasEntry
                if let cached = (isCluster ? clusterCache[key] : scalarCache[key]) {
                    e = cached
                } else {
                    let text: String
                    if isCluster {
                        if let p = aleno_pane_cluster(pane, ch) { text = String(cString: p); aleno_string_free(p) } else { text = "?" }
                    } else {
                        text = String(UnicodeScalar(ch).map { Character($0) } ?? "\u{FFFD}")
                    }
                    let gen = atlas.generation
                    e = atlas.entry(text: text, bold: bold, italic: italic, wide: wide)
                    if atlas.generation != gen {
                        // Atlas was flushed mid-frame: stale entries everywhere, rebuild on next sync.
                        scalarCache.removeAll(); clusterCache.removeAll()
                        atlasGeneration = atlas.generation
                        forceFullRebuild = true
                    }
                    if isCluster { clusterCache[key] = e } else { scalarCache[key] = e }
                }
                flags |= CellFlag.glyph | (e.isColor ? CellFlag.color : 0)
                i.rx = e.x; i.ry = e.y; i.rw = e.w; i.rh = e.h
            }
            i.flags = flags
            inst[base + c] = i
        }
    }

    // MARK: Drawing

    /// `cursorMode`: 0 hidden, 1 block, 2 hollow, 3 bar, 4 underline.
    func draw(in view: MTKView, cursorMode: UInt32, scale: CGFloat) {
        guard let buf = instanceBuffer, cols > 0,
              let rpd = view.currentRenderPassDescriptor, let drawable = view.currentDrawable,
              let cb = ctx.queue.makeCommandBuffer() else { return }
        rpd.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        rpd.colorAttachments[0].loadAction = .clear
        guard let enc = cb.makeRenderCommandEncoder(descriptor: rpd) else { return }
        var u = uniforms
        let pad = Float(settings.appearance.padding * Double(scale))
        u.viewport = SIMD2(Float(view.drawableSize.width), Float(view.drawableSize.height))
        u.origin = SIMD2(pad, pad + Float(topInsetPoints * scale))
        u.cell = SIMD2(Float(fonts.cellWidth), Float(fonts.cellHeight))
        let cc = RGBA(theme.cursor)
        u.cursorColor = SIMD4(cc.r, cc.g, cc.b, 1)
        let tc = RGBA(theme.background)
        u.cursorTextColor = SIMD4(tc.r, tc.g, tc.b, 1)
        let sc = RGBA(theme.selection)
        u.selectionColor = SIMD4(sc.r, sc.g, sc.b, 1)
        var ccol = Int(info.cursor_col)
        if info.cursor_row < rows, ccol < cols, cells[Int(info.cursor_row) * cols + ccol].flags & 512 != 0, ccol > 0 { ccol -= 1 }
        u.cursorPos = SIMD2(UInt32(ccol), info.cursor_row)
        u.cursorMode = (info.cursor_visible != 0) ? cursorMode : 0
        u.scale = Float(scale)
        u.gridCols = UInt32(cols); u.gridRows = UInt32(rows)
        u.underlinePos = Float(fonts.underlinePosition)
        u.underlineThickness = Float(fonts.underlineThickness)
        enc.setRenderPipelineState(ctx.pipeline)
        enc.setVertexBuffer(buf, offset: 0, index: 0)
        enc.setVertexBytes(&u, length: MemoryLayout<TerminalUniforms>.size, index: 1)
        enc.setFragmentBytes(&u, length: MemoryLayout<TerminalUniforms>.size, index: 0)
        enc.setFragmentTexture(atlas.texture, index: 0)
        enc.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: cols * rows)
        enc.endEncoding()
        if Bench.on {
            let t0 = CACurrentMediaTime()
            cb.addCompletedHandler { Bench.recordGPU(($0.gpuEndTime - $0.gpuStartTime) * 1000) }
            drawable.addPresentedHandler { d in if d.presentedTime > 0 { Bench.recordPresent((d.presentedTime - t0) * 1000) } }
        }
        cb.present(drawable)
        cb.commit()
    }
}
