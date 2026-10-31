import AppKit
import ServiceManagement

/// Settings window (gear): app-wide switches on top, tab manager below. Every
/// change applies immediately and is written to tabs.toml. Mirrors
/// src/linux/settings.rs.
final class SettingsWindow: NSObject, NSWindowDelegate {
  let window: NSWindow
  private weak var app: App?
  let list = NSStackView()
  private var switches: [(row: SwitchRow, get: (Config) -> Bool)] = []
  private(set) var closeRow: SwitchRow!
  private(set) var dockRow: SwitchRow!
  private var notificationsRow: SwitchRow!
  private static let dockSubtitle = "Number of unread messages on the WebTabs icon"
  /// Re-checks macOS permissions when the user comes back from System Settings.
  private var activation: NSObjectProtocol?

  init(app: App) {
    self.app = app
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 560, height: 720),
      styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered,
      defer: true)
    super.init()
    window.title = "WebTabs Settings"
    window.minSize = NSSize(width: 480, height: 540)
    window.isReleasedWhenClosed = false
    window.tabbingMode = .disallowed
    window.delegate = self
    if !window.setFrameUsingName("WebTabsSettings") { window.center() }
    window.setFrameAutosaveName("WebTabsSettings")

    let general = Group()
    let login = SwitchRow(
      "Open at login", "Opens hidden in the menu bar when you log in",
      on: SMAppService.mainApp.status == .enabled
    ) { [weak self] on in
      do {
        if on {
          try SMAppService.mainApp.register()
        } else {
          try SMAppService.mainApp.unregister()
        }
      } catch {
        self?.app?.showError("Could not change Open at login", error, in: self?.window)
        return false
      }
      if on && SMAppService.mainApp.status == .requiresApproval {
        SMAppService.openSystemSettingsLoginItems()
      }
      return true
    }
    general.add(login)
    func option(
      _ title: String, _ subtitle: String, _ key: WritableKeyPath<Config, Bool>
    ) -> SwitchRow {
      let row = SwitchRow(title, subtitle, on: app.config[keyPath: key]) { [weak app] on in
        app?.updateOptions { $0[keyPath: key] = on }
        return true
      }
      switches.append((row, { $0[keyPath: key] }))
      general.add(row)
      return row
    }
    notificationsRow = option("Show notifications", "Notifications from your tabs", \.notifications)
    _ = option("Show icon in menu bar", "Unread indicator and quick menu in the menu bar", \.tray)
    closeRow = option("Close to menu bar", "Closing the window keeps WebTabs running", \.closeToTray)
    _ = option("Unread count in window title", "e.g. “WebTabs (5)”", \.unreadInTitle)
    dockRow = option("Unread count on Dock icon", Self.dockSubtitle, \.unreadInDock)

    list.orientation = .vertical
    list.alignment = .centerX
    list.spacing = 0
    list.translatesAutoresizingMaskIntoConstraints = false
    let doc = FlippedView()
    doc.translatesAutoresizingMaskIntoConstraints = false
    doc.addSubview(list)
    let scroll = NSScrollView()
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    scroll.documentView = doc
    let tabsGroup = Group()
    tabsGroup.addFilling(scroll)

    let add = NSButton(title: "Add tab", target: self, action: #selector(addTab))
    add.image = NSImage(systemSymbolName: "plus", accessibilityDescription: nil)
    add.imagePosition = .imageLeading
    let toml = NSButton(title: "Open tabs.toml", target: self, action: #selector(openToml))
    let actions = NSStackView(views: [add, NSView(), toml])

    let generalHeading = heading("General")
    let tabsHeading = heading("Tabs")
    let page = NSStackView(views: [generalHeading, general, tabsHeading, tabsGroup, actions])
    page.orientation = .vertical
    page.alignment = .leading
    page.spacing = 6
    page.edgeInsets = NSEdgeInsets(top: 18, left: 20, bottom: 18, right: 20)
    page.setCustomSpacing(20, after: general)
    page.setCustomSpacing(12, after: tabsGroup)
    page.translatesAutoresizingMaskIntoConstraints = false
    window.contentView = NSView()
    window.contentView!.addSubview(page)
    NSLayoutConstraint.activate([
      page.leadingAnchor.constraint(equalTo: window.contentView!.leadingAnchor),
      page.trailingAnchor.constraint(equalTo: window.contentView!.trailingAnchor),
      page.topAnchor.constraint(equalTo: window.contentView!.topAnchor),
      page.bottomAnchor.constraint(equalTo: window.contentView!.bottomAnchor),
      doc.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
      doc.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
      doc.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
      list.leadingAnchor.constraint(equalTo: doc.leadingAnchor),
      list.trailingAnchor.constraint(equalTo: doc.trailingAnchor),
      list.topAnchor.constraint(equalTo: doc.topAnchor),
      list.bottomAnchor.constraint(equalTo: doc.bottomAnchor),
      tabsGroup.heightAnchor.constraint(greaterThanOrEqualToConstant: 260),
    ])
    for view in [general, tabsGroup, actions] {
      view.widthAnchor.constraint(equalTo: page.widthAnchor, constant: -40).isActive = true
    }
    tabsGroup.setContentHuggingPriority(.defaultLow, for: .vertical)
    optionsChanged()
    refresh()
    activation = NotificationCenter.default.addObserver(
      forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
    ) { [weak self] _ in self?.optionsChanged() }
  }

  func show() {
    window.makeKeyAndOrderFront(nil)
    NSApp.activate()
  }

  func windowWillClose(_ notification: Notification) {
    if let activation { NotificationCenter.default.removeObserver(activation) }
    if app?.settingsWindow === self { app?.settingsWindow = nil }
  }

  /// App options changed (here or in tabs.toml): sync the switches.
  func optionsChanged() {
    guard let app else { return }
    for (row, get) in switches { row.toggle.state = get(app.config) ? .on : .off }
    // Closing to a menu bar icon that isn't there would make the app unreachable.
    closeRow.isEnabled = app.config.tray
    // macOS can block both, whatever the switches say: point at the fix.
    app.notificationPermissions { [weak self] alerts, badges in
      guard let self, let app = self.app else { return }
      let open: () -> Void = { [weak app] in app?.openNotificationSettings() }
      let link = "Open Notification Settings…"
      if app.config.notifications && !alerts {
        self.notificationsRow.setWarning(
          "Notifications are off for WebTabs in System Settings", link: link, open)
      } else {
        self.notificationsRow.setWarning(nil)
      }
      if app.config.unreadInDock && !badges {
        self.dockRow.setWarning("Badges are off for WebTabs in System Settings", link: link, open)
      } else {
        self.dockRow.setWarning(nil)
      }
    }
  }

  /// The list is rebuilt from App state after every change; App calls this too
  /// when tabs.toml is edited by hand while the window is open.
  func refresh() {
    guard let app else { return }
    list.arrangedSubviews.forEach { $0.removeFromSuperview() }
    if app.tabs.isEmpty {
      let empty = NSTextField(labelWithString: "No tabs yet")
      empty.textColor = .secondaryLabelColor
      let box = NSStackView(views: [empty])
      box.edgeInsets = NSEdgeInsets(top: 18, left: 0, bottom: 18, right: 0)
      list.addArrangedSubview(box)
      box.widthAnchor.constraint(equalTo: list.widthAnchor).isActive = true
    }
    for (i, tv) in app.tabs.enumerated() {
      if i > 0 { Group.addSeparator(to: list) }
      let row = tabRow(app, tv, index: i, count: app.tabs.count)
      list.addArrangedSubview(row)
      row.widthAnchor.constraint(equalTo: list.widthAnchor).isActive = true
    }
    optionsChanged()
  }

  private func tabRow(_ app: App, _ tv: TabView, index: Int, count: Int) -> NSView {
    let icon = NSImageView()
    icon.imageScaling = .scaleProportionallyUpOrDown
    icon.image = tv.icon ?? Icons.placeholder
    icon.contentTintColor = tv.icon == nil ? .secondaryLabelColor : nil
    icon.widthAnchor.constraint(equalToConstant: 24).isActive = true
    icon.heightAnchor.constraint(equalToConstant: 24).isActive = true

    let name = NSTextField(labelWithString: tv.tab.name)
    name.lineBreakMode = .byTruncatingTail
    let url = NSTextField(labelWithString: tv.tab.url)
    url.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    url.textColor = .secondaryLabelColor
    url.lineBreakMode = .byTruncatingTail
    let text = NSStackView(views: [name, url])
    text.orientation = .vertical
    text.alignment = .leading
    text.spacing = 2
    // The text takes the spare width, so every row's buttons line up on the right.
    text.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
    text.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    url.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

    let size = NSSize(width: 30, height: 28)
    func tile(_ symbol: String, _ label: String, enabled: Bool, _ action: @escaping () -> Void)
      -> SymbolTile
    {
      let t = SymbolTile(symbol, label: label, size: size, pointSize: 13)
      t.radius = 7
      t.isEnabled = enabled
      // Rebuilding the list destroys this tile; do it after the click returns.
      t.onClick = { DispatchQueue.main.async(execute: action) }
      return t
    }
    let bell = SymbolTile("bell", label: "", size: size, pointSize: 13)
    bell.radius = 7
    // Static and capture-free: the tile keeps its click handler alive, so a
    // handler holding the tile or the tab would leak both (and the tab's profile).
    func show(_ bell: SymbolTile, on: Bool) {
      bell.setSymbol(
        on ? "bell" : "bell.slash", label: on ? "Notifications on" : "Muted",
        tooltip: on ? "Notifications on (click to mute)" : "Muted (click to unmute)")
    }
    show(bell, on: tv.tab.notifications)
    bell.onClick = { [weak app, weak tv, weak bell] in
      guard let app, let tv, let bell else { return }
      app.setTabNotifications(tv, !tv.tab.notifications)
      show(bell, on: tv.tab.notifications)
    }
    weak let parent = window
    let row = NSStackView(views: [
      icon, text, bell,
      tile("chevron.up", "Move up (left in the tab bar)", enabled: index > 0) { [weak app, weak tv] in
        if let app, let tv { app.moveTab(tv, -1) }
      },
      tile("chevron.down", "Move down (right in the tab bar)", enabled: index + 1 < count) {
        [weak app, weak tv] in
        if let app, let tv { app.moveTab(tv, 1) }
      },
      tile("pencil", "Edit", enabled: true) { [weak app, weak tv] in
        if let app, let tv { app.editTabDialog(tv, parent: parent) }
      },
      tile("trash", "Remove", enabled: true) { [weak app, weak tv] in
        if let app, let tv { app.confirmRemove(tv, parent: parent) }
      },
    ])
    row.distribution = .fill
    row.spacing = 10
    row.setCustomSpacing(4, after: bell)
    for view in row.arrangedSubviews.suffix(4) { row.setCustomSpacing(4, after: view) }
    row.edgeInsets = NSEdgeInsets(top: 8, left: 10, bottom: 8, right: 8)
    return row
  }

  @objc private func addTab() { app?.addTabDialog(parent: window) }
  @objc private func openToml() { app?.openConfigFile() }
}

/// Bold section title.
private func heading(_ text: String) -> NSTextField {
  let l = NSTextField(labelWithString: text)
  l.font = .boldSystemFont(ofSize: 13)
  return l
}

/// A rounded, bordered box of rows (GTK's framed list box).
final class Group: NSView {
  private let stack = NSStackView()

  init() {
    super.init(frame: .zero)
    translatesAutoresizingMaskIntoConstraints = false
    stack.orientation = .vertical
    stack.spacing = 0
    addFilling(stack)
  }
  required init?(coder: NSCoder) { fatalError() }

  func addFilling(_ view: NSView) {
    view.translatesAutoresizingMaskIntoConstraints = false
    addSubview(view)
    NSLayoutConstraint.activate([
      view.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 1),
      view.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -1),
      view.topAnchor.constraint(equalTo: topAnchor, constant: 1),
      view.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -1),
    ])
  }

  func add(_ row: NSView) {
    if !stack.arrangedSubviews.isEmpty { Group.addSeparator(to: stack) }
    stack.addArrangedSubview(row)
    row.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
  }

  /// An inset hairline between rows.
  static func addSeparator(to stack: NSStackView) {
    let line = NSBox()
    line.boxType = .separator
    stack.addArrangedSubview(line)
    line.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -20).isActive = true
  }

  override func draw(_ dirtyRect: NSRect) {
    let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 8, yRadius: 8)
    NSColor.labelColor.withAlphaComponent(0.03).setFill()
    path.fill()
    NSColor.separatorColor.setStroke()
    path.lineWidth = 1
    path.stroke()
  }
}

/// A title/subtitle row with a switch. `apply` returns whether the change stuck;
/// if not, the switch flips back.
final class SwitchRow: NSView {
  let toggle = NSSwitch()
  private let title: NSTextField
  private let subtitle: NSTextField
  private let subtitleText: String
  private let link = NSButton(title: "", target: nil, action: nil)
  private var linkAction: (() -> Void)?
  private let apply: (Bool) -> Bool
  var isEnabled = true {
    didSet {
      toggle.isEnabled = isEnabled
      title.textColor = isEnabled ? .labelColor : .disabledControlTextColor
      subtitle.textColor = isEnabled ? .secondaryLabelColor : .disabledControlTextColor
    }
  }

  /// A problem to show instead of the subtitle, with a link that fixes it.
  func setWarning(_ text: String?, link label: String = "", _ action: (() -> Void)? = nil) {
    subtitle.stringValue = text ?? subtitleText
    subtitle.textColor = text == nil ? .secondaryLabelColor : .systemOrange
    link.title = label
    link.isHidden = text == nil || action == nil
    linkAction = action
  }

  init(_ title: String, _ subtitle: String, on: Bool, apply: @escaping (Bool) -> Bool) {
    self.title = NSTextField(labelWithString: title)
    self.subtitle = NSTextField(labelWithString: subtitle)
    subtitleText = subtitle
    self.apply = apply
    super.init(frame: .zero)
    self.subtitle.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    self.subtitle.textColor = .secondaryLabelColor
    toggle.state = on ? .on : .off
    toggle.target = self
    toggle.action = #selector(changed)
    toggle.setAccessibilityLabel(title)
    link.isBordered = false
    link.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    link.contentTintColor = .linkColor
    link.target = self
    link.action = #selector(followLink)
    link.isHidden = true
    let text = NSStackView(views: [self.title, self.subtitle, link])
    text.orientation = .vertical
    text.alignment = .leading
    text.spacing = 2
    let row = NSStackView(views: [text, NSView(), toggle])
    row.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
    row.translatesAutoresizingMaskIntoConstraints = false
    addSubview(row)
    NSLayoutConstraint.activate([
      row.leadingAnchor.constraint(equalTo: leadingAnchor),
      row.trailingAnchor.constraint(equalTo: trailingAnchor),
      row.topAnchor.constraint(equalTo: topAnchor),
      row.bottomAnchor.constraint(equalTo: bottomAnchor),
    ])
  }
  required init?(coder: NSCoder) { fatalError() }

  @objc private func followLink() { linkAction?() }

  @objc private func changed() {
    let on = toggle.state == .on
    if !apply(on) { toggle.state = on ? .off : .on }
  }
}
