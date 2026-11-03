import AppKit

/// The brand orange (the square in the app icon), for the active-tab marker.
extension NSColor {
  static let webtabsAccent = NSColor(name: nil) { appearance in
    appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
      ? NSColor(srgbRed: 1, green: 0x6A / 255, blue: 0x2B / 255, alpha: 1)
      : NSColor(srgbRed: 0xF0 / 255, green: 0x52 / 255, blue: 0x0F / 255, alpha: 1)
  }
  /// Unread bubbles: white text on it passes WCAG AA at 10 pt bold.
  static let webtabsBadge = NSColor(srgbRed: 0xD9 / 255, green: 0x30 / 255, blue: 0x36 / 255, alpha: 1)
}

/// Title bar geometry for an icon size: the tile is the icon plus 10 pt, with
/// room under it for the active marker. Icons over 24 pt get the taller
/// (52 pt) unified title bar; the rest fit the compact 40 pt one.
struct BarMetrics: Equatable {
  let icon: CGFloat
  let height: CGFloat
  let tile: NSRect

  init(iconSize: Int) {
    icon = CGFloat(iconSize)
    height = iconSize > 24 ? 52 : 40
    let side = icon + 10
    tile = NSRect(x: 0, y: floor((height - side - 4) / 2) + 1, width: side, height: side)
  }
  var tall: Bool { height > 40 }
  var radius: CGFloat { round(tile.width / 4) }
}

/// Title bar colours (`tint` in tabs.toml): soft presets that follow light and
/// dark mode, or one custom colour used as is.
enum Tint {
  private static let presets: [String: (light: UInt32, dark: UInt32)] = [
    "graphite": (0xE4E5E8, 0x2B2D31),
    "sand": (0xEFE7DA, 0x332E27),
    "sage": (0xE1EADF, 0x273029),
    "sky": (0xDDE8F3, 0x24303C),
    "lavender": (0xE7E2F2, 0x2E2A38),
    "rose": (0xF2E1E4, 0x382A2D),
  ]

  static func rgb(_ value: UInt32) -> NSColor {
    NSColor(
      srgbRed: CGFloat(value >> 16 & 255) / 255, green: CGFloat(value >> 8 & 255) / 255,
      blue: CGFloat(value & 255) / 255, alpha: 1)
  }

  /// The bar colour; nil for the system's own.
  static func color(_ tint: String?) -> NSColor? {
    guard let tint else { return nil }
    if let p = presets[tint] {
      return NSColor(name: nil) { appearance in
        rgb(appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? p.dark : p.light)
      }
    }
    guard tint.hasPrefix("#"), let value = UInt32(tint.dropFirst(), radix: 16) else { return nil }
    return rgb(value)
  }

  /// A custom colour picks light or dark text for itself; presets follow the system.
  static func appearance(_ tint: String?) -> NSAppearance? {
    guard let tint, presets[tint] == nil, let color = color(tint)?.usingColorSpace(.sRGB) else {
      return nil
    }
    let luminance =
      0.2126 * color.redComponent + 0.7152 * color.greenComponent + 0.0722 * color.blueComponent
    return NSAppearance(named: luminance < 0.5 ? .darkAqua : .aqua)
  }

  static func hex(_ color: NSColor) -> String? {
    guard let c = color.usingColorSpace(.sRGB) else { return nil }
    let v = [c.redComponent, c.greenComponent, c.blueComponent].map {
      Int((min(max($0, 0), 1) * 255).rounded())
    }
    return String(format: "#%02x%02x%02x", v[0], v[1], v[2])
  }
}

/// The tab bar, laid into the macOS title bar (the window's content runs under
/// a transparent one). Empty space drags the window and a double-click zooms
/// or minimizes, as the user set for real title bars.
final class TitleBar: NSView {
  var color: NSColor = .windowBackgroundColor {
    didSet { needsDisplay = true }
  }

  override func draw(_ dirtyRect: NSRect) {
    color.setFill()
    bounds.fill()
  }
  override var mouseDownCanMoveWindow: Bool { true }
  override func mouseDown(with event: NSEvent) {
    guard event.clickCount == 2 else { return super.mouseDown(with: event) }
    switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
    case "Minimize": window?.performMiniaturize(nil)
    case "None": break
    default: window?.performZoom(nil)
    }
  }
}

/// A flat rounded tile that lights up on hover and while active. Tabs, the
/// bar's + and settings, presets and the settings row buttons are all tiles.
class Tile: NSView {
  var onClick: (() -> Void)?
  var radius: CGFloat = 10
  /// Where the highlight is drawn; the whole view unless set.
  var backgroundRect: NSRect?
  /// A tint shown even at rest (chips), under the hover/press highlight.
  var restingAlpha: CGFloat = 0
  var isActive = false {
    didSet { needsDisplay = true }
  }
  var isEnabled = true {
    didSet {
      alphaValue = isEnabled ? 1 : 0.35
      needsDisplay = true
      window?.invalidateCursorRects(for: self)
    }
  }
  private var hovering = false {
    didSet { needsDisplay = true }
  }
  private var pressed = false {
    didSet { needsDisplay = true }
  }
  private var tracking: NSTrackingArea?
  private var width: NSLayoutConstraint!
  private var height: NSLayoutConstraint!

  init(size: NSSize) {
    super.init(frame: NSRect(origin: .zero, size: size))
    translatesAutoresizingMaskIntoConstraints = false
    width = widthAnchor.constraint(equalToConstant: size.width)
    height = heightAnchor.constraint(equalToConstant: size.height)
    NSLayoutConstraint.activate([width, height])
  }

  func resize(to size: NSSize) {
    width.constant = size.width
    height.constant = size.height
  }
  required init?(coder: NSCoder) { fatalError() }

  override var isFlipped: Bool { true }
  // The icon and labels inside are decoration; the tile takes every click.
  override func hitTest(_ point: NSPoint) -> NSView? {
    !isHidden && frame.contains(point) ? self : nil
  }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    if let tracking { removeTrackingArea(tracking) }
    let area = NSTrackingArea(
      rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self)
    addTrackingArea(area)
    tracking = area
  }
  override func mouseEntered(with event: NSEvent) { hovering = true }
  override func mouseExited(with event: NSEvent) { hovering = false }
  // A pointing hand, like Linux's `hand_cursor`.
  override func resetCursorRects() {
    if isEnabled { addCursorRect(bounds, cursor: .pointingHand) }
  }

  override func mouseDown(with event: NSEvent) {
    if event.modifierFlags.contains(.control), let menu = menu(for: event) {
      NSMenu.popUpContextMenu(menu, with: event, for: self)
      return
    }
    pressed = isEnabled
  }
  override func mouseDragged(with event: NSEvent) {
    pressed = isEnabled && bounds.contains(convert(event.locationInWindow, from: nil))
  }
  override func mouseUp(with event: NSEvent) {
    let clicked = pressed && bounds.contains(convert(event.locationInWindow, from: nil))
    pressed = false
    if clicked { onClick?() }
  }

  override func draw(_ dirtyRect: NSRect) {
    let alpha: CGFloat =
      restingAlpha + (pressed ? 0.12 : isActive ? 0.08 : (hovering && isEnabled) ? 0.05 : 0)
    guard alpha > 0 else { return }
    NSColor.labelColor.withAlphaComponent(alpha).setFill()
    NSBezierPath(roundedRect: backgroundRect ?? bounds, xRadius: radius, yRadius: radius).fill()
  }

  // Keyboard: reachable with Full Keyboard Access; Space or Return presses it.
  override var acceptsFirstResponder: Bool { isEnabled && NSApp.isFullKeyboardAccessEnabled }
  override var canBecomeKeyView: Bool { acceptsFirstResponder }
  override func keyDown(with event: NSEvent) {
    if [" ", "\r"].contains(event.charactersIgnoringModifiers ?? "") {
      if isEnabled { onClick?() }
    } else {
      super.keyDown(with: event)
    }
  }
  override var focusRingMaskBounds: NSRect { backgroundRect ?? bounds }
  override func drawFocusRingMask() {
    NSBezierPath(roundedRect: backgroundRect ?? bounds, xRadius: radius, yRadius: radius).fill()
  }

  override func isAccessibilityElement() -> Bool { true }
  override func accessibilityRole() -> NSAccessibility.Role? { .button }
  override func isAccessibilityEnabled() -> Bool { isEnabled }
  override func accessibilityPerformPress() -> Bool {
    guard isEnabled, let onClick else { return false }
    onClick()
    return true
  }
}

/// A tile showing one SF Symbol: the bar's + and gear, settings row actions.
final class SymbolTile: Tile {
  let imageView = NSImageView()

  init(
    _ symbol: String, label: String, tooltip: String? = nil,
    size: NSSize = NSSize(width: 40, height: 38), pointSize: CGFloat = 18
  ) {
    super.init(size: size)
    imageView.translatesAutoresizingMaskIntoConstraints = false
    imageView.symbolConfiguration = NSImage.SymbolConfiguration(
      pointSize: pointSize, weight: .regular)
    imageView.contentTintColor = NSColor.labelColor.withAlphaComponent(0.85)
    addSubview(imageView)
    NSLayoutConstraint.activate([
      imageView.centerXAnchor.constraint(equalTo: centerXAnchor),
      imageView.centerYAnchor.constraint(equalTo: centerYAnchor),
    ])
    setSymbol(symbol, label: label, tooltip: tooltip)
  }
  required init?(coder: NSCoder) { fatalError() }

  func setSymbol(_ symbol: String, label: String, tooltip: String?) {
    imageView.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
    toolTip = tooltip ?? label
    setAccessibilityLabel(label)
  }
}

/// A tab in the title bar: the site icon in a tile, the unread bubble
/// top-right, an optional tag bottom-left (to tell same-service tabs apart) and,
/// when active, a short orange marker under the tile.
final class TabButton: Tile {
  /// What new tabs get; App changes it with the icon size and tint settings.
  static var metrics = BarMetrics(iconSize: 22)
  static var barColor: NSColor = .windowBackgroundColor
  let icon = NSImageView()
  /// Rings in the bar's colour, so bubble and tag sit cut into the icon.
  let badge = Pill(
    color: .webtabsBadge, font: .boldSystemFont(ofSize: 10), height: 16, padding: 4, ringWidth: 2)
  let tagPill = Pill(
    color: .labelColor, font: .systemFont(ofSize: 8, weight: .heavy), height: 13, padding: 3,
    radius: 4, ringWidth: 1.5)
  var onMenu: (() -> NSMenu?)?
  var onMiddleClick: (() -> Void)?
  private(set) var metrics = TabButton.metrics
  private var placement: [NSLayoutConstraint] = []

  init() {
    super.init(size: NSSize(width: Self.metrics.tile.width, height: Self.metrics.height))
    icon.imageScaling = .scaleProportionallyUpOrDown
    for view in [icon, badge, tagPill] as [NSView] {
      view.translatesAutoresizingMaskIntoConstraints = false
      addSubview(view)
    }
    apply(Self.metrics)
    setBarColor(Self.barColor)
  }
  required init?(coder: NSCoder) { fatalError() }

  func apply(_ m: BarMetrics) {
    metrics = m
    resize(to: NSSize(width: m.tile.width, height: m.height))
    radius = m.radius
    backgroundRect = m.tile
    NSLayoutConstraint.deactivate(placement)
    placement = [
      icon.widthAnchor.constraint(equalToConstant: m.icon),
      icon.heightAnchor.constraint(equalToConstant: m.icon),
      icon.centerXAnchor.constraint(equalTo: leadingAnchor, constant: m.tile.midX),
      icon.centerYAnchor.constraint(equalTo: topAnchor, constant: m.tile.midY),
      badge.trailingAnchor.constraint(equalTo: leadingAnchor, constant: m.tile.maxX + 6),
      badge.topAnchor.constraint(equalTo: topAnchor, constant: m.tile.minY - 3),
      tagPill.leadingAnchor.constraint(equalTo: leadingAnchor, constant: m.tile.minX),
      tagPill.bottomAnchor.constraint(equalTo: topAnchor, constant: m.tile.maxY),
    ]
    NSLayoutConstraint.activate(placement)
    needsDisplay = true
  }

  func setBarColor(_ color: NSColor) {
    badge.ring = color
    tagPill.ring = color
    tagPill.textColor = color
  }

  override var isActive: Bool {
    didSet { NSAccessibility.post(element: self, notification: .valueChanged) }
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    guard isActive else { return }
    NSColor.webtabsAccent.setFill()
    NSBezierPath(
      roundedRect: NSRect(x: metrics.tile.midX - 7, y: metrics.tile.maxY + 1, width: 14, height: 3),
      xRadius: 1.5, yRadius: 1.5
    ).fill()
  }
  override func menu(for event: NSEvent) -> NSMenu? { onMenu?() }
  override func otherMouseDown(with event: NSEvent) {
    if event.buttonNumber == 2 { onMiddleClick?() } else { super.otherMouseDown(with: event) }
  }
  // VoiceOver reads the bar like a tab group (as NSTabView does).
  override func accessibilityRole() -> NSAccessibility.Role? { .radioButton }
  override func accessibilityValue() -> Any? { isActive ? 1 : 0 }
}

/// A small rounded label: the unread bubble and the icon tag.
final class Pill: NSView {
  var text = "" {
    didSet {
      isHidden = text.isEmpty
      invalidateIntrinsicContentSize()
      needsDisplay = true
    }
  }
  private let color: NSColor
  private let font: NSFont
  private let height: CGFloat
  private let padding: CGFloat
  private let radius: CGFloat
  var textColor: NSColor {
    didSet { needsDisplay = true }
  }
  /// An outline in the background's colour around the pill (0 = none).
  var ring: NSColor {
    didSet { needsDisplay = true }
  }
  private let ringWidth: CGFloat

  init(
    color: NSColor, font: NSFont, height: CGFloat, padding: CGFloat, radius: CGFloat? = nil,
    textColor: NSColor = .white, ring: NSColor = .windowBackgroundColor, ringWidth: CGFloat = 0
  ) {
    (self.color, self.font, self.height, self.padding) = (color, font, height, padding)
    self.radius = radius ?? height / 2
    (self.textColor, self.ring, self.ringWidth) = (textColor, ring, ringWidth)
    super.init(frame: .zero)
    isHidden = true
  }
  required init?(coder: NSCoder) { fatalError() }

  override var intrinsicContentSize: NSSize {
    let width = (text as NSString).size(withAttributes: [.font: font]).width
    return NSSize(
      width: max(height, ceil(width) + 2 * padding) + 2 * ringWidth, height: height + 2 * ringWidth)
  }
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
  override func draw(_ dirtyRect: NSRect) {
    if ringWidth > 0 {
      ring.setFill()
      NSBezierPath(
        roundedRect: bounds, xRadius: radius + ringWidth, yRadius: radius + ringWidth
      ).fill()
    }
    let pill = bounds.insetBy(dx: ringWidth, dy: ringWidth)
    color.setFill()
    NSBezierPath(roundedRect: pill, xRadius: radius, yRadius: radius).fill()
    let s = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: textColor])
    // Center the digits optically (cap height), not the whole line box.
    let baseline = pill.minY + (pill.height - font.capHeight) / 2
    s.draw(at: NSPoint(x: pill.midX - s.size().width / 2, y: baseline + font.descender))
  }
}

/// The horizontal tab strip. Overflowing tabs scroll; a plain mouse wheel scrolls
/// it sideways, like GTK's scrolled window.
final class TabStrip: NSScrollView {
  override func scrollWheel(with event: NSEvent) {
    guard abs(event.scrollingDeltaY) > abs(event.scrollingDeltaX), let doc = documentView else {
      return super.scrollWheel(with: event)
    }
    let clip = contentView
    let maxX = doc.frame.width - clip.bounds.width
    guard maxX > 0 else { return }
    let step = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY : event.scrollingDeltaY * 12
    clip.scroll(to: NSPoint(x: min(max(clip.bounds.origin.x - step, 0), maxX), y: 0))
    reflectScrolledClipView(clip)
  }
}

/// A view whose subviews lay out top-down (for scrolling lists).
final class FlippedView: NSView {
  override var isFlipped: Bool { true }
}

/// A menu item that runs a closure.
final class ActionItem: NSMenuItem {
  private let handler: () -> Void

  init(_ title: String, enabled: Bool = true, _ handler: @escaping () -> Void) {
    self.handler = handler
    super.init(title: title, action: #selector(run), keyEquivalent: "")
    target = self
    isEnabled = enabled
  }
  required init(coder: NSCoder) { fatalError() }

  @objc private func run() { handler() }
}
