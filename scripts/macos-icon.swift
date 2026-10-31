import AppKit

let output = CommandLine.arguments[1]
// Package the approved artwork without redrawing or changing its proportions.
let sourceURL = URL(fileURLWithPath: #filePath)
  .deletingLastPathComponent().deletingLastPathComponent()
  .appendingPathComponent("assets/webtabs-macos.png")
guard let artwork = NSImage(contentsOf: sourceURL) else {
  fatalError("Cannot load app icon: \(sourceURL.path)")
}
let fm = FileManager.default
try fm.createDirectory(atPath: output, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
  for scale in [1, 2] {
    let pixels = size * scale
    let bitmap = NSBitmapImageRep(
      bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
      samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
      bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    NSGraphicsContext.current?.imageInterpolation = .high
    artwork.draw(
      in: NSRect(x: 0, y: 0, width: pixels, height: pixels),
      from: .zero, operation: .copy, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    try bitmap.representation(using: .png, properties: [:])!.write(
      to: URL(fileURLWithPath: "\(output)/icon_\(size)x\(size)\(scale == 2 ? "@2x" : "").png"))
  }
}
