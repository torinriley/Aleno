import AppKit

/// Recursive split-tree layout model. A node is either a leaf (one terminal pane) or a split of two nodes.
class PaneNode {
    weak var parent: SplitNode?
}

final class LeafNode: PaneNode {
    let view: TerminalView
    init(view: TerminalView) { self.view = view }
}

final class SplitNode: PaneNode {
    /// true: children side by side (a | b). false: stacked (a over b).
    var sideBySide: Bool
    var ratio: CGFloat
    var a: PaneNode { didSet { a.parent = self } }
    var b: PaneNode { didSet { b.parent = self } }
    let divider = DividerView()
    init(sideBySide: Bool, ratio: CGFloat, a: PaneNode, b: PaneNode) {
        self.sideBySide = sideBySide; self.ratio = ratio; self.a = a; self.b = b
        super.init()
        a.parent = self; b.parent = self
        divider.node = self
    }
}

final class Tab {
    var root: PaneNode
    var focused: LeafNode
    var customTitle: String?
    init(root: PaneNode, focused: LeafNode) { self.root = root; self.focused = focused }

    var leaves: [LeafNode] { Tab.leaves(of: root) }
    static func leaves(of n: PaneNode) -> [LeafNode] {
        if let l = n as? LeafNode { return [l] }
        let s = n as! SplitNode
        return leaves(of: s.a) + leaves(of: s.b)
    }

    var title: String {
        if let c = customTitle, !c.isEmpty { return c }
        let t = focused.view.title
        return t
    }
}

/// Thin separator between two panes; draggable.
final class DividerView: NSView {
    weak var node: SplitNode?
    var onChange: (() -> Void)?
    private var tracking = false
    /// True while a divider is being dragged; the size badge only appears for resizes the user is making.
    static private(set) var isDragging = false

    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        guard let n = node else { return }
        if n.sideBySide { NSRect(x: bounds.midX - 0.5, y: 0, width: 1, height: bounds.height).fill() }
        else { NSRect(x: 0, y: bounds.midY - 0.5, width: bounds.width, height: 1).fill() }
    }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: (node?.sideBySide ?? true) ? .resizeLeftRight : .resizeUpDown)
    }
    override func mouseDown(with event: NSEvent) { tracking = true; DividerView.isDragging = true }
    override func mouseUp(with event: NSEvent) { tracking = false; DividerView.isDragging = false }
    override func mouseDragged(with event: NSEvent) {
        guard tracking, let n = node, let sv = superview else { return }
        let p = sv.convert(event.locationInWindow, from: nil)
        let f = splitFrame ?? sv.bounds
        let r: CGFloat
        if n.sideBySide { r = (p.x - f.minX) / max(1, f.width) } else { r = 1 - (p.y - f.minY) / max(1, f.height) }
        n.ratio = min(max(r, 0.08), 0.92)
        onChange?()
    }
    var splitFrame: NSRect?
    override var mouseDownCanMoveWindow: Bool { false }
}



/// What is needed to bring a closed tab back: its split layout, each pane's folder, and each pane's screen and history.
indirect enum ClosedNode {
    case leaf(cwd: String, seed: String)
    case split(sideBySide: Bool, ratio: CGFloat, a: ClosedNode, b: ClosedNode)
}

struct ClosedTab {
    var root: ClosedNode
    var customTitle: String?
    /// Index of the focused pane among the tab's leaves, in layout order.
    var focusedIndex: Int
}
