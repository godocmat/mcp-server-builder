import CoreGraphics
import Darwin
import Foundation

guard CommandLine.arguments.count == 2, let pid = Int32(CommandLine.arguments[1]), pid > 0 else {
    fputs("Expected the installed application PID.\n", stderr)
    exit(2)
}
let end = Date().addingTimeInterval(30)
while Date() < end {
    if kill(pid, 0) != 0 {
        fputs("The installed application exited before opening its window.\n", stderr)
        exit(1)
    }
    // Window owner, layer and bounds are available without screen recording permission.
    let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
    if windows.contains(where: { window in
        guard window[kCGWindowOwnerPID as String] as? Int == Int(pid),
              window[kCGWindowLayer as String] as? Int == 0,
              let bounds = window[kCGWindowBounds as String] as? NSDictionary,
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary)
        else { return false }
        return rect.width > 0 && rect.height > 0
    }) {
        print("The installed application opened a native window.")
        exit(0)
    }
    Thread.sleep(forTimeInterval: 0.25)
}
fputs("Timed out waiting for the installed application window.\n", stderr)
exit(1)
