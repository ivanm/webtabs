import AppKit

/// A flat rounded tile that lights up on hover and while active: the look of the
/// Linux `.tabbar button` CSS. Tabs, the bar's + and gear, presets and the
/// settings row buttons are all tiles.
class Tile: NSView {
  var onClick: (() -> Void)?
  var radius: CGFloat = 10
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

  init(size: NSSize) {
    super.init(frame: NSRect(origin: .zero, size: size))
    translatesAutoresizingMaskIntoConstraints = false
    NSLayoutConstraint.activate([
      widthAnchor.constraint(equalToConstant: size.width),
      heightAnchor.constraint(equalToConstant: size.height),
    ])
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
      pressed ? 0.22 : isActive ? 0.16 : (hovering && isEnabled) ? 0.08 : 0
    guard alpha > 0 else { return }
    NSColor.labelColor.withAlphaComponent(alpha).setFill()
    NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
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
  override var focusRingMaskBounds: NSRect { bounds }
  override func drawFocusRingMask() {
    NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
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

/// A tab in the bar: the site icon with the unread bubble pinned top-right and an
/// optional tag bottom-left, to tell same-service tabs apart (Linux `create_tab`).
final class TabButton: Tile {
  let icon = NSImageView()
  let badge = Pill(
    color: NSColor(srgbRed: 0xe5 / 255, green: 0x48 / 255, blue: 0x4d / 255, alpha: 1),
    font: .boldSystemFont(ofSize: 10), height: 16, padding: 4)
  let tagPill = Pill(
    color: NSColor(srgbRed: 0x3b / 255, green: 0x82 / 255, blue: 0xf6 / 255, alpha: 1),
    font: .boldSystemFont(ofSize: 9), height: 13, padding: 3, radius: 5)
  var onMenu: (() -> NSMenu?)?
  var onMiddleClick: (() -> Void)?

  init() {
    super.init(size: NSSize(width: 52, height: 44))
    icon.imageScaling = .scaleProportionallyUpOrDown
    for view in [icon, badge, tagPill] as [NSView] {
      view.translatesAutoresizingMaskIntoConstraints = false
      addSubview(view)
    }
    NSLayoutConstraint.activate([
      icon.widthAnchor.constraint(equalToConstant: 32),
      icon.heightAnchor.constraint(equalToConstant: 32),
      icon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 9),
      icon.topAnchor.constraint(equalTo: topAnchor, constant: 7),
      badge.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
      badge.topAnchor.constraint(equalTo: topAnchor, constant: 3),
      tagPill.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 5),
      tagPill.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -3),
    ])
  }
  required init?(coder: NSCoder) { fatalError() }

  override var isActive: Bool {
    didSet { NSAccessibility.post(element: self, notification: .valueChanged) }
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

  init(color: NSColor, font: NSFont, height: CGFloat, padding: CGFloat, radius: CGFloat? = nil) {
    (self.color, self.font, self.height, self.padding) = (color, font, height, padding)
    self.radius = radius ?? height / 2
    super.init(frame: .zero)
    isHidden = true
  }
  required init?(coder: NSCoder) { fatalError() }

  override var intrinsicContentSize: NSSize {
    let width = (text as NSString).size(withAttributes: [.font: font]).width
    return NSSize(width: max(height, ceil(width) + 2 * padding), height: height)
  }
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
  override func draw(_ dirtyRect: NSRect) {
    color.setFill()
    NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
    let s = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: NSColor.white])
    // Center the digits optically (cap height), not the whole line box.
    let baseline = (bounds.height - font.capHeight) / 2
    s.draw(at: NSPoint(x: (bounds.width - s.size().width) / 2, y: baseline + font.descender))
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
