import AppKit
import ServiceManagement

/// Settings window: a sidebar of panes (General · Tabs · Appearance · About), as
/// on Linux and Windows. Every change applies immediately and is written to
/// tabs.toml. Mirrors src/linux/settings.rs.
final class SettingsWindow: NSObject, NSWindowDelegate, NSTableViewDataSource,
  NSTableViewDelegate
{
  enum Pane: Int, CaseIterable {
    case general, tabs, appearance, about

    var title: String {
      switch self {
      case .general: "General"
      case .tabs: "Tabs"
      case .appearance: "Appearance"
      case .about: "About"
      }
    }
    var symbol: String {
      switch self {
      case .general: "gearshape"
      case .tabs: "square.grid.2x2"
      case .appearance: "paintpalette"
      case .about: "info.circle"
      }
    }
  }

  let window: NSWindow
  private weak var app: App?
  private(set) var pane = Pane.general
  private var nav: [Pane: NavItem] = [:]
  private var panes: [Pane: NSView] = [:]
  private let detail = NSView()

  private var switches: [(row: SwitchRow, get: (Config) -> Bool)] = []
  private(set) var closeRow: SwitchRow!
  private(set) var dockRow: SwitchRow!
  private var notificationsRow: SwitchRow!
  private static let dockSubtitle = "Number of unread messages on the WebTabs icon"

  /// One row per tab, in tab bar order; drag a row to reorder.
  let table = NSTableView()
  private let noTabs = NSTextField(labelWithString: "No tabs yet")

  let preview = AppearancePreview()
  let slider = NSSlider()
  private let sizeLabel = NSTextField(labelWithString: "")
  private(set) var swatches: [Swatch] = []
  private let tintLabel = NSTextField(labelWithString: "")
  /// The shared colour panel is sending its changes here.
  private var pickingColor = false

  /// Re-checks macOS permissions when the user comes back from System Settings.
  private var activation: NSObjectProtocol?

  init(app: App) {
    self.app = app
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 720, height: 580),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered, defer: true)
    super.init()
    window.title = "WebTabs Settings"
    // The sidebar runs up under the traffic lights, as in System Settings.
    window.titlebarAppearsTransparent = true
    window.titleVisibility = .hidden
    window.contentMinSize = NSSize(width: 640, height: 560)
    window.isReleasedWhenClosed = false
    window.tabbingMode = .disallowed
    window.delegate = self
    if !window.setFrameUsingName("WebTabsSettingsWindow") { window.center() }
    window.setFrameAutosaveName("WebTabsSettingsWindow")
    // A saved frame can predate the minimum.
    let size = window.contentRect(forFrameRect: window.frame).size
    if size.width < window.contentMinSize.width || size.height < window.contentMinSize.height {
      window.setContentSize(
        NSSize(
          width: max(size.width, window.contentMinSize.width),
          height: max(size.height, window.contentMinSize.height)))
    }

    let sidebar = NSVisualEffectView()
    sidebar.material = .sidebar
    sidebar.blendingMode = .behindWindow
    sidebar.state = .followsWindowActiveState
    let navStack = NSStackView()
    navStack.orientation = .vertical
    navStack.spacing = 2
    for p in Pane.allCases {
      let item = NavItem(p.title, symbol: p.symbol)
      item.onClick = { [weak self] in self?.select(p) }
      nav[p] = item
      navStack.addArrangedSubview(item)
    }
    let divider = NSBox()
    divider.boxType = .separator
    // A separator box hugs its 1 pt length, which would shrink the window to it.
    divider.setContentHuggingPriority(.defaultLow, for: .vertical)
    let root = NSView()
    window.contentView = root
    for view in [sidebar, divider, detail, navStack] as [NSView] {
      view.translatesAutoresizingMaskIntoConstraints = false
    }
    sidebar.addSubview(navStack)
    for view in [sidebar, divider, detail] as [NSView] { root.addSubview(view) }
    NSLayoutConstraint.activate([
      sidebar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
      sidebar.topAnchor.constraint(equalTo: root.topAnchor),
      sidebar.bottomAnchor.constraint(equalTo: root.bottomAnchor),
      sidebar.widthAnchor.constraint(equalToConstant: NavItem.width + 20),
      navStack.topAnchor.constraint(equalTo: sidebar.topAnchor, constant: 52),
      navStack.centerXAnchor.constraint(equalTo: sidebar.centerXAnchor),
      divider.leadingAnchor.constraint(equalTo: sidebar.trailingAnchor),
      divider.widthAnchor.constraint(equalToConstant: 1),
      divider.topAnchor.constraint(equalTo: root.topAnchor),
      divider.bottomAnchor.constraint(equalTo: root.bottomAnchor),
      detail.leadingAnchor.constraint(equalTo: divider.trailingAnchor),
      detail.trailingAnchor.constraint(equalTo: root.trailingAnchor),
      detail.topAnchor.constraint(equalTo: root.topAnchor),
      detail.bottomAnchor.constraint(equalTo: root.bottomAnchor),
    ])

    panes[.general] = scrolled(generalPane(app))
    panes[.tabs] = tabsPane()
    panes[.appearance] = scrolled(appearancePane())
    panes[.about] = scrolled(aboutPane())
    select(.general)
    refresh()
    activation = NotificationCenter.default.addObserver(
      forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
    ) { [weak self] _ in self?.optionsChanged() }
  }

  func show() {
    window.makeKeyAndOrderFront(nil)
    NSApp.activate()
  }

  func select(_ p: Pane) {
    pane = p
    for (key, item) in nav { item.isActive = key == p }
    detail.subviews.forEach { $0.removeFromSuperview() }
    guard let view = panes[p] else { return }
    view.translatesAutoresizingMaskIntoConstraints = false
    detail.addSubview(view)
    NSLayoutConstraint.activate([
      view.leadingAnchor.constraint(equalTo: detail.leadingAnchor),
      view.trailingAnchor.constraint(equalTo: detail.trailingAnchor),
      view.topAnchor.constraint(equalTo: detail.topAnchor),
      view.bottomAnchor.constraint(equalTo: detail.bottomAnchor),
    ])
  }

  func windowWillClose(_ notification: Notification) {
    if let activation { NotificationCenter.default.removeObserver(activation) }
    if pickingColor {
      NSColorPanel.shared.setTarget(nil)
      NSColorPanel.shared.orderOut(nil)
    }
    if app?.settingsWindow === self { app?.settingsWindow = nil }
  }

  /// App options changed (here or in tabs.toml): sync the controls.
  func optionsChanged() {
    guard let app else { return }
    for (row, get) in switches { row.toggle.state = get(app.config) ? .on : .off }
    // Closing to a menu bar icon that isn't there would make the app unreachable.
    closeRow.isEnabled = app.config.tray
    syncAppearance(app.config)
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

  /// Tabs changed: App calls this too when tabs.toml is edited by hand.
  func refresh() {
    guard let app else { return }
    table.reloadData()
    noTabs.isHidden = !app.tabs.isEmpty
    preview.show(app.tabs.prefix(4).map { $0.icon })
    optionsChanged()
  }

  // ---- General ----------------------------------------------------------------------

  private func generalPane(_ app: App) -> NSView {
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
    func option(
      _ title: String, _ subtitle: String, _ key: WritableKeyPath<Config, Bool>
    ) -> SwitchRow {
      let row = SwitchRow(title, subtitle, on: app.config[keyPath: key]) { [weak app] on in
        app?.updateOptions { $0[keyPath: key] = on }
        return true
      }
      switches.append((row, { $0[keyPath: key] }))
      return row
    }
    notificationsRow = option("Show notifications", "Notifications from your tabs", \.notifications)
    let title = option("Unread count in window title", "e.g. “WebTabs (5)”", \.unreadInTitle)
    dockRow = option("Unread count on Dock icon", Self.dockSubtitle, \.unreadInDock)
    let tray = option(
      "Show icon in menu bar", "Unread indicator and quick menu in the menu bar", \.tray)
    closeRow = option("Close to menu bar", "Closing the window keeps WebTabs running", \.closeToTray)
    closeRow.indent = 18
    return page(
      "General",
      [
        section("Startup", [login]),
        section("Notifications", [notificationsRow, title, dockRow]),
        section("Menu bar", [tray, closeRow]),
      ])
  }

  // ---- Tabs -------------------------------------------------------------------------

  private func tabsPane() -> NSView {
    table.headerView = nil
    table.addTableColumn(NSTableColumn(identifier: NSUserInterfaceItemIdentifier("tab")))
    table.style = .plain
    table.rowHeight = 52
    table.intercellSpacing = .zero
    table.selectionHighlightStyle = .none
    table.backgroundColor = .clear
    table.dataSource = self
    table.delegate = self
    table.registerForDraggedTypes([.webtabsTabRow])
    table.setDraggingSourceOperationMask(.move, forLocal: true)
    table.draggingDestinationFeedbackStyle = .gap
    let scroll = NSScrollView()
    scroll.documentView = table
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    scroll.setContentHuggingPriority(.defaultLow, for: .vertical)
    scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 200).isActive = true
    noTabs.textColor = .secondaryLabelColor
    noTabs.translatesAutoresizingMaskIntoConstraints = false
    scroll.addSubview(noTabs)
    NSLayoutConstraint.activate([
      noTabs.centerXAnchor.constraint(equalTo: scroll.centerXAnchor),
      noTabs.centerYAnchor.constraint(equalTo: scroll.centerYAnchor),
    ])

    let add = NSButton(title: "Add Tab…", target: self, action: #selector(addTab))
    let toml = NSButton(title: "Open tabs.toml", target: self, action: #selector(openToml))
    toml.isBordered = false
    toml.contentTintColor = .linkColor
    let actions = NSStackView(views: [add, NSView(), toml])
    let hint = note("Drag a tab to change its place in the tab bar.")
    let view = page("Tabs", [hint, scroll, actions], fill: true)
    (view as? NSStackView)?.setCustomSpacing(14, after: hint)
    (view as? NSStackView)?.setCustomSpacing(14, after: scroll)
    return view
  }

  func numberOfRows(in tableView: NSTableView) -> Int { app?.tabs.count ?? 0 }

  func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int)
    -> NSView?
  {
    guard let app, app.tabs.indices.contains(row) else { return nil }
    return tabRow(app, app.tabs[row], index: row, count: app.tabs.count)
  }

  func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
    HairlineRow()
  }

  func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int)
    -> (any NSPasteboardWriting)?
  {
    let item = NSPasteboardItem()
    item.setString(String(row), forType: .webtabsTabRow)
    return item
  }

  func tableView(
    _ tableView: NSTableView, validateDrop info: any NSDraggingInfo, proposedRow row: Int,
    proposedDropOperation dropOperation: NSTableView.DropOperation
  ) -> NSDragOperation {
    guard info.draggingSource as? NSTableView === table else { return [] }
    if dropOperation == .on { tableView.setDropRow(row, dropOperation: .above) }
    return .move
  }

  func tableView(
    _ tableView: NSTableView, acceptDrop info: any NSDraggingInfo, row: Int,
    dropOperation: NSTableView.DropOperation
  ) -> Bool {
    guard let app, let text = info.draggingPasteboard.string(forType: .webtabsTabRow),
      let from = Int(text), app.tabs.indices.contains(from)
    else { return false }
    let tv = app.tabs[from]
    // Moving reloads the table: do it once the drop has finished.
    DispatchQueue.main.async { [weak app] in app?.moveTab(tv, to: from < row ? row - 1 : row) }
    return true
  }

  private func tabRow(_ app: App, _ tv: TabView, index: Int, count: Int) -> NSView {
    let grip = NSImageView(
      image: NSImage(systemSymbolName: "line.3.horizontal", accessibilityDescription: nil)!)
    grip.contentTintColor = .tertiaryLabelColor
    grip.toolTip = "Drag to reorder"
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
    let more = SymbolTile("ellipsis", label: "More", tooltip: "Edit, move or remove", size: size,
      pointSize: 13)
    more.radius = 7
    weak let parent = window
    more.onClick = { [weak app, weak tv, weak more] in
      guard let app, let tv, let more else { return }
      // Each item may rebuild the list (and this tile): run it after the menu closes.
      func item(_ title: String, enabled: Bool = true, _ action: @escaping (App, TabView) -> Void)
        -> NSMenuItem
      {
        ActionItem(title, enabled: enabled) { [weak app, weak tv] in
          DispatchQueue.main.async { if let app, let tv { action(app, tv) } }
        }
      }
      let menu = NSMenu()
      menu.autoenablesItems = false
      menu.items = [
        item("Edit…") { app, tv in app.editTabDialog(tv, parent: parent) },
        item("Move Up", enabled: index > 0) { app, tv in app.moveTab(tv, -1) },
        item("Move Down", enabled: index + 1 < count) { app, tv in app.moveTab(tv, 1) },
        .separator(),
        item("Remove…") { app, tv in app.confirmRemove(tv, parent: parent) },
      ]
      menu.popUp(positioning: nil, at: NSPoint(x: 0, y: more.bounds.maxY + 4), in: more)
    }
    let row = NSStackView(views: [grip, icon, text, bell, more])
    row.spacing = 10
    row.setCustomSpacing(4, after: bell)
    row.edgeInsets = NSEdgeInsets(top: 0, left: 4, bottom: 0, right: 0)
    return row
  }

  @objc private func addTab() { app?.addTabDialog(parent: window) }
  @objc private func openToml() { app?.openConfigFile() }

  // ---- Appearance -------------------------------------------------------------------

  private func appearancePane() -> NSView {
    let limits = Appearance.shared.iconSize
    slider.minValue = Double(limits.min)
    slider.maxValue = Double(limits.max)
    slider.numberOfTickMarks = (limits.max - limits.min) / 2 + 1
    slider.allowsTickMarkValuesOnly = true
    slider.isContinuous = true
    slider.target = self
    slider.action = #selector(sizeChanged)
    slider.setAccessibilityLabel("Icon size")
    sizeLabel.font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .medium)
    sizeLabel.alignment = .right
    sizeLabel.widthAnchor.constraint(equalToConstant: 44).isActive = true
    func glyph(_ points: CGFloat) -> NSImageView {
      let v = NSImageView(
        image: NSImage(systemSymbolName: "app.fill", accessibilityDescription: nil)!)
      v.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: points, weight: .regular)
      v.contentTintColor = .tertiaryLabelColor
      return v
    }
    let sizeRow = NSStackView(views: [glyph(10), slider, glyph(17), sizeLabel])
    sizeRow.spacing = 10
    slider.setContentHuggingPriority(.defaultLow, for: .horizontal)

    let names = ["Default"] + Appearance.shared.tints + ["Custom"]
    for name in names {
      let tint: String? = name == "Default" ? nil : name == "Custom" ? "custom" : name
      let swatch = Swatch(tint: tint, label: name.capitalized)
      swatch.onClick = { [weak self] in self?.pick(tint) }
      swatches.append(swatch)
    }
    let swatchRow = NSStackView(views: swatches)
    swatchRow.spacing = 10
    tintLabel.textColor = .secondaryLabelColor
    tintLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)

    return page(
      "Appearance",
      [
        preview,
        section(
          "Icon size",
          [sizeRow, note("Above 24 pt the title bar grows to fit the icons.")]),
        section(
          "Title bar color",
          [swatchRow, tintLabel, note("Tints the tab bar. Pages keep their own colors.")]),
      ])
  }

  private func syncAppearance(_ config: Config) {
    slider.integerValue = config.iconSize
    sizeLabel.stringValue = "\(config.iconSize) pt"
    let custom = config.tint?.hasPrefix("#") == true
    for s in swatches {
      s.isActive = s.tint == config.tint || (custom && s.tint == "custom")
      if s.tint == "custom" { s.custom = custom ? Tint.color(config.tint) : nil }
    }
    tintLabel.stringValue =
      config.tint.map { custom ? "Custom \($0)" : $0.capitalized } ?? "Default (system gray)"
    preview.update(BarMetrics(iconSize: config.iconSize), tint: config.tint)
  }

  @objc private func sizeChanged() {
    let value = slider.integerValue
    guard value != app?.config.iconSize else { return }
    app?.updateOptions(saveNow: false) { $0.iconSize = value }
  }

  private func pick(_ tint: String?) {
    guard tint == "custom" else {
      app?.updateOptions { $0.tint = tint }
      return
    }
    let panel = NSColorPanel.shared
    panel.showsAlpha = false
    panel.setTarget(self)
    pickingColor = true
    panel.setAction(#selector(colorPicked))
    panel.color = Tint.color(app?.config.tint).flatMap { $0.usingColorSpace(.sRGB) }
      ?? Tint.rgb(0xF4E3D0)
    panel.orderFront(nil)
    colorPicked(panel)
  }

  @objc private func colorPicked(_ panel: NSColorPanel) {
    guard let hex = Tint.hex(panel.color), hex != app?.config.tint else { return }
    app?.updateOptions(saveNow: false) { $0.tint = hex }
  }

  // ---- About ------------------------------------------------------------------------

  private func aboutPane() -> NSView {
    let icon = NSImageView(image: NSApp.applicationIconImage)
    icon.widthAnchor.constraint(equalToConstant: 72).isActive = true
    icon.heightAnchor.constraint(equalToConstant: 72).isActive = true
    let name = NSTextField(labelWithString: "WebTabs")
    name.font = .systemFont(ofSize: 22, weight: .semibold)
    let info = Bundle.main.infoDictionary ?? [:]
    let version = NSTextField(
      labelWithString: "Version \(info["CFBundleShortVersionString"] as? String ?? "?")")
    version.textColor = .secondaryLabelColor
    let blurb = NSTextField(labelWithString: "Your web apps in one window, signed in and quiet.")
    let config = NSButton(title: "Open tabs.toml", target: self, action: #selector(openToml))
    let site = NSButton(title: "Website", target: self, action: #selector(openSite))
    let buttons = NSStackView(views: [config, site])
    let stack = NSStackView(views: [icon, name, version, blurb, buttons])
    stack.orientation = .vertical
    stack.spacing = 8
    stack.setCustomSpacing(14, after: icon)
    stack.setCustomSpacing(14, after: blurb)
    stack.edgeInsets = NSEdgeInsets(top: 96, left: 32, bottom: 32, right: 32)
    stack.setHuggingPriority(.defaultLow, for: .vertical)
    return stack
  }

  @objc private func openSite() {
    NSWorkspace.shared.open(URL(string: "https://github.com/ivanm/webtabs")!)
  }

  // ---- layout -----------------------------------------------------------------------

  /// A pane that scrolls when the window is short, so its rows keep their size.
  private func scrolled(_ content: NSView) -> NSView {
    let doc = FlippedView()
    for view in [doc, content] { view.translatesAutoresizingMaskIntoConstraints = false }
    doc.addSubview(content)
    let scroll = NSScrollView()
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.autohidesScrollers = true
    // The pane leaves room for the title bar itself.
    scroll.automaticallyAdjustsContentInsets = false
    scroll.documentView = doc
    NSLayoutConstraint.activate([
      doc.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
      doc.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
      doc.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
      content.leadingAnchor.constraint(equalTo: doc.leadingAnchor),
      content.trailingAnchor.constraint(equalTo: doc.trailingAnchor),
      content.topAnchor.constraint(equalTo: doc.topAnchor),
      content.bottomAnchor.constraint(equalTo: doc.bottomAnchor),
    ])
    return scroll
  }

  /// A pane: big title, then its sections, 32 pt from the edges.
  private func page(_ title: String, _ views: [NSView], fill: Bool = false) -> NSView {
    let heading = NSTextField(labelWithString: title)
    heading.font = .systemFont(ofSize: 20, weight: .semibold)
    let stack = NSStackView(views: [heading])
    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 20
    stack.edgeInsets = NSEdgeInsets(top: 44, left: 32, bottom: fill ? 20 : 32, right: 32)
    stack.setCustomSpacing(18, after: heading)
    // The window sets the size, not the content.
    stack.setHuggingPriority(.defaultLow, for: .vertical)
    for (i, view) in views.enumerated() {
      if i > 0 && view is Section && views[i - 1] is Section {
        let line = NSBox()
        line.boxType = .separator
        stack.addArrangedSubview(line)
        line.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -64).isActive = true
      }
      stack.addArrangedSubview(view)
      view.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -64).isActive = true
    }
    return stack
  }

  /// A small uppercase heading over flat rows (no boxes), as on Linux/Windows.
  private func section(_ title: String, _ rows: [NSView]) -> Section {
    let heading = NSTextField(labelWithString: title.uppercased())
    heading.font = .systemFont(ofSize: 11, weight: .semibold)
    heading.textColor = .secondaryLabelColor
    let s = Section(views: [heading] + rows)
    s.orientation = .vertical
    s.alignment = .leading
    s.spacing = 14
    s.setCustomSpacing(10, after: heading)
    // Stacks hug at 750, the same as labels resist squashing: rows would lose.
    s.setHuggingPriority(.defaultLow, for: .vertical)
    for row in rows { row.widthAnchor.constraint(equalTo: s.widthAnchor).isActive = true }
    return s
  }
}

private final class Section: NSStackView {}

private func note(_ text: String) -> NSTextField {
  let l = NSTextField(wrappingLabelWithString: text)
  l.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
  l.textColor = .secondaryLabelColor
  return l
}

extension NSPasteboard.PasteboardType {
  static let webtabsTabRow = NSPasteboard.PasteboardType("app.webtabs.tab-row")
}

/// A table row with an inset hairline under it (none under the last).
private final class HairlineRow: NSTableRowView {
  override func drawBackground(in dirtyRect: NSRect) {
    guard let table = superview as? NSTableView, table.row(for: self) < table.numberOfRows - 1
    else { return }
    NSColor.separatorColor.setFill()
    NSRect(x: 40, y: bounds.maxY - 1, width: bounds.width - 40, height: 1).fill()
  }
}

/// A sidebar entry: symbol and title; the current pane is highlighted.
final class NavItem: Tile {
  static let width: CGFloat = 168
  private let symbol = NSImageView()
  private let title: NSTextField

  init(_ text: String, symbol name: String) {
    title = NSTextField(labelWithString: text)
    super.init(size: NSSize(width: Self.width, height: 30))
    radius = 7
    symbol.image = NSImage(systemSymbolName: name, accessibilityDescription: nil)
    symbol.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular)
    let row = NSStackView(views: [symbol, title])
    row.spacing = 8
    row.translatesAutoresizingMaskIntoConstraints = false
    addSubview(row)
    NSLayoutConstraint.activate([
      symbol.widthAnchor.constraint(equalToConstant: 18),
      row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
      row.centerYAnchor.constraint(equalTo: centerYAnchor),
    ])
    setAccessibilityLabel(text)
    update()
  }
  required init?(coder: NSCoder) { fatalError() }

  override var isActive: Bool {
    didSet { update() }
  }
  private func update() {
    symbol.contentTintColor = isActive ? .webtabsAccent : .secondaryLabelColor
    title.font = .systemFont(ofSize: 13, weight: isActive ? .semibold : .regular)
  }
  override func accessibilityRole() -> NSAccessibility.Role? { .radioButton }
  override func accessibilityValue() -> Any? { isActive ? 1 : 0 }
}

/// A round colour choice. `tint` is the config value, nil for the system gray
/// and "custom" for the colour panel (showing the picked colour once there is one).
final class Swatch: Tile {
  let tint: String?
  var custom: NSColor? {
    didSet { needsDisplay = true }
  }

  init(tint: String?, label: String) {
    self.tint = tint
    super.init(size: NSSize(width: 30, height: 30))
    radius = 15
    toolTip = label
    setAccessibilityLabel(label)
  }
  required init?(coder: NSCoder) { fatalError() }

  override func draw(_ dirtyRect: NSRect) {
    let disc = bounds.insetBy(dx: 4, dy: 4)
    let path = NSBezierPath(ovalIn: disc)
    if tint == "custom", let custom {
      custom.setFill()
      path.fill()
    } else if tint == "custom" {
      // A colour wheel: pick any colour.
      let hues = 12
      for i in 0..<hues {
        let slice = NSBezierPath()
        let c = NSPoint(x: disc.midX, y: disc.midY)
        slice.move(to: c)
        slice.appendArc(
          withCenter: c, radius: disc.width / 2, startAngle: CGFloat(i) * 360 / CGFloat(hues),
          endAngle: CGFloat(i + 1) * 360 / CGFloat(hues))
        slice.close()
        NSColor(hue: CGFloat(i) / CGFloat(hues), saturation: 0.45, brightness: 0.95, alpha: 1)
          .setFill()
        slice.fill()
      }
    } else {
      (Tint.color(tint) ?? .windowBackgroundColor).setFill()
      path.fill()
    }
    NSColor.labelColor.withAlphaComponent(0.15).setStroke()
    path.lineWidth = 1
    path.stroke()
    if isActive {
      let ring = NSBezierPath(ovalIn: bounds.insetBy(dx: 1, dy: 1))
      ring.lineWidth = 2
      NSColor.webtabsAccent.setStroke()
      ring.stroke()
    }
  }
  override func accessibilityRole() -> NSAccessibility.Role? { .radioButton }
  override func accessibilityValue() -> Any? { isActive ? 1 : 0 }
}

/// A miniature WebTabs window showing the chosen icon size and title bar colour,
/// with the user's own tab icons.
final class AppearancePreview: NSView {
  let bar = TitleBar()
  private let tabs = NSStackView()
  private var barHeight: NSLayoutConstraint!
  private var metrics = BarMetrics(iconSize: 22)
  private var color: NSColor = .windowBackgroundColor
  private static let pageHeight: CGFloat = 58

  init() {
    super.init(frame: .zero)
    translatesAutoresizingMaskIntoConstraints = false
    wantsLayer = true
    layer?.cornerRadius = 10
    layer?.masksToBounds = true
    tabs.spacing = 8
    let lights = TrafficLights()
    for view in [bar, tabs, lights] as [NSView] {
      view.translatesAutoresizingMaskIntoConstraints = false
    }
    addSubview(bar)
    bar.addSubview(tabs)
    bar.addSubview(lights)
    barHeight = bar.heightAnchor.constraint(equalToConstant: metrics.height)
    let height = heightAnchor.constraint(equalTo: bar.heightAnchor, constant: Self.pageHeight)
    NSLayoutConstraint.activate([
      bar.topAnchor.constraint(equalTo: topAnchor),
      bar.leadingAnchor.constraint(equalTo: leadingAnchor),
      bar.trailingAnchor.constraint(equalTo: trailingAnchor),
      barHeight, height,
      tabs.leadingAnchor.constraint(equalTo: bar.leadingAnchor, constant: 88),
      tabs.topAnchor.constraint(equalTo: bar.topAnchor),
      tabs.bottomAnchor.constraint(equalTo: bar.bottomAnchor),
      lights.leadingAnchor.constraint(equalTo: bar.leadingAnchor, constant: 20),
      lights.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
    ])
    setAccessibilityElement(true)
    setAccessibilityRole(.image)
    setAccessibilityLabel("Preview of the tab bar")
    show([nil, nil, nil])
  }
  required init?(coder: NSCoder) { fatalError() }

  override var isFlipped: Bool { true }
  // A picture, not controls.
  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  /// The first tabs' icons (nil: placeholder); the first is active with unread mail.
  func show(_ icons: [NSImage?]) {
    tabs.arrangedSubviews.forEach { $0.removeFromSuperview() }
    for (i, image) in (icons.isEmpty ? [nil, nil, nil] : icons).enumerated() {
      let b = TabButton()
      b.icon.image = image ?? Icons.placeholder
      b.icon.contentTintColor = image == nil ? .secondaryLabelColor : nil
      b.isActive = i == 0
      b.badge.text = i == 0 ? "3" : ""
      b.apply(metrics)
      b.setBarColor(color)
      tabs.addArrangedSubview(b)
    }
  }

  func update(_ m: BarMetrics, tint: String?) {
    metrics = m
    color = Tint.color(tint) ?? .windowBackgroundColor
    bar.color = color
    bar.appearance = Tint.appearance(tint)
    barHeight.constant = m.height
    for case let b as TabButton in tabs.arrangedSubviews {
      b.apply(m)
      b.setBarColor(color)
    }
    needsDisplay = true
  }

  override func draw(_ dirtyRect: NSRect) {
    NSColor.textBackgroundColor.setFill()
    bounds.fill()
    NSColor.separatorColor.setFill()
    NSRect(x: 0, y: metrics.height, width: bounds.width, height: 1).fill()
    // A page: a few grey lines.
    NSColor.labelColor.withAlphaComponent(0.07).setFill()
    for (i, width) in [0.42, 0.7, 0.55].enumerated() {
      NSBezierPath(
        roundedRect: NSRect(
          x: 20, y: metrics.height + 14 + CGFloat(i) * 13, width: bounds.width * width, height: 6),
        xRadius: 3, yRadius: 3
      ).fill()
    }
  }

  override func viewDidMoveToSuperview() {
    super.viewDidMoveToSuperview()
    layer?.borderWidth = 1
    layer?.borderColor = NSColor.separatorColor.cgColor
  }
}

/// The window buttons, drawn where the real ones sit.
private final class TrafficLights: NSView {
  override var intrinsicContentSize: NSSize { NSSize(width: 52, height: 12) }
  override func draw(_ dirtyRect: NSRect) {
    for (i, hex) in [0xFF5F57, 0xFEBC2E, 0x28C840].enumerated() {
      Tint.rgb(UInt32(hex)).setFill()
      NSBezierPath(ovalIn: NSRect(x: CGFloat(i) * 20, y: 0, width: 12, height: 12)).fill()
    }
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
  private var leading: NSLayoutConstraint!
  var isEnabled = true {
    didSet {
      toggle.isEnabled = isEnabled
      title.textColor = isEnabled ? .labelColor : .disabledControlTextColor
      subtitle.textColor = isEnabled ? .secondaryLabelColor : .disabledControlTextColor
    }
  }
  /// Indents a row that depends on the one above it.
  var indent: CGFloat = 0 {
    didSet { leading.constant = indent }
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
    self.title.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .medium)
    self.subtitle.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    self.subtitle.textColor = .secondaryLabelColor
    toggle.state = on ? .on : .off
    toggle.controlSize = .small
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
    text.setHuggingPriority(.defaultLow, for: .vertical)
    row.setHuggingPriority(.defaultLow, for: .vertical)
    row.translatesAutoresizingMaskIntoConstraints = false
    addSubview(row)
    leading = row.leadingAnchor.constraint(equalTo: leadingAnchor)
    NSLayoutConstraint.activate([
      leading,
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
