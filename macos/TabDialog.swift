import AppKit

/// Add/edit tab sheet: one-click presets on top (when adding), the form below,
/// rarely needed knobs (user agent) folded into "Advanced". Mirrors
/// src/linux/tab_dialog.rs.
final class TabDialog: NSObject, NSTextFieldDelegate {
  /// Keeps each dialog alive while its sheet is up.
  private(set) static var open: [TabDialog] = []

  /// Calls `done` with the new/edited tab; nothing on cancel.
  static func present(
    on parent: NSWindow, editing: Tab?, existing: [Tab], done: @escaping (Tab) -> Void
  ) {
    if let sheet = parent.attachedSheet {
      sheet.makeKeyAndOrderFront(nil)
      return
    }
    let dialog = TabDialog(editing: editing, existing: existing, done: done)
    open.append(dialog)
    parent.beginSheet(dialog.window) { _ in open.removeAll { $0 === dialog } }
  }

  let window: NSWindow
  private let editing: Tab?
  private let existing: [Tab]
  private let done: (Tab) -> Void
  private let content = NSStackView()
  let form = NSGridView()
  let url = NSTextField()
  let name = NSTextField()
  let tag = NSTextField()
  private let urlError = caption("Enter a web address, like https://example.com", error: true)
  let isolated = NSButton(
    checkboxWithTitle: "Isolated login (own cookies, for a second account)", target: nil,
    action: nil)
  private let isoHint = caption("")
  private let mute = NSButton(checkboxWithTitle: "Mute notifications", target: nil, action: nil)
  private let disclosure = NSButton()
  let advanced = NSGridView()
  let ua = NSPopUpButton()
  private let uaCustom = NSTextField()

  private static let customTag = -1

  private init(editing: Tab?, existing: [Tab], done: @escaping (Tab) -> Void) {
    (self.editing, self.existing, self.done) = (editing, existing, done)
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 500, height: 300), styleMask: [.titled],
      backing: .buffered, defer: true)
    super.init()
    let adding = editing == nil
    window.title = adding ? "Add tab" : "Edit tab"

    let heading = NSTextField(labelWithString: window.title)
    heading.font = .boldSystemFont(ofSize: 13)

    func field(_ f: NSTextField, _ value: String?, _ placeholder: String) {
      f.stringValue = value ?? ""
      f.placeholderString = placeholder
      f.delegate = self
      f.lineBreakMode = .byTruncatingTail
      f.cell?.isScrollable = true
    }
    field(url, editing?.url, "https://… or pick a service above")
    field(name, editing?.name, "shown when you hover the icon")
    field(tag, editing?.label, "e.g. W")
    url.setAccessibilityLabel("URL")
    name.setAccessibilityLabel("Name")
    tag.setAccessibilityLabel("Icon tag")
    tag.toolTip = "Optional 1-2 letters drawn on the icon, to tell similar tabs apart"
    tag.widthAnchor.constraint(equalToConstant: 64).isActive = true
    isolated.state = editing?.session != nil ? .on : .off
    isolated.target = self
    isolated.action = #selector(updateHint)
    mute.state = editing?.notifications == false ? .on : .off

    for row in [
      [label("URL"), url], [NSGridCell.emptyContentView, urlError], [label("Name"), name],
      [label("Icon tag"), tag], [NSGridCell.emptyContentView, isolated],
      [NSGridCell.emptyContentView, isoHint], [NSGridCell.emptyContentView, mute],
    ] {
      form.addRow(with: row)
    }
    style(form)
    form.cell(for: tag)?.xPlacement = .leading
    form.row(at: 1).isHidden = true

    // Advanced: user agent.
    ua.addItem(withTitle: "Default (Safari)")
    for preset in Core.userAgents.presets where preset.count == 2 {
      ua.addItem(withTitle: preset[0])
      ua.lastItem?.representedObject = preset[1]
    }
    ua.addItem(withTitle: "Custom…")
    ua.lastItem?.tag = Self.customTag
    ua.target = self
    ua.action = #selector(syncCustom)
    ua.setAccessibilityLabel("User agent")
    uaCustom.placeholderString = Core.userAgents.defaultAgent
    uaCustom.lineBreakMode = .byTruncatingTail
    uaCustom.cell?.isScrollable = true
    uaCustom.setAccessibilityLabel("Custom user agent")
    let current = editing?.userAgent
    if let current {
      if let i = ua.itemArray.firstIndex(where: { $0.representedObject as? String == current }) {
        ua.selectItem(at: i)
      } else {
        ua.selectItem(withTag: Self.customTag)
        uaCustom.stringValue = current
      }
    }
    let uaHint = caption(
      "Change only if a site says your browser isn’t supported. The tab reloads.")
    advanced.addRow(with: [label("User agent"), ua])
    advanced.addRow(with: [NSGridCell.emptyContentView, uaCustom])
    advanced.addRow(with: [NSGridCell.emptyContentView, uaHint])
    style(advanced)
    advanced.cell(for: ua)?.xPlacement = .leading
    // Keep both grids' columns aligned.
    form.column(at: 0).width = 70
    advanced.column(at: 0).width = 70

    disclosure.bezelStyle = .disclosure
    disclosure.setButtonType(.pushOnPushOff)
    disclosure.title = ""
    disclosure.target = self
    disclosure.action = #selector(toggleAdvanced)
    disclosure.setAccessibilityLabel("Advanced")
    let advancedLabel = NSButton(title: "Advanced", target: self, action: #selector(flipAdvanced))
    advancedLabel.isBordered = false
    advancedLabel.setAccessibilityElement(false)
    let advancedRow = NSStackView(views: [disclosure, advancedLabel])
    advancedRow.spacing = 2
    disclosure.state = current != nil ? .on : .off

    let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel))
    cancel.keyEquivalent = "\u{1b}"
    let ok = NSButton(title: adding ? "Add tab" : "Save", target: self, action: #selector(submit))
    ok.keyEquivalent = "\r"
    let buttons = NSStackView(views: [NSView(), cancel, ok])

    content.orientation = .vertical
    content.alignment = .leading
    content.spacing = 14
    content.edgeInsets = NSEdgeInsets(top: 18, left: 18, bottom: 18, right: 18)
    content.addArrangedSubview(heading)
    if adding { content.addArrangedSubview(presetsRow()) }
    for view in [form, advancedRow, advanced, buttons] { content.addArrangedSubview(view) }
    content.setCustomSpacing(6, after: advancedRow)
    content.setCustomSpacing(20, after: advanced)
    content.translatesAutoresizingMaskIntoConstraints = false
    for view in content.arrangedSubviews where view !== heading && view !== advancedRow {
      view.widthAnchor.constraint(equalTo: content.widthAnchor, constant: -36).isActive = true
    }
    window.contentView = content
    content.widthAnchor.constraint(equalToConstant: 500).isActive = true

    updateHint()
    syncCustom()
    toggleAdvanced()
    window.setContentSize(content.fittingSize)
    window.initialFirstResponder = url
  }

  private func style(_ grid: NSGridView) {
    grid.rowSpacing = 8
    grid.columnSpacing = 12
    grid.rowAlignment = .firstBaseline
    grid.column(at: 0).xPlacement = .trailing
    grid.column(at: 1).xPlacement = .fill
  }

  /// The row of service buttons; clicking one fills in URL and name.
  private func presetsRow() -> NSView {
    let row = NSStackView()
    row.distribution = .fillEqually
    row.spacing = 8
    for preset in Core.presets {
      let tile = Tile(size: NSSize(width: 100, height: 82))
      tile.radius = 12
      tile.removeConstraints(tile.constraints.filter { $0.firstAttribute == .width })
      let icon = NSImageView()
      icon.imageScaling = .scaleProportionallyUpOrDown
      icon.image = Icons.cached(preset.url) ?? Icons.placeholder
      if icon.image === Icons.placeholder {
        icon.contentTintColor = .secondaryLabelColor
        // Icons come from the favicon cache, or are fetched on first use.
        Icons.fetch(Core.favicons(page: preset.url, links: "[]")) { [weak icon] image in
          guard let image else { return }
          Icons.store(preset.url, image)
          icon?.image = image
        }
      }
      let text = NSTextField(labelWithString: preset.name)
      let column = NSStackView(views: [icon, text])
      column.orientation = .vertical
      column.spacing = 6
      column.translatesAutoresizingMaskIntoConstraints = false
      tile.addSubview(column)
      NSLayoutConstraint.activate([
        icon.widthAnchor.constraint(equalToConstant: 40),
        icon.heightAnchor.constraint(equalToConstant: 40),
        column.centerXAnchor.constraint(equalTo: tile.centerXAnchor),
        column.centerYAnchor.constraint(equalTo: tile.centerYAnchor),
      ])
      tile.toolTip = preset.url
      tile.setAccessibilityLabel(preset.name)
      tile.onClick = { [weak self] in
        guard let self else { return }
        self.url.stringValue = preset.url
        self.name.stringValue = preset.name
        self.form.row(at: 1).isHidden = true
        self.updateHint()
        self.window.makeFirstResponder(self.name)
      }
      row.addArrangedSubview(tile)
    }
    return row
  }

  // ---- behaviour ------------------------------------------------------------

  func controlTextDidChange(_ note: Notification) {
    if note.object as? NSTextField === tag, tag.stringValue.count > 2 {
      tag.stringValue = String(tag.stringValue.prefix(2))
    }
    if note.object as? NSTextField === url {
      form.row(at: 1).isHidden = true
      updateHint()
    }
  }

  /// Warns when a second tab on the same site would share the first one's login.
  @objc func updateHint() {
    let site = Core.site(url.stringValue)
    let twin = existing.first { t in
      t.url != editing?.url && t.session == nil && site != nil && Core.site(t.url) == site
    }
    if let twin, isolated.state != .on {
      isoHint.stringValue =
        "Shares its login with your “\(twin.name)” tab. Select Isolated login to use a different account."
      form.row(at: 5).isHidden = false
    } else {
      form.row(at: 5).isHidden = true
    }
  }

  @objc func syncCustom() {
    advanced.row(at: 1).isHidden = ua.selectedTag() != Self.customTag
    if ua.selectedTag() == Self.customTag { window.makeFirstResponder(uaCustom) }
  }

  @objc func flipAdvanced() {
    disclosure.state = disclosure.state == .on ? .off : .on
    toggleAdvanced()
  }

  @objc private func toggleAdvanced() {
    advanced.isHidden = disclosure.state != .on
  }

  @objc func cancel() {
    window.sheetParent?.endSheet(window)
  }

  @objc func submit() {
    let userAgent: Any =
      ua.selectedTag() == Self.customTag
      ? uaCustom.stringValue
      : (ua.selectedItem?.representedObject as? String).map { $0 as Any } ?? NSNull()
    guard
      let made: MadeTab = try? Core.call(
        "make_tab",
        [
          "name": name.stringValue, "url": url.stringValue, "label": tag.stringValue,
          "user_agent": userAgent,
        ])
    else { return }
    guard made.valid else {
      // Keep everything typed so far; point at the URL.
      form.row(at: 1).isHidden = false
      window.makeFirstResponder(url)
      url.currentEditor()?.selectAll(nil)
      return
    }
    var tab = made.tab
    tab.notifications = mute.state != .on
    if isolated.state == .on {
      // Keep the existing storage when editing an already isolated tab.
      tab.session =
        editing?.session
        ?? Core.get(
          "session_key", ["name": tab.name, "taken": existing.compactMap(\.session)], or: "tab")
    }
    window.sheetParent?.endSheet(window)
    done(tab)
  }
}

private func label(_ text: String) -> NSTextField {
  NSTextField(labelWithString: text)
}

func caption(_ text: String, error: Bool = false) -> NSTextField {
  let c = NSTextField(wrappingLabelWithString: text)
  c.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
  c.textColor = error ? .systemRed : .secondaryLabelColor
  c.preferredMaxLayoutWidth = 370
  return c
}
