import AppKit

let output = CommandLine.arguments[1]
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
    let transform = NSAffineTransform()
    transform.scale(by: CGFloat(pixels) / 128)
    transform.concat()
    let background = NSBezierPath(
      roundedRect: NSRect(x: 8, y: 8, width: 112, height: 112), xRadius: 26, yRadius: 26)
    NSGradient(
      starting: NSColor(red: 0.18, green: 0.37, blue: 0.85, alpha: 1),
      ending: NSColor(red: 0.31, green: 0.55, blue: 1, alpha: 1))!.draw(in: background, angle: 90)
    for x in [24, 52, 80] {
      NSColor.white.withAlphaComponent(x == 24 ? 1 : 0.55).setFill()
      NSBezierPath(roundedRect: NSRect(x: x, y: 86, width: 24, height: 12), xRadius: 4, yRadius: 4)
        .fill()
    }
    NSColor.white.setFill()
    NSBezierPath(roundedRect: NSRect(x: 24, y: 28, width: 80, height: 54), xRadius: 6, yRadius: 6)
      .fill()
    NSGraphicsContext.restoreGraphicsState()
    try bitmap.representation(using: .png, properties: [:])!.write(
      to: URL(fileURLWithPath: "\(output)/icon_\(size)x\(size)\(scale == 2 ? "@2x" : "").png"))
  }
}
