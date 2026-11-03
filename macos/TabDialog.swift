import AppKit

/// The add/edit tab form: one-click presets on top (when adding), address and
/// name, and the rarely needed knobs folded into Options. Adding from the title
/// bar's + shows it in a popover; editing, or adding from Settings, in a sheet.
/// Same fields and rules as src/linux/tab_dialog.rs.
final class TabDialog: NSObject, NSTextFieldDelegate, NSPopoverDelegate {
  /// Keeps each form alive while it is shown.
  private(set) static var open: [TabDialog] = []

  /// As a sheet on `parent`. Calls `done` with the new/edited tab; nothing on cancel.
  static func present(
    on parent: NSWindow, editing: Tab?, existing: [Tab], done: @escaping (Tab) -> Void
  ) {
    if let sheet = parent.attachedSheet {
      sheet.makeKeyAndOrderFront(nil)
      return
    }
    let dialog = TabDialog(editing: editing, existing: existing, done: done)
    let window = NSWindow(
      contentRect: .zero, styleMask: [.titled], backing: .buffered, defer: true)
    window.title = dialog.heading.stringValue
    window.contentView = dialog.content
    window.setContentSize(dialog.content.fittingSize)
    window.initialFirstResponder = dialog.url
    dialog.dismiss = { [weak window] in
      if let window { window.sheetParent?.endSheet(window) }
    }
    open.append(dialog)
    parent.beginSheet(window) { _ in open.removeAll { $0 === dialog } }
  }

  /// As a popover under `anchor` (the title bar's +); a second click closes it.
  static func popover(from anchor: NSView, existing: [Tab], done: @escaping (Tab) -> Void) {
    if let shown = open.first(where: { $0.popover?.isShown == true }) {
      shown.popover?.close()
      return
    }
    let dialog = TabDialog(editing: nil, existing: existing, done: done)
    let popover = NSPopover()
    popover.behavior = .transient
    popover.delegate = dialog
    let controller = NSViewController()
    controller.view = dialog.content
    popover.contentViewController = controller
    dialog.popover = popover
    dialog.dismiss = { [weak popover] in popover?.close() }
    open.append(dialog)
    popover.show(relativeTo: anchor.bounds, of: anchor, preferredEdge: .maxY)
    dialog.content.window?.makeFirstResponder(dialog.url)
  }

  func popoverDidClose(_ notification: Notification) {
    Self.open.removeAll { $0 === self }
  }

  /// The form itself, hosted by the popover or the sheet.
  let content = NSStackView()
  let heading = NSTextField(labelWithString: "")
  let url = NSTextField()
  let name = NSTextField()
  let tag = NSTextField()
  let ownLogin = NSSwitch()
  let notify = NSSwitch()
  let ua = NSSegmentedControl()
  private(set) var popover: NSPopover?
  private var dismiss: () -> Void = {}
  private let editing: Tab?
  private let existing: [Tab]
  private let done: (Tab) -> Void
  private let urlError = caption("Enter a web address, like example.com", error: true)
  private let isoHint = caption("")
  private let chevron = NSButton()
  private let summary = NSTextField(labelWithString: "")
  private let options = NSStackView()
  private let uaCustom = NSTextField()
  /// Browser identities offered: nil is the default (Safari).
  private let agents: [(name: String, value: String?)]

  private static let width: CGFloat = 400

  private init(editing: Tab?, existing: [Tab], done: @escaping (Tab) -> Void) {
    (self.editing, self.existing, self.done) = (editing, existing, done)
    agents =
      [("Safari", nil)]
      + Core.userAgents.presets.filter { $0.count == 2 }.map { ($0[0], $0[1]) }
    super.init()
    let adding = editing == nil
    heading.stringValue = adding ? "Add a web app" : "Edit tab"
    heading.font = .systemFont(ofSize: 15, weight: .semibold)

    func field(_ f: NSTextField, _ value: String?, _ placeholder: String, _ label: String) {
      f.stringValue = value ?? ""
      f.placeholderString = placeholder
      f.delegate = self
      f.lineBreakMode = .byTruncatingTail
      f.cell?.isScrollable = true
      f.setAccessibilityLabel(label)
    }
    field(url, editing?.url, "app.example.com", "Web address")
    field(name, editing?.name, "Shown when you hover the tab", "Name")
    field(tag, editing?.label, "W", "Icon tag")
    tag.alignment = .center
    tag.widthAnchor.constraint(equalToConstant: 44).isActive = true
    ownLogin.state = editing?.session != nil ? .on : .off
    notify.state = editing?.notifications == false ? .off : .on
    for control in [ownLogin, notify] {
      control.target = self
      control.action = #selector(optionChanged)
    }
    ownLogin.setAccessibilityLabel("Own login")
    notify.setAccessibilityLabel("Notifications")

    ua.segmentCount = agents.count + 1
    for (i, agent) in agents.enumerated() { ua.setLabel(agent.name, forSegment: i) }
    ua.setLabel("Custom…", forSegment: agents.count)
    ua.trackingMode = .selectOne
    ua.target = self
    ua.action = #selector(syncCustom)
    ua.setAccessibilityLabel("Browser identity")
    uaCustom.placeholderString = Core.userAgents.defaultAgent
    uaCustom.lineBreakMode = .byTruncatingTail
    uaCustom.cell?.isScrollable = true
    uaCustom.delegate = self
    uaCustom.setAccessibilityLabel("Custom browser identity")
    let current = editing?.userAgent
    if let index = agents.firstIndex(where: { $0.value == current }) {
      ua.selectedSegment = index
    } else {
      ua.selectedSegment = agents.count
      uaCustom.stringValue = current ?? ""
    }

    // Options: folded to one summary line unless something is set.
    chevron.bezelStyle = .disclosure
    chevron.setButtonType(.pushOnPushOff)
    chevron.title = ""
    chevron.target = self
    chevron.action = #selector(toggleOptions)
    chevron.setAccessibilityLabel("Options")
    let optionsTitle = NSButton(title: "Options", target: self, action: #selector(flipOptions))
    optionsTitle.isBordered = false
    optionsTitle.font = .systemFont(ofSize: 13, weight: .medium)
    optionsTitle.setAccessibilityElement(false)
    summary.textColor = .secondaryLabelColor
    summary.lineBreakMode = .byTruncatingTail
    summary.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    let optionsRow = NSStackView(views: [chevron, optionsTitle, summary])
    optionsRow.spacing = 4
    optionsRow.setCustomSpacing(8, after: optionsTitle)

    options.orientation = .vertical
    options.alignment = .leading
    options.spacing = 12
    options.edgeInsets = NSEdgeInsets(top: 0, left: 18, bottom: 0, right: 0)
    let uaBlock = NSStackView(views: [ua, uaCustom, caption("Change it only if a site says your browser isn’t supported.", width: 320)])
    uaBlock.orientation = .vertical
    uaBlock.alignment = .leading
    uaBlock.spacing = 6
    uaCustom.widthAnchor.constraint(equalTo: uaBlock.widthAnchor).isActive = true
    for view in [
      optionLine("Own login", "Separate cookies and storage, for a second account", ownLogin),
      optionLine("Notifications", "Unread badges show either way", notify),
      optionLine("Icon tag", "One or two letters on the icon, to tell similar tabs apart", tag),
      labelled("Browser identity", uaBlock, small: false),
    ] {
      options.addArrangedSubview(view)
      view.widthAnchor.constraint(equalTo: options.widthAnchor, constant: -18).isActive = true
    }
    chevron.state =
      !adding || editing?.session != nil || editing?.notifications == false
        || editing?.label != nil || current != nil ? .on : .off

    let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel))
    cancel.keyEquivalent = "\u{1b}"
    let ok = NSButton(title: adding ? "Add Tab" : "Save", target: self, action: #selector(submit))
    ok.keyEquivalent = "\r"
    let buttons = NSStackView(views: [NSView(), cancel, ok])

    content.orientation = .vertical
    content.alignment = .leading
    content.spacing = 14
    content.edgeInsets = NSEdgeInsets(top: 18, left: 18, bottom: 18, right: 18)
    content.addArrangedSubview(heading)
    if adding { content.addArrangedSubview(presetsRow()) }
    for view in [
      labelled(adding ? "Or any web address" : "Web address", url),
      urlError,
      labelled("Name", name),
      isoHint, optionsRow, options, buttons,
    ] {
      content.addArrangedSubview(view)
    }
    content.setCustomSpacing(6, after: urlError)
    content.setCustomSpacing(8, after: optionsRow)
    content.setCustomSpacing(18, after: options)
    content.translatesAutoresizingMaskIntoConstraints = false
    for view in content.arrangedSubviews where view !== heading && view !== optionsRow {
      view.widthAnchor.constraint(equalTo: content.widthAnchor, constant: -36).isActive = true
    }
    content.widthAnchor.constraint(equalToConstant: Self.width).isActive = true
    urlError.isHidden = true

    updateHint()
    syncCustom()
    toggleOptions()
    updateSummary()
  }

  /// Title and detail on the left, the control on the right.
  private func optionLine(_ title: String, _ detail: String, _ control: NSView) -> NSView {
    let titleLabel = NSTextField(labelWithString: title)
    titleLabel.font = .systemFont(ofSize: 13, weight: .medium)
    let text = NSStackView(views: [titleLabel, caption(detail, width: 260)])
    text.orientation = .vertical
    text.alignment = .leading
    text.spacing = 2
    text.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
    let row = NSStackView(views: [text, control])
    row.distribution = .fill
    row.alignment = .centerY
    row.spacing = 12
    return row
  }

  /// A small label above its control.
  private func labelled(_ title: String, _ control: NSView, small: Bool = true) -> NSView {
    let label = NSTextField(labelWithString: title)
    if small {
      label.font = .systemFont(ofSize: 12)
      label.textColor = .secondaryLabelColor
    } else {
      label.font = .systemFont(ofSize: 13, weight: .medium)
    }
    let stack = NSStackView(views: [label, control])
    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 6
    control.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = small
    return stack
  }

  /// The service chips; clicking one fills in address and name.
  private func presetsRow() -> NSView {
    let row = NSStackView()
    row.distribution = .fillEqually
    row.spacing = 8
    for preset in Core.presets {
      let tile = Tile(size: NSSize(width: 80, height: 72))
      tile.radius = 10
      tile.restingAlpha = 0.045
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
      text.font = .systemFont(ofSize: 12)
      let column = NSStackView(views: [icon, text])
      column.orientation = .vertical
      column.spacing = 6
      column.translatesAutoresizingMaskIntoConstraints = false
      tile.addSubview(column)
      NSLayoutConstraint.activate([
        icon.widthAnchor.constraint(equalToConstant: 32),
        icon.heightAnchor.constraint(equalToConstant: 32),
        column.centerXAnchor.constraint(equalTo: tile.centerXAnchor),
        column.centerYAnchor.constraint(equalTo: tile.centerYAnchor),
      ])
      tile.toolTip = preset.url
      tile.setAccessibilityLabel(preset.name)
      tile.onClick = { [weak self] in
        guard let self else { return }
        self.url.stringValue = preset.url
        self.name.stringValue = preset.name
        self.urlError.isHidden = true
        self.updateHint()
        self.content.window?.makeFirstResponder(self.name)
      }
      row.addArrangedSubview(tile)
    }
    return row
  }

  // ---- behaviour ------------------------------------------------------------

  /// Shown, as a popover or sheet.
  var isShowing: Bool { popover?.isShown ?? (content.window?.sheetParent != nil) }
  var errorVisible: Bool { !urlError.isHidden }
  var hintVisible: Bool { !isoHint.isHidden }
  var optionsVisible: Bool { !options.isHidden }
  var customAgentVisible: Bool { !uaCustom.isHidden }
  var summaryText: String { summary.stringValue }

  func controlTextDidChange(_ note: Notification) {
    if note.object as? NSTextField === tag, tag.stringValue.count > 2 {
      tag.stringValue = String(tag.stringValue.prefix(2))
    }
    if note.object as? NSTextField === url {
      urlError.isHidden = true
      updateHint()
    }
    updateSummary()
  }

  /// Warns when a second tab on the same site would share the first one's login.
  func updateHint() {
    let site = Core.site(url.stringValue)
    let twin = existing.first { t in
      t.url != editing?.url && t.session == nil && site != nil && Core.site(t.url) == site
    }
    if let twin, ownLogin.state != .on {
      isoHint.stringValue =
        "This would share its login with your “\(twin.name)” tab. Turn on Own login in Options to use a different account."
      isoHint.isHidden = false
    } else {
      isoHint.isHidden = true
    }
  }

  /// One line saying what Options holds, so it can stay folded.
  private func updateSummary() {
    var parts = [ownLogin.state == .on ? "Own login" : "Shared login"]
    parts.append(notify.state == .on ? "Notifications on" : "Muted")
    let label = tag.stringValue.trimmingCharacters(in: .whitespaces)
    if !label.isEmpty { parts.append("Tag \(label)") }
    if ua.selectedSegment > 0 { parts.append(ua.label(forSegment: ua.selectedSegment) ?? "") }
    summary.stringValue = parts.joined(separator: " · ")
  }

  @objc func optionChanged() {
    updateHint()
    updateSummary()
  }

  @objc func syncCustom() {
    let custom = ua.selectedSegment == agents.count
    uaCustom.isHidden = !custom
    if custom { content.window?.makeFirstResponder(uaCustom) }
    updateSummary()
  }

  @objc func flipOptions() {
    chevron.state = chevron.state == .on ? .off : .on
    toggleOptions()
  }

  @objc private func toggleOptions() {
    options.isHidden = chevron.state != .on
    summary.isHidden = chevron.state == .on
  }

  @objc func cancel() { dismiss() }

  @objc func submit() {
    let agent: Any =
      ua.selectedSegment == agents.count
      ? uaCustom.stringValue
      : (agents[max(0, ua.selectedSegment)].value.map { $0 as Any } ?? NSNull())
    guard
      let made: MadeTab = try? Core.call(
        "make_tab",
        [
          "name": name.stringValue, "url": url.stringValue, "label": tag.stringValue,
          "user_agent": agent,
        ])
    else { return }
    guard made.valid else {
      // Keep everything typed so far; point at the address.
      urlError.isHidden = false
      content.window?.makeFirstResponder(url)
      url.currentEditor()?.selectAll(nil)
      return
    }
    var tab = made.tab
    tab.notifications = notify.state == .on
    if ownLogin.state == .on {
      // Keep the existing storage when editing an already isolated tab.
      tab.session =
        editing?.session
        ?? Core.get(
          "session_key", ["name": tab.name, "taken": existing.compactMap(\.session)], or: "tab")
    }
    dismiss()
    done(tab)
  }
}

func caption(_ text: String, error: Bool = false, width: CGFloat = 360) -> NSTextField {
  let c = NSTextField(wrappingLabelWithString: text)
  c.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
  c.textColor = error ? .systemRed : .secondaryLabelColor
  c.preferredMaxLayoutWidth = width
  return c
}
