import AppKit

/// Hosts the active tab's split tree. Flipped so layout math reads top-to-bottom.
final class WorkspaceView: NSView {
    override var isFlipped: Bool { true }

    var tab: Tab? { didSet { refreshStructure() } }

    /// Height of the title bar / tab strip overlaying the top of the workspace.
    var chromeInset: CGFloat = 0 { didSet { applyInsets() } }

    // The terminal uses the final inset while the renderer animates to it.
    var gridChromeInset: CGFloat = TabBarView.titleRowHeight

    private func applyInsets() {
        guard let t = tab else { return }
        for l in t.leaves { l.view.topInset = l.view.frame.minY < 0.5 ? chromeInset : 0 }
    }

    private func add(_ n: PaneNode) {
        if let l = n as? LeafNode { addSubview(l.view); return }
        let s = n as! SplitNode
        add(s.a); add(s.b)
    }
    private func addDividers(_ n: PaneNode) {
        guard let s = n as? SplitNode else { return }
        addSubview(s.divider)
        s.divider.onChange = { [weak self] in self?.layoutTree() }
        addDividers(s.a); addDividers(s.b)
    }

    func refreshStructure() {
        subviews.forEach { $0.removeFromSuperview() }
        guard let t = tab else { return }
        layoutTree()
        add(t.root); addDividers(t.root)
    }

    override func layout() { super.layout(); layoutTree() }

    func layoutTree() {
        guard let t = tab else { return }
        place(t.root, in: bounds)
        applyInsets()
        for leaf in t.leaves {
            leaf.view.gridTopInset = leaf.view.frame.minY < 0.5 ? gridChromeInset : 0
        }
    }

    private func place(_ n: PaneNode, in r: NSRect) {
        if let l = n as? LeafNode {
            if l.view.frame != r { l.view.frame = r }
            return
        }
        let s = n as! SplitNode
        if s.sideBySide {
            let total = r.width - 1
            let wa = floor(total * s.ratio)
            place(s.a, in: NSRect(x: r.minX, y: r.minY, width: wa, height: r.height))
            place(s.b, in: NSRect(x: r.minX + wa + 1, y: r.minY, width: total - wa, height: r.height))
            s.divider.frame = NSRect(x: r.minX + wa - 3, y: r.minY, width: 7, height: r.height)
        } else {
            let total = r.height - 1
            let ha = floor(total * s.ratio)
            place(s.a, in: NSRect(x: r.minX, y: r.minY, width: r.width, height: ha))
            place(s.b, in: NSRect(x: r.minX, y: r.minY + ha + 1, width: r.width, height: total - ha))
            s.divider.frame = NSRect(x: r.minX, y: r.minY + ha - 3, width: r.width, height: 7)
        }
        s.divider.splitFrame = r
        s.divider.window?.invalidateCursorRects(for: s.divider)
        s.divider.needsDisplay = true
    }

    /// Frame of a leaf in workspace coordinates.
    func frame(of leaf: LeafNode) -> NSRect { leaf.view.frame }

    /// Directional focus: nearest pane whose edge lies in that direction, preferring overlap.
    func neighbor(of leaf: LeafNode, direction: Direction) -> LeafNode? {
        guard let t = tab else { return nil }
        let f = leaf.view.frame
        var best: (LeafNode, CGFloat)?
        for c in t.leaves where c !== leaf {
            let g = c.view.frame
            let overlap: CGFloat, dist: CGFloat
            switch direction {
            case .left:  guard g.maxX <= f.minX + 2 else { continue }; overlap = min(f.maxY, g.maxY) - max(f.minY, g.minY); dist = f.minX - g.maxX
            case .right: guard g.minX >= f.maxX - 2 else { continue }; overlap = min(f.maxY, g.maxY) - max(f.minY, g.minY); dist = g.minX - f.maxX
            case .up:    guard g.maxY <= f.minY + 2 else { continue }; overlap = min(f.maxX, g.maxX) - max(f.minX, g.minX); dist = f.minY - g.maxY
            case .down:  guard g.minY >= f.maxY - 2 else { continue }; overlap = min(f.maxX, g.maxX) - max(f.minX, g.minX); dist = g.minY - f.maxY
            }
            guard overlap > 0 else { continue }
            let score = dist * 1000 - overlap
            if best == nil || score < best!.1 { best = (c, score) }
        }
        return best?.0
    }

    enum Direction { case left, right, up, down }
}
