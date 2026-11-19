import AppKit

Bench.mark("main() entered")
// Warm the expensive one-time setup (Metal device + shader pipeline, fonts) while AppKit initialises.
DispatchQueue.global(qos: .userInteractive).async {
    _ = MetalContext.shared
    _ = FontSet.symbolFallbacks
    Bench.mark("prewarm done")
}
let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
