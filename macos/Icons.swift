import AppKit
import ImageIO

/// Site icons: download the best declared icon, cache it on disk (same files as
/// Linux, see `favicon::cache_path`) and draw it crisp at any scale.
enum Icons {
  /// Cached icons are stored at this size; tabs draw them at 32pt.
  static let cachePx = 128
  /// Icons are tiny; anything bigger is not an icon (or is hostile).
  static let maxBytes = 1024 * 1024
  static let maxSide = 4096

  private static let session: URLSession = {
    let c = URLSessionConfiguration.ephemeral
    c.timeoutIntervalForRequest = 15
    c.httpAdditionalHeaders = ["User-Agent": Core.userAgents.defaultAgent]
    return URLSession(configuration: c)
  }()

  static let placeholder: NSImage = {
    let config = NSImage.SymbolConfiguration(pointSize: 26, weight: .light)
    return NSImage(systemSymbolName: "globe", accessibilityDescription: nil)!
      .withSymbolConfiguration(config)!
  }()

  /// Tries each URL in order and hands the first one that decodes to `done`,
  /// on the main queue.
  static func fetch(_ urls: [String], done: @escaping (NSImage?) -> Void) {
    guard let first = urls.first else { return done(nil) }
    let next = { fetch(Array(urls.dropFirst()), done: done) }
    guard let url = URL(string: first), ["http", "https", "data"].contains(url.scheme ?? "") else {
      return next()
    }
    session.dataTask(with: url) { data, response, _ in
      let ok = (response as? HTTPURLResponse).map { $0.statusCode == 200 } ?? (url.scheme == "data")
      let image = data.flatMap { ok && $0.count <= maxBytes ? decode($0) : nil }
      DispatchQueue.main.async {
        if let image { done(image) } else { next() }
      }
    }.resume()
  }

  /// PNG/ICO/JPEG/... are downscaled while decoding (so a hostile image can't
  /// allocate gigabytes); SVGs stay vectors.
  static func decode(_ data: Data) -> NSImage? {
    if let source = CGImageSourceCreateWithData(data as CFData, nil),
      CGImageSourceGetCount(source) > 0
    {
      // ICO files hold several sizes: take the biggest sane one.
      let sides = (0..<CGImageSourceGetCount(source)).map { i -> Int in
        let p = CGImageSourceCopyPropertiesAtIndex(source, i, nil) as? [CFString: Any]
        let w = p?[kCGImagePropertyPixelWidth] as? Int ?? 0
        let h = p?[kCGImagePropertyPixelHeight] as? Int ?? 0
        return w > 0 && h > 0 && max(w, h) <= maxSide ? max(w, h) : 0
      }
      guard let best = sides.indices.max(by: { sides[$0] < sides[$1] }), sides[best] > 0 else {
        return nil
      }
      let options: [CFString: Any] = [
        kCGImageSourceThumbnailMaxPixelSize: cachePx,
        kCGImageSourceCreateThumbnailFromImageAlways: true,
        kCGImageSourceCreateThumbnailWithTransform: true,
      ]
      guard let cg = CGImageSourceCreateThumbnailAtIndex(source, best, options as CFDictionary)
      else { return nil }
      return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
    guard let image = NSImage(data: data), image.isValid, image.size.width > 0,
      image.size.height > 0
    else { return nil }
    return image
  }

  /// `image` centered in a transparent `px`×`px` square, keeping its aspect ratio.
  static func fitted(_ image: NSImage, px: Int) -> NSBitmapImageRep? {
    guard
      let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0),
      let context = NSGraphicsContext(bitmapImageRep: rep)
    else { return nil }
    let side = CGFloat(px)
    let k = side / max(image.size.width, image.size.height)
    let (w, h) = (image.size.width * k, image.size.height * k)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    image.draw(in: NSRect(x: (side - w) / 2, y: (side - h) / 2, width: w, height: h))
    NSGraphicsContext.restoreGraphicsState()
    return rep
  }

  static func cached(_ pageURL: String) -> NSImage? {
    Core.iconCachePath(pageURL).flatMap { NSImage(contentsOfFile: $0) }
  }

  static func store(_ pageURL: String, _ image: NSImage) {
    guard let path = Core.iconCachePath(pageURL),
      let png = fitted(image, px: cachePx)?.representation(using: .png, properties: [:])
    else { return }
    let url = URL(fileURLWithPath: path)
    do {
      try FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
      try png.write(to: url, options: .atomic)
    } catch {
      NSLog("webtabs: cannot cache icon %@: %@", path, error.localizedDescription)
    }
  }
}
