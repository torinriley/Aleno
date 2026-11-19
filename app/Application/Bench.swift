import AppKit
import QuartzCore

/// Lightweight runtime measurements, enabled with ALENO_BENCH=1 (prints to stderr every 2 s while frames are drawn).
enum Bench {
    static let on = ProcessInfo.processInfo.environment["ALENO_BENCH"] != nil
    private static var cpu: [Double] = []
    private static var syncT: [Double] = []
    private static var gpu: [Double] = []
    private static var present: [Double] = []
    private static var rows = 0
    private static var lastReport = CACurrentMediaTime()
    private static var firstFrameLogged = false
    static var windowStart: CFTimeInterval?

    /// Milliseconds since this process was started (from the kernel's record).
    static func msSinceProcessStart() -> Double {
        var info = kinfo_proc(); var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
        guard sysctl(&mib, 4, &info, &size, nil, 0) == 0 else { return 0 }
        let t = info.kp_proc.p_un.__p_starttime
        let start = Double(t.tv_sec) + Double(t.tv_usec) / 1e6
        return (Date().timeIntervalSince1970 - start) * 1000
    }

    static func mark(_ name: String) {
        guard on else { return }
        FileHandle.standardError.write(Data(String(format: "BENCH phase %-28@ %.0f ms since process start\n", name as NSString, msSinceProcessStart()).utf8))
    }

    static func firstFrame() {
        guard on else { return }
        if !firstFrameLogged {
            firstFrameLogged = true
            FileHandle.standardError.write(Data(String(format: "BENCH launch->first frame: %.1f ms\n", msSinceProcessStart()).utf8))
        }
        if let w = windowStart {
            windowStart = nil
            FileHandle.standardError.write(Data(String(format: "BENCH new window -> first frame: %.1f ms\n", (CACurrentMediaTime() - w) * 1000).utf8))
        }
    }

    static func recordCPU(_ ms: Double, syncMs: Double, rebuiltRows r: Int) {
        guard on else { return }
        cpu.append(ms); syncT.append(syncMs); rows += r
        let now = CACurrentMediaTime()
        if now - lastReport >= 2 { report(now) }
    }
    static func recordGPU(_ ms: Double) { if on { DispatchQueue.main.async { gpu.append(ms) } } }
    static func recordPresent(_ ms: Double) { if on { DispatchQueue.main.async { present.append(ms) } } }

    private static func pct(_ v: [Double], _ p: Double) -> Double {
        guard !v.isEmpty else { return 0 }
        let s = v.sorted(); return s[Int(Double(s.count - 1) * p)]
    }

    private static func report(_ now: CFTimeInterval) {
        let secs = now - lastReport
        let line = String(format: "BENCH %.0f fps | frame total ms p50 %.2f p99 %.2f max %.2f | of which terminal sync+instance build p50 %.2f p99 %.2f | GPU ms p50 %.2f p99 %.2f | draw->on-screen ms p50 %.1f p99 %.1f | rows rebuilt/frame %.1f\n",
                          Double(cpu.count) / secs, pct(cpu, 0.5), pct(cpu, 0.99), pct(cpu, 1), pct(syncT, 0.5), pct(syncT, 0.99), pct(gpu, 0.5), pct(gpu, 0.99), pct(present, 0.5), pct(present, 0.99),
                          Double(rows) / Double(max(1, cpu.count)))
        FileHandle.standardError.write(Data(line.utf8))
        cpu.removeAll(); syncT.removeAll(); gpu.removeAll(); present.removeAll(); rows = 0
        lastReport = now
    }
}
