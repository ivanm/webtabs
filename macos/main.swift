// macOS front end: an AppKit window with the Linux icon tab bar, one WKWebView
// per tab, a menu bar icon, notifications, settings and a hot-reloaded config
// file. Behaviour follows src/linux/mod.rs; rules come from the Rust core.

import AppKit
import CryptoKit
import UserNotifications
import WebKit

/// One tab: its config entry, web view and bar button (Linux `TabView`).
final class TabView {
  let id: Int
  var tab: Tab
  let web: WKWebView
  let button = TabButton()
  var icon: NSImage?
  var iconFetched = false
  /// Bumped when the tab's URL changes, so a slow fetch for the old URL is ignored.
  var iconGeneration = 0
  var badge = Badge()
  var loaded = false
  /// The page's id for the notification currently shown for this tab.
  var notificationID: String?
  var crashes: [Date] = []
  var titleObservation: NSKeyValueObservation?

  init(id: Int, tab: Tab, web: WKWebView) {
    (self.id, self.tab, self.web) = (id, tab, web)
    setIcon(nil)
    updateTooltip()
    updateTag()
  }

  func setIcon(_ image: NSImage?) {
    icon = image
    button.icon.image = image ?? Icons.placeholder
    button.icon.contentTintColor = image == nil ? .secondaryLabelColor : nil
  }

  func updateTag() { button.tagPill.text = tab.label ?? "" }

  func updateTooltip() {
    let unread =
      badge.count > 0 ? " — \(badge.count) unread" : badge.unread ? " — new activity" : ""
    button.toolTip = tab.name + unread
    button.setAccessibilityLabel(tab.name + unread)
  }

  var notificationKey: String { "tab-\(id)" }
}

/// Stable WebKit profile id for a profile name.
func profileID(_ key: String) -> UUID {
  var bytes = Array(SHA256.hash(data: Data(key.utf8)).prefix(16))
  bytes[6] = (bytes[6] & 15) | 80
  bytes[8] = (bytes[8] & 63) | 128
  return UUID(
    uuid: (
      bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
      bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

final class App: NSObject, NSApplicationDelegate, NSWindowDelegate, NSMenuDelegate,
  NSMenuItemValidation, UNUserNotificationCenterDelegate, WKScriptMessageHandlerWithReply
{
  var window: NSWindow!
  /// Tab buttons, in tab order; the "+" sits right after it.
  let tabStack = NSStackView()
  let strip = TabStrip()
  /// Holds one web view per tab plus the empty state.
  let content = NSView()
  var emptyState: NSView!
  var config = Config()
  var configPath = ""
  var dataDir = ""
  var lastSaved = ""
  var lastRejected = ""
  var tabs: [TabView] = []
  var currentID = 0
  var nextID = 1
  var lastUnread = 0
  var status: NSStatusItem?
  let statusDot = NSView()
  var settingsWindow: SettingsWindow?
  /// Isolated tabs' private profiles, by session key.
  var isolated: [String: WKWebsiteDataStore] = [:]
  /// Isolated-session data scheduled for deletion; cancelled if the key returns.
  var pendingDeletes: [String: DispatchWorkItem] = [:]
  var popups: [ObjectIdentifier: Popup] = [:]
  var downloads: [ObjectIdentifier: URL] = [:]
  var loadTimer: Timer?
  var watchTimer: Timer?
  let smoke = CommandLine.arguments.contains("--smoke-test")
  var smokeDone = false

  var current: TabView? { tabs.first { $0.id == currentID } }

  /// `WEBTABS_DATA` picks a separate set of WebKit profiles (tests, second setups).
  private let namespace = ProcessInfo.processInfo.environment["WEBTABS_DATA"] ?? "default"
  /// Shared profile used by every non-isolated tab.
  lazy var sharedStore = WKWebsiteDataStore(forIdentifier: profileID(namespace + ":shared"))

  // ---- startup ----------------------------------------------------------------

  func applicationDidFinishLaunching(_ notification: Notification) {
    NSApp.setActivationPolicy(.regular)
    makeMenu()
    UNUserNotificationCenter.current().delegate = self
    let loaded: Loaded
    do {
      loaded = try Core.call("load")
    } catch {
      return startupFailed(error)
    }
    (config, configPath, dataDir) = (loaded.config, loaded.path, loaded.data)
    lastSaved = (try? String(contentsOfFile: configPath, encoding: .utf8)) ?? ""
    buildWindow()
    tabs = config.tabs.map(createTab)
    purgeStaleSessions()
    updateEmptyState()
    applyOptions()
    loadTabsProgressively()
    watchConfig()
    requestNotifications()
    let hidden = CommandLine.arguments.contains("--hidden") || launchedAsLoginItem()
    if !(hidden && status != nil) { showWindow() }
    if smoke {
      DispatchQueue.main.asyncAfter(deadline: .now() + 40) {
        if !self.smokeDone { self.smokeFail("WebKit timed out") }
      }
    }
  }

  /// A broken tabs.toml: don't run (saving from the UI would overwrite it).
  private func startupFailed(_ error: Error) {
    let alert = NSAlert()
    alert.alertStyle = .critical
    alert.messageText = "WebTabs can’t read tabs.toml"
    alert.informativeText = "\(error.localizedDescription)\n\nFix the file and open WebTabs again."
    alert.addButton(withTitle: "Quit")
    alert.addButton(withTitle: "Open tabs.toml")
    if alert.runModal() == .alertSecondButtonReturn,
      let path: String = try? Core.call("config_path")
    {
      openConfigFile(path)
    }
    NSApp.terminate(nil)
  }

  private func launchedAsLoginItem() -> Bool {
    guard let event = NSAppleEventManager.shared().currentAppleEvent else { return false }
    return event.eventID == AEEventID(kAEOpenApplication)
      && event.paramDescriptor(forKeyword: AEKeyword(keyAEPropData))?.enumCodeValue
        == OSType(keyAELaunchedAsLogInItem)
  }

  // ---- window & chrome ----------------------------------------------------------

  private func buildWindow() {
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 1280, height: 860),
      styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered,
      defer: false)
    window.title = "WebTabs"
    window.minSize = NSSize(width: 540, height: 400)
    window.delegate = self
    window.isReleasedWhenClosed = false
    window.tabbingMode = .disallowed
    if !window.setFrameUsingName("WebTabsMain") { window.center() }
    window.setFrameAutosaveName("WebTabsMain")

    // [tab][tab][tab][+] ............................ [gear]
    let bar = NSView()
    let add = SymbolTile("plus", label: "Add tab", tooltip: "Add tab (⌘T)")
    add.onClick = { [weak self] in self?.addTabDialog() }
    let gear = SymbolTile("gearshape", label: "Settings", tooltip: "Settings (⌘,)")
    gear.onClick = { [weak self] in self?.openSettings() }
    tabStack.orientation = .horizontal
    tabStack.spacing = 2
    tabStack.translatesAutoresizingMaskIntoConstraints = false
    tabStack.setAccessibilityElement(true)
    tabStack.setAccessibilityRole(.tabGroup)
    tabStack.setAccessibilityLabel("Tabs")
    strip.documentView = tabStack
    strip.drawsBackground = false
    strip.hasHorizontalScroller = false
    strip.verticalScrollElasticity = .none
    let line = NSBox()
    line.boxType = .separator
    emptyState = makeEmptyState()

    let root = window.contentView!
    for view in [bar, line, content] as [NSView] {
      view.translatesAutoresizingMaskIntoConstraints = false
      root.addSubview(view)
    }
    for view in [strip, add, gear] as [NSView] {
      view.translatesAutoresizingMaskIntoConstraints = false
      bar.addSubview(view)
    }
    emptyState.translatesAutoresizingMaskIntoConstraints = false
    content.addSubview(emptyState)
    let stripWidth = strip.widthAnchor.constraint(equalTo: tabStack.widthAnchor)
    stripWidth.priority = .defaultHigh  // shrinks (and scrolls) when tabs overflow
    NSLayoutConstraint.activate([
      bar.topAnchor.constraint(equalTo: root.topAnchor),
      bar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
      bar.trailingAnchor.constraint(equalTo: root.trailingAnchor),
      bar.heightAnchor.constraint(equalToConstant: 52),
      line.topAnchor.constraint(equalTo: bar.bottomAnchor),
      line.leadingAnchor.constraint(equalTo: root.leadingAnchor),
      line.trailingAnchor.constraint(equalTo: root.trailingAnchor),
      content.topAnchor.constraint(equalTo: line.bottomAnchor),
      content.leadingAnchor.constraint(equalTo: root.leadingAnchor),
      content.trailingAnchor.constraint(equalTo: root.trailingAnchor),
      content.bottomAnchor.constraint(equalTo: root.bottomAnchor),
      strip.leadingAnchor.constraint(equalTo: bar.leadingAnchor, constant: 6),
      strip.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      strip.heightAnchor.constraint(equalToConstant: 44),
      stripWidth,
      tabStack.leadingAnchor.constraint(equalTo: strip.contentView.leadingAnchor),
      tabStack.topAnchor.constraint(equalTo: strip.contentView.topAnchor),
      tabStack.bottomAnchor.constraint(equalTo: strip.contentView.bottomAnchor),
      add.leadingAnchor.constraint(equalTo: strip.trailingAnchor, constant: 4),
      add.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      add.trailingAnchor.constraint(lessThanOrEqualTo: gear.leadingAnchor, constant: -8),
      gear.trailingAnchor.constraint(equalTo: bar.trailingAnchor, constant: -6),
      gear.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      emptyState.centerXAnchor.constraint(equalTo: content.centerXAnchor),
      emptyState.centerYAnchor.constraint(equalTo: content.centerYAnchor),
    ])
  }

  private func makeEmptyState() -> NSView {
    let title = NSTextField(labelWithString: "No tabs yet")
    title.font = .systemFont(ofSize: 18, weight: .bold)
    let hint = NSTextField(labelWithString: "Press ⌘T, or open settings with the gear")
    hint.textColor = .secondaryLabelColor
    let button = NSButton(title: "Add a tab", target: self, action: #selector(addTab))
    let stack = NSStackView(views: [title, hint, button])
    stack.orientation = .vertical
    stack.spacing = 12
    stack.isHidden = true
    return stack
  }

  @objc func showWindow() {
    window.makeKeyAndOrderFront(nil)
    NSApp.activate()
  }

  @objc func toggleVisible() {
    if window.isVisible && window.isKeyWindow && NSApp.isActive {
      window.orderOut(nil)
    } else {
      showWindow()
    }
  }

  /// Never hide into a menu bar icon that isn't there.
  var canHideToTray: Bool { config.tray && config.closeToTray && status != nil }

  func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool
  {
    showWindow()
    return true
  }

  func windowShouldClose(_ sender: NSWindow) -> Bool {
    guard sender === window else { return true }
    if canHideToTray {
      sender.orderOut(nil)
    } else {
      NSApp.terminate(nil)
    }
    return false
  }

  func windowWillClose(_ notification: Notification) {
    if let closed = notification.object as? NSWindow, closed !== window { closePopup(closed) }
  }

  private func makeStatusItem() {
    let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    let image = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { _ in
      // The app icon's mark: four chevrons making a diamond around a square.
      NSColor.black.set()
      let (c, r, arm): (CGFloat, CGFloat, CGFloat) = (9, 6.8, 2.7)
      for (dx, dy) in [(0.0, 1.0), (1, 0), (0, -1), (-1, 0)] as [(CGFloat, CGFloat)] {
        let tip = NSPoint(x: c + dx * r, y: c + dy * r)
        let chevron = NSBezierPath()
        chevron.move(to: NSPoint(x: tip.x + arm * (-dy - dx), y: tip.y + arm * (dx - dy)))
        chevron.line(to: tip)
        chevron.line(to: NSPoint(x: tip.x + arm * (dy - dx), y: tip.y + arm * (-dx - dy)))
        chevron.lineWidth = 1.8
        chevron.lineJoinStyle = .round
        chevron.stroke()
      }
      NSBezierPath(
        roundedRect: NSRect(x: c - 2.3, y: c - 2.3, width: 4.6, height: 4.6), xRadius: 0.8,
        yRadius: 0.8
      ).fill()
      return true
    }
    image.isTemplate = true
    item.button?.image = image
    item.button?.setAccessibilityLabel("WebTabs")
    // Unread: a red dot on the icon, like Linux's webtabs-unread icon.
    statusDot.wantsLayer = true
    statusDot.layer?.backgroundColor = NSColor.systemRed.cgColor
    statusDot.layer?.cornerRadius = 3.5
    statusDot.frame = NSRect(x: 0, y: 0, width: 7, height: 7)
    statusDot.isHidden = true
    if let button = item.button {
      button.addSubview(statusDot)
      statusDot.frame.origin = NSPoint(x: button.bounds.maxX - 9, y: button.bounds.maxY - 10)
      statusDot.autoresizingMask = [.minXMargin, .minYMargin]
    }
    let menu = NSMenu()
    menu.delegate = self
    menu.addItem(withTitle: "Show WebTabs", action: #selector(toggleVisible), keyEquivalent: "")
      .target = self
    menu.addItem(withTitle: "Settings…", action: #selector(openSettings), keyEquivalent: "")
      .target = self
    menu.addItem(.separator())
    menu.addItem(withTitle: "Quit WebTabs", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "")
    item.menu = menu
    status = item
  }

  func menuNeedsUpdate(_ menu: NSMenu) {
    let shown = window.isVisible && window.isKeyWindow && NSApp.isActive
    menu.items.first?.title = shown ? "Hide WebTabs" : "Show WebTabs"
  }

  func updateEmptyState() {
    if tabs.isEmpty {
      currentID = 0
      emptyState.isHidden = false
    } else if current == nil {
      select(tabs[0])
    }
  }

  // ---- options --------------------------------------------------------------------

  /// Changes app-wide options from the settings window and persists them.
  func updateOptions(_ change: (inout Config) -> Void) {
    let before = config
    change(&config)
    if (config.notifications && !before.notifications)
      || (config.unreadInDock && !before.unreadInDock)
    {
      requestNotifications()
    }
    applyOptions()
    save()
  }

  func applyOptions() {
    if config.tray && status == nil {
      makeStatusItem()
    } else if !config.tray, let status {
      NSStatusBar.system.removeStatusItem(status)
      self.status = nil
    }
    settingsWindow?.optionsChanged()
    refreshUnread()
  }

  func showError(_ title: String, _ error: Error, in parent: NSWindow? = nil) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = error.localizedDescription
    if let parent = parent ?? window, parent.isVisible {
      alert.beginSheetModal(for: parent)
    } else {
      alert.runModal()
    }
  }

  func refreshSettings() { settingsWindow?.refresh() }

  @objc func openSettings() {
    // One settings window at a time (gear, ⌘, and the menu bar can all open it).
    if settingsWindow == nil { settingsWindow = SettingsWindow(app: self) }
    settingsWindow?.show()
  }

  /// Opens tabs.toml in the user's editor (TextEdit when nothing claims .toml).
  func openConfigFile(_ path: String? = nil) {
    let url = URL(fileURLWithPath: path ?? configPath)
    if NSWorkspace.shared.urlForApplication(toOpen: url) != nil {
      NSWorkspace.shared.open(url)
    } else if let editor = NSWorkspace.shared.urlForApplication(
      withBundleIdentifier: "com.apple.TextEdit")
    {
      NSWorkspace.shared.open([url], withApplicationAt: editor, configuration: NSWorkspace.OpenConfiguration())
    }
  }

  // ---- tabs -------------------------------------------------------------------------

  func createTab(_ tab: Tab) -> TabView {
    let wc = WKWebViewConfiguration()
    wc.websiteDataStore = store(for: tab.session)
    // Needed for OAuth/Meet popups; external opens still require a click.
    // (Pages get no clipboard read access: WebKit's default.)
    wc.preferences.javaScriptCanOpenWindowsAutomatically = true
    wc.mediaTypesRequiringUserActionForPlayback = []
    // Video fullscreen (YouTube, native controls); off by default in WKWebView,
    // on in Safari and WebKitGTK.
    wc.preferences.isElementFullscreenEnabled = true
    // No back/forward page cache (as on Linux): web apps don't need it, and it
    // keeps whole pages (and processes) alive after sign-in redirects.
    if wc.preferences.responds(to: NSSelectorFromString("_setUsesPageCache:")) {
      wc.preferences.setValue(false, forKey: "usesPageCache")
    }
    // "Inspect Element" in the page menu, as Linux enables developer extras.
    if wc.preferences.responds(to: NSSelectorFromString("_setDeveloperExtrasEnabled:")) {
      wc.preferences.setValue(true, forKey: "developerExtrasEnabled")
    }
    wc.userContentController.addScriptMessageHandler(self, contentWorld: .page, name: "webtabs")
    let web = WKWebView(frame: .zero, configuration: wc)
    let tv = TabView(id: nextID, tab: tab, web: web)
    nextID += 1
    installNotificationShim(tv)
    web.customUserAgent = Core.userAgent(config, tab)
    web.navigationDelegate = self
    web.uiDelegate = self
    web.isInspectable = true
    web.allowsBackForwardNavigationGestures = true
    web.allowsMagnification = true
    web.isHidden = true
    if let cached = Icons.cached(tab.url) {
      // Cached icons are reused; they're refetched only when the URL changes.
      tv.iconFetched = true
      tv.setIcon(cached)
    }
    tv.titleObservation = web.observe(\.title) { [weak self, weak tv] _, _ in
      if let self, let tv { self.titleChanged(tv) }
    }
    tv.button.onClick = { [weak self, weak tv] in
      if let self, let tv { self.select(tv) }
    }
    tv.button.onMenu = { [weak self, weak tv] in
      guard let self, let tv else { return nil }
      return self.tabMenu(tv)
    }
    tv.button.onMiddleClick = { [weak tv] in tv?.web.reload() }

    web.translatesAutoresizingMaskIntoConstraints = false
    content.addSubview(web)
    NSLayoutConstraint.activate([
      web.leadingAnchor.constraint(equalTo: content.leadingAnchor),
      web.trailingAnchor.constraint(equalTo: content.trailingAnchor),
      web.topAnchor.constraint(equalTo: content.topAnchor),
      web.bottomAnchor.constraint(equalTo: content.bottomAnchor),
    ])
    tabStack.addArrangedSubview(tv.button)
    return tv
  }

  private func titleChanged(_ tv: TabView) {
    let badge = Core.badge(tv.web.title ?? "")
    guard badge != tv.badge else { return }
    tv.badge = badge
    tv.button.badge.text = badge.unread ? badge.label : ""
    tv.updateTooltip()
    refreshUnread()
  }

  /// Reads the page's declared icons, downloads the best one and caches it.
  func fetchIcon(_ tv: TabView) {
    let page = tv.web.url?.absoluteString ?? ""
    // Wait until we're on the tab's own site: the first load is often a login
    // redirect (accounts.google.com) whose icon isn't the app's.
    guard !tv.iconFetched, !smoke, Core.sameSite(page, tv.tab.url) else { return }
    tv.iconFetched = true
    let (home, generation) = (tv.tab.url, tv.iconGeneration)
    tv.web.evaluateJavaScript(Core.faviconScript) { [weak tv] result, _ in
      let links = result as? String ?? "[]"
      Icons.fetch(Core.favicons(page: page, links: links)) { image in
        // Gone, or its URL changed while this was downloading.
        guard let tv, let image, tv.iconGeneration == generation else { return }
        Icons.store(home, image)
        tv.setIcon(image)
      }
    }
  }

  func load(_ tv: TabView) {
    guard !tv.loaded else { return }
    tv.loaded = true
    if smoke {
      tv.web.loadHTMLString(
        "<html><head><title>(3) Smoke Test</title></head><body><h1>WebTabs native WebKit smoke test</h1><input id='editor' value='working'></body></html>",
        baseURL: URL(string: "https://webtabs.test"))
    } else if let url = URL(string: tv.tab.url) {
      tv.web.load(URLRequest(url: url))
    }
  }

  // ---- selection & ordering -------------------------------------------------------

  func select(_ tv: TabView) {
    currentID = tv.id
    for t in tabs {
      t.button.isActive = t === tv
      t.web.isHidden = t !== tv
    }
    emptyState.isHidden = true
    load(tv)
    tv.button.scrollToVisible(tv.button.bounds)
    DispatchQueue.main.async { [weak self, weak tv] in
      if let self, let tv, self.current === tv { self.window.makeFirstResponder(tv.web) }
    }
  }

  func select(index: Int) {
    if tabs.indices.contains(index) { select(tabs[index]) }
  }

  var currentIndex: Int? { tabs.firstIndex { $0.id == currentID } }

  /// Makes the bar's button order match `tabs`.
  private func syncBar() {
    for (i, tv) in tabs.enumerated() { tabStack.insertArrangedSubview(tv.button, at: i) }
  }

  func moveTab(_ tv: TabView, _ delta: Int) {
    guard let i = tabs.firstIndex(where: { $0 === tv }), tabs.indices.contains(i + delta) else {
      return
    }
    tabs.swapAt(i, i + delta)
    syncBar()
    save()
    refreshSettings()
  }

  func setTabNotifications(_ tv: TabView, _ on: Bool) {
    tv.tab.notifications = on
    save()
  }

  /// Show the first tab immediately, then wake the rest up one by one so the
  /// window is usable before every web process has started.
  func loadTabsProgressively() {
    if let tv = current { load(tv) }
    loadTimer?.invalidate()
    loadTimer = Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { [weak self] timer in
      guard let self, let next = self.tabs.first(where: { !$0.loaded }) else {
        return timer.invalidate()
      }
      self.load(next)
    }
  }

  func removeTab(_ tv: TabView) {
    let index = tabs.firstIndex { $0 === tv }
    detach(tv)
    tabs.removeAll { $0 === tv }
    if currentID == tv.id {
      currentID = 0
      if let index, !tabs.isEmpty { select(tabs[min(index, tabs.count - 1)]) }
    }
    updateEmptyState()
    refreshUnread()
  }

  /// Drops a tab's views and web process.
  private func detach(_ tv: TabView) {
    tv.titleObservation = nil
    tv.web.navigationDelegate = nil
    tv.web.uiDelegate = nil
    tv.web.stopLoading()
    tv.web.configuration.userContentController.removeAllScriptMessageHandlers()
    tv.web.configuration.userContentController.removeAllUserScripts()
    // Its popups run in its web process, which goes next.
    for popup in popups.values where popup.owner === tv { popup.window.close() }
    // Releasing a WKWebView leaves its web process running (one kept spinning at
    // ~50% CPU long after its tab was removed), so end it here. Forcefully: a
    // graceful close is what got stuck. Plain `_killWebContentProcess` doesn't
    // end it on current WebKit.
    for name in ["_killWebContentProcessAndResetState", "_close"] {
      let selector = NSSelectorFromString(name)
      if tv.web.responds(to: selector) {
        tv.web.perform(selector)
        break
      }
    }
    tv.web.removeFromSuperview()
    tabStack.removeArrangedSubview(tv.button)
    tv.button.removeFromSuperview()
    UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [
      tv.notificationKey
    ])
  }

  private func applyTab(_ tv: TabView, _ new: Tab) {
    let old = tv.tab
    tv.tab = new
    tv.updateTooltip()
    tv.updateTag()
    if old.url != new.url {
      tv.iconFetched = false
      tv.iconGeneration += 1
      installNotificationShim(tv)
    }
    let ua = Core.userAgent(config, new)
    if tv.web.customUserAgent != ua {
      tv.web.customUserAgent = ua
      if tv.loaded && old.url == new.url { tv.web.reload() }
    }
    if old.url != new.url && tv.loaded, let url = URL(string: new.url) {
      tv.web.load(URLRequest(url: url))
    }
  }

  func tabMenu(_ tv: TabView) -> NSMenu {
    let menu = NSMenu()
    menu.autoenablesItems = false
    let i = tabs.firstIndex { $0 === tv } ?? 0
    for item in [
      ActionItem("Reload") { [weak tv] in tv?.web.reload() },
      ActionItem("Go to Home Page") { [weak self, weak tv] in
        if let self, let tv { self.goHome(tv) }
      },
      ActionItem("Move Left", enabled: i > 0) { [weak self, weak tv] in
        if let self, let tv { self.moveTab(tv, -1) }
      },
      ActionItem("Move Right", enabled: i + 1 < tabs.count) { [weak self, weak tv] in
        if let self, let tv { self.moveTab(tv, 1) }
      },
      ActionItem("Edit…") { [weak self, weak tv] in
        if let self, let tv { self.editTabDialog(tv) }
      },
      ActionItem("Remove…") { [weak self, weak tv] in
        if let self, let tv { self.confirmRemove(tv) }
      },
      .separator(),
      ActionItem("Add Tab…") { [weak self] in self?.addTabDialog() },
      ActionItem("Settings…") { [weak self] in self?.openSettings() },
    ] {
      menu.addItem(item)
    }
    return menu
  }

  private func goHome(_ tv: TabView) {
    if let url = URL(string: tv.tab.url) { tv.web.load(URLRequest(url: url)) }
  }

  @objc func addTab() { addTabDialog() }

  func addTabDialog(parent: NSWindow? = nil) {
    if parent == nil { showWindow() }
    TabDialog.present(on: parent ?? window, editing: nil, existing: tabs.map(\.tab)) {
      [weak self] tab in
      guard let self else { return }
      let tv = self.createTab(tab)
      self.tabs.append(tv)
      self.select(tv)
      self.updateEmptyState()
      self.save()
      self.refreshSettings()
    }
  }

  func editTabDialog(_ tv: TabView, parent: NSWindow? = nil) {
    if parent == nil { showWindow() }
    let current = tv.tab
    TabDialog.present(on: parent ?? window, editing: current, existing: tabs.map(\.tab)) {
      [weak self, weak tv] tab in
      // A tabs.toml reload may have removed the tab meanwhile: nothing left to edit.
      guard let self, let tv, self.tabs.contains(where: { $0 === tv }) else { return }
      if tab.session != current.session {
        // A web view can't change profiles; give the tab a fresh one.
        self.replaceTab(tv, tab)
        if let key = current.session { self.dropSessionIfUnused(key) }
      } else {
        self.applyTab(tv, tab)
      }
      self.save()
      self.refreshSettings()
    }
  }

  /// Swaps a tab for a freshly built one in the same position.
  private func replaceTab(_ old: TabView, _ tab: Tab) {
    guard let i = tabs.firstIndex(where: { $0 === old }) else { return }
    let wasCurrent = currentID == old.id
    let fresh = createTab(tab)
    tabs[i] = fresh
    detach(old)
    syncBar()
    if wasCurrent { select(fresh) } else { load(fresh) }
    refreshUnread()
  }

  func confirmRemove(_ tv: TabView, parent: NSWindow? = nil) {
    if parent == nil { showWindow() }
    let session = tv.tab.session
    let alert = NSAlert()
    alert.messageText = "Remove “\(tv.tab.name)”?"
    if session != nil { alert.informativeText = "Its isolated login data is deleted too." }
    alert.addButton(withTitle: "Remove").hasDestructiveAction = true
    alert.addButton(withTitle: "Cancel")
    alert.beginSheetModal(for: parent ?? window) { [weak self, weak tv] response in
      guard response == .alertFirstButtonReturn, let self, let tv,
        self.tabs.contains(where: { $0 === tv })
      else { return }
      self.removeTab(tv)
      if let key = session { self.dropSessionIfUnused(key) }
      self.save()
      self.refreshSettings()
    }
  }

  // ---- unread & notifications --------------------------------------------------

  func refreshUnread() {
    let total = tabs.reduce(0) { $0 + $1.badge.count }
    let any = tabs.contains { $0.badge.unread }
    window.title = total > 0 && config.unreadInTitle ? "WebTabs (\(total))" : "WebTabs"
    NSApp.dockTile.badgeLabel =
      !config.unreadInDock ? nil : total > 0 ? String(total) : any ? "•" : nil
    statusDot.isHidden = !any
    status?.button?.setAccessibilityLabel(any ? "WebTabs, unread messages" : "WebTabs")
    // Linux sets the urgency hint; here the Dock icon bounces once.
    if total > lastUnread && !NSApp.isActive { NSApp.requestUserAttention(.informationalRequest) }
    lastUnread = total
  }

  /// Asks macOS once for what the options need. The Dock count is a badge
  /// permission: macOS hides it unless WebTabs may badge its icon.
  func requestNotifications() {
    var options: UNAuthorizationOptions = []
    if config.notifications { options.formUnion([.alert, .sound]) }
    if config.unreadInDock { options.insert(.badge) }
    guard !options.isEmpty, !smoke else { return }
    UNUserNotificationCenter.current().requestAuthorization(options: options) { _, error in
      if let error { NSLog("webtabs: notifications: %@", error.localizedDescription) }
      DispatchQueue.main.async { self.settingsWindow?.optionsChanged() }
    }
  }

  /// Whether macOS currently lets WebTabs show alerts and badge its Dock icon.
  func notificationPermissions(_ done: @escaping (_ alerts: Bool, _ badges: Bool) -> Void) {
    UNUserNotificationCenter.current().getNotificationSettings { s in
      let denied = s.authorizationStatus == .denied
      let (alerts, badges) = (!denied && s.alertSetting != .disabled, !denied && s.badgeSetting != .disabled)
      DispatchQueue.main.async { done(alerts, badges) }
    }
  }

  /// System Settings › Notifications › WebTabs.
  func openNotificationSettings() {
    let id = Bundle.main.bundleIdentifier ?? ""
    if let url = URL(
      string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=\(id)")
    {
      NSWorkspace.shared.open(url)
    }
  }

  /// Posts a desktop notification; `id` replaces an earlier one with the same id.
  func post(_ title: String, subtitle: String = "", body: String, info: [String: Any], id: String?)
  {
    let c = UNMutableNotificationContent()
    c.title = String(title.prefix(256))
    c.subtitle = String(subtitle.prefix(256))
    c.body = String(body.prefix(4096))
    c.sound = .default
    c.userInfo = info
    UNUserNotificationCenter.current().add(
      UNNotificationRequest(identifier: id ?? UUID().uuidString, content: c, trigger: nil))
  }

  /// Web Notification API for the page (WKWebView has none of its own). Granted
  /// up front on the tab's own origin so chat apps don't show a permission nag.
  private func installNotificationShim(_ tv: TabView) {
    let ucc = tv.web.configuration.userContentController
    ucc.removeAllUserScripts()
    let origin = URL(string: tv.tab.url).flatMap { u in
      u.scheme.flatMap { s in u.host.map { "\(s)://\($0)" + (u.port.map { ":\($0)" } ?? "") } }
    }
    let home = (try? JSONSerialization.data(withJSONObject: [origin ?? ""]))
      .flatMap { String(data: $0, encoding: .utf8) } ?? "[\"\"]"
    ucc.addUserScript(
      WKUserScript(
        source: notificationJS.replacingOccurrences(of: "HOME_ORIGIN", with: "\(home)[0]"),
        injectionTime: .atDocumentStart, forMainFrameOnly: true))
  }

  func userContentController(
    _ userContentController: WKUserContentController, didReceive message: WKScriptMessage,
    replyHandler: @escaping (Any?, String?) -> Void
  ) {
    // Only the tab's own site (a page reached through a redirect gets nothing).
    guard message.frameInfo.isMainFrame, let web = message.webView, let tv = tab(for: web),
      let body = message.body as? [String: Any],
      Core.sameSite(web.url?.absoluteString ?? "", tv.tab.url)
    else { return replyHandler("denied", nil) }
    let id = body["id"] as? String ?? ""
    switch body["op"] as? String {
    case "permission":
      replyHandler("granted", nil)
    case "notify":
      notify(tv, title: body["title"] as? String ?? "", body: body["body"] as? String ?? "", id: id)
      replyHandler(nil, nil)
    case "close":
      // All of a tab's notifications share one id, so only withdraw when the page
      // closes the one currently shown (not an older one).
      if tv.notificationID == id {
        tv.notificationID = nil
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [
          tv.notificationKey
        ])
      }
      replyHandler(nil, nil)
    default:
      replyHandler(nil, nil)
    }
  }

  private func notify(_ tv: TabView, title: String, body: String, id: String) {
    guard config.notifications, tv.tab.notifications else { return }
    if window.isKeyWindow && NSApp.isActive && current === tv { return }  // you're looking at it
    let name = tv.tab.name
    post(
      title.isEmpty ? name : title, subtitle: title.isEmpty ? "" : name, body: body,
      info: ["tab": tv.id, "page": id], id: tv.notificationKey)
    tv.notificationID = id
  }

  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
  ) { completionHandler([.banner, .list, .sound]) }

  func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    let info = response.notification.request.content.userInfo
    DispatchQueue.main.async {
      defer { completionHandler() }
      if let path = info["download"] as? String {
        return NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
      }
      guard let id = info["tab"] as? Int, let tv = self.tabs.first(where: { $0.id == id }) else {
        return
      }
      self.showWindow()
      self.select(tv)
      // Lets the page jump to the chat/email.
      if let page = info["page"] as? String, page == tv.notificationID,
        let data = try? JSONSerialization.data(withJSONObject: [page]),
        let arg = String(data: data, encoding: .utf8)
      {
        tv.notificationID = nil
        tv.web.evaluateJavaScript("window.__webtabsClick?.(\(arg)[0])")
      }
    }
  }

  // ---- profiles ----------------------------------------------------------------------

  /// The browsing profile a tab runs in, creating isolated ones on demand.
  private func store(for session: String?) -> WKWebsiteDataStore {
    guard let key = session else { return sharedStore }
    // The key is back (re-added, or a config edit in progress): keep its data.
    pendingDeletes.removeValue(forKey: key)?.cancel()
    if let store = isolated[key] { return store }
    let store = WKWebsiteDataStore(forIdentifier: storeID(key))
    isolated[key] = store
    // A marker per session, so data left behind (app quit first) is found again.
    try? FileManager.default.createDirectory(
      atPath: sessionMarker(key), withIntermediateDirectories: true)
    return store
  }

  private func storeID(_ key: String) -> UUID { profileID(namespace + ":session:" + key) }
  private func sessionMarker(_ key: String) -> String { dataDir + "/sessions/" + key }

  private func sessionInUse(_ key: String) -> Bool { tabs.contains { $0.tab.session == key } }

  /// Deletes an isolated profile's data once no tab uses it anymore. Cookies and
  /// storage are cleared right away; the profile goes a moment later. A stale
  /// profile (app quit first) is purged at the next start.
  func dropSessionIfUnused(_ key: String) {
    guard !sessionInUse(key) else { return }
    if let store = isolated.removeValue(forKey: key) {
      store.removeData(
        ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast
      ) {}
    }
    let work = DispatchWorkItem { [weak self] in
      guard let self else { return }
      self.pendingDeletes[key] = nil
      if !self.sessionInUse(key) { self.deleteSession(key) }
    }
    pendingDeletes.updateValue(work, forKey: key)?.cancel()
    // Give the web/network processes a moment to let go of the profile.
    DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: work)
  }

  private func deleteSession(_ key: String, attempt: Int = 0) {
    WKWebsiteDataStore.remove(forIdentifier: storeID(key)) { [weak self] error in
      guard let self else { return }
      guard let error else {
        try? FileManager.default.removeItem(atPath: self.sessionMarker(key))
        return
      }
      // WebKit lets go of a profile a little after its last page closes; a popup
      // can keep it open longer. Retry a few times, then at the next start.
      guard attempt < 3 else {
        return NSLog("webtabs: cannot delete session %@: %@", key, error.localizedDescription)
      }
      DispatchQueue.main.asyncAfter(deadline: .now() + Double(2 << attempt)) {
        if self.isolated[key] == nil && !self.sessionInUse(key) {
          self.deleteSession(key, attempt: attempt + 1)
        }
      }
    }
  }

  /// Removes isolated profiles no tab refers to.
  private func purgeStaleSessions() {
    let keys = (try? FileManager.default.contentsOfDirectory(atPath: dataDir + "/sessions")) ?? []
    for key in keys where !sessionInUse(key) { deleteSession(key) }
  }

  // ---- config file sync -------------------------------------------------------------

  func save() {
    var cfg = config
    cfg.tabs = tabs.map(\.tab)
    do {
      config = try Core.call("save", ["config": Core.json(cfg)])
      lastSaved = (try? String(contentsOfFile: configPath, encoding: .utf8)) ?? ""
    } catch {
      NSLog("webtabs: cannot save %@: %@", configPath, error.localizedDescription)
      showError("Couldn’t save tabs.toml", error)
    }
  }

  private func watchConfig() {
    watchTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
      self?.reloadConfig()
    }
  }

  private func reloadConfig() {
    guard let text = try? String(contentsOfFile: configPath, encoding: .utf8),
      text != lastSaved, text != lastRejected
    else { return }
    let new: Config
    do {
      new = try Core.call("parse", ["text": text])
    } catch {
      lastRejected = text
      return post(
        "WebTabs: tabs.toml not applied", body: error.localizedDescription, info: [:],
        id: "config-error")
    }
    UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [
      "config-error"
    ])
    lastSaved = text
    reconcile(new)
  }

  /// Keep live web views (and their logins/state) for URLs that survive.
  func reconcile(_ new: Config) {
    config = new
    var old = tabs
    var next: [TabView] = []
    for tab in new.tabs {
      if let i = old.firstIndex(where: { $0.tab.url == tab.url && $0.tab.session == tab.session }) {
        let tv = old.remove(at: i)
        applyTab(tv, tab)
        next.append(tv)
      } else {
        next.append(createTab(tab))
      }
    }
    for tv in old {
      detach(tv)
      if currentID == tv.id { currentID = 0 }
    }
    tabs = next
    for key in Set(old.compactMap(\.tab.session)) { dropSessionIfUnused(key) }
    syncBar()
    updateEmptyState()
    applyOptions()
    loadTabsProgressively()
    refreshSettings()
  }

  // ---- menus & shortcuts ------------------------------------------------------------

  private func makeMenu() {
    let main = NSMenu()
    func menu(_ title: String, _ items: [NSMenuItem]) -> NSMenu {
      let top = NSMenuItem()
      let sub = NSMenu(title: title)
      items.forEach { sub.addItem($0) }
      top.submenu = sub
      main.addItem(top)
      return sub
    }
    func item(
      _ title: String, _ action: Selector, _ key: String = "",
      _ modifiers: NSEvent.ModifierFlags = .command, target: AnyObject? = nil, tag: Int = 0,
      hidden: Bool = false
    ) -> NSMenuItem {
      let i = NSMenuItem(title: title, action: action, keyEquivalent: key)
      i.keyEquivalentModifierMask = modifiers
      i.target = target
      i.tag = tag
      // Extra shortcuts for the same command, kept out of sight.
      i.isHidden = hidden
      i.allowsKeyEquivalentWhenHidden = hidden
      return i
    }
    _ = menu(
      "WebTabs",
      [
        item("About WebTabs", #selector(NSApplication.orderFrontStandardAboutPanel(_:))),
        .separator(),
        item("Settings…", #selector(openSettings), ",", target: self),
        .separator(),
        item("Hide WebTabs", #selector(NSApplication.hide(_:)), "h"),
        item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]),
        item("Show All", #selector(NSApplication.unhideAllApplications(_:))),
        .separator(),
        item("Quit WebTabs", #selector(NSApplication.terminate(_:)), "q"),
      ])
    _ = menu(
      "File",
      [
        item("New Tab…", #selector(addTab), "t", target: self),
        item("Edit Tab…", #selector(editTab), "e", target: self),
        item("Remove Tab…", #selector(removeCurrentTab), target: self),
        .separator(),
        item("Close Window", #selector(NSWindow.performClose(_:)), "w"),
      ])
    _ = menu(
      "Edit",
      [
        item("Undo", Selector(("undo:")), "z"),
        item("Redo", Selector(("redo:")), "z", [.command, .shift]),
        .separator(),
        item("Cut", #selector(NSText.cut(_:)), "x"),
        item("Copy", #selector(NSText.copy(_:)), "c"),
        item("Paste", #selector(NSText.paste(_:)), "v"),
        item("Paste and Match Style", #selector(NSTextView.pasteAsPlainText(_:)), "v", [.command, .option, .shift]),
        item("Delete", #selector(NSText.delete(_:))),
        item("Select All", #selector(NSText.selectAll(_:)), "a"),
      ])
    _ = menu(
      "View",
      [
        item("Reload", #selector(reload), "r", target: self),
        item("Reload Without Cache", #selector(hardReload), "r", [.command, .shift], target: self),
        .separator(),
        item("Back", #selector(back), "[", target: self),
        item("Forward", #selector(forward), "]", target: self),
        item("Home Page", #selector(homePage), "h", [.command, .shift], target: self),
        .separator(),
        item("Zoom In", #selector(zoomIn), "=", target: self),
        item("Zoom In", #selector(zoomIn), "+", target: self, hidden: true),
        item("Zoom Out", #selector(zoomOut), "-", target: self),
        item("Actual Size", #selector(zoomReset), "0", target: self),
        .separator(),
        item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control]),
      ])
    var tabItems = [
      item("Next Tab", #selector(nextTab), "\t", .control, target: self),
      item("Previous Tab", #selector(previousTab), "\t", [.control, .shift], target: self),
      item("Next Tab", #selector(nextTab), "]", [.command, .shift], target: self, hidden: true),
      item("Previous Tab", #selector(previousTab), "[", [.command, .shift], target: self, hidden: true),
      .separator(),
      item("Move Tab Left", #selector(moveLeft), target: self),
      item("Move Tab Right", #selector(moveRight), target: self),
      .separator(),
    ]
    for n in 1...9 {
      // ⌘9 is always the last tab, like browsers.
      tabItems.append(
        item(n == 9 ? "Last Tab" : "Tab \(n)", #selector(numberTab(_:)), String(n), target: self, tag: n))
    }
    _ = menu("Tabs", tabItems)
    NSApp.windowsMenu = menu(
      "Window",
      [
        item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
        item("Zoom", #selector(NSWindow.performZoom(_:))),
        .separator(),
        item("Show WebTabs", #selector(showWindow), target: self),
        item("Bring All to Front", #selector(NSApplication.arrangeInFront(_:))),
      ])
    NSApp.mainMenu = main
  }

  func validateMenuItem(_ item: NSMenuItem) -> Bool {
    switch item.action {
    case #selector(editTab), #selector(removeCurrentTab), #selector(reload), #selector(hardReload),
      #selector(homePage), #selector(zoomIn), #selector(zoomOut), #selector(zoomReset),
      #selector(nextTab), #selector(previousTab):
      return current != nil
    case #selector(back): return current?.web.canGoBack ?? false
    case #selector(forward): return current?.web.canGoForward ?? false
    case #selector(moveLeft): return (currentIndex ?? 0) > 0
    case #selector(moveRight): return currentIndex.map { $0 + 1 < tabs.count } ?? false
    case #selector(numberTab(_:)): return !tabs.isEmpty
    default: return true
    }
  }

  @objc func editTab() { if let tv = current { editTabDialog(tv) } }
  @objc func removeCurrentTab() { if let tv = current { confirmRemove(tv) } }
  @objc func reload() { current?.web.reload() }
  @objc func hardReload() { current?.web.reloadFromOrigin() }
  @objc func back() { current?.web.goBack() }
  @objc func forward() { current?.web.goForward() }
  @objc func homePage() { if let tv = current { goHome(tv) } }
  @objc func zoomIn() { if let w = current?.web { w.pageZoom = min(4, w.pageZoom * 1.1) } }
  @objc func zoomOut() { if let w = current?.web { w.pageZoom = max(0.3, w.pageZoom / 1.1) } }
  @objc func zoomReset() { current?.web.pageZoom = 1 }
  @objc func nextTab() {
    if !tabs.isEmpty { select(index: ((currentIndex ?? 0) + 1) % tabs.count) }
  }
  @objc func previousTab() {
    if !tabs.isEmpty { select(index: ((currentIndex ?? 0) + tabs.count - 1) % tabs.count) }
  }
  @objc func numberTab(_ sender: NSMenuItem) {
    guard !tabs.isEmpty else { return }
    select(index: sender.tag == 9 ? tabs.count - 1 : min(sender.tag - 1, tabs.count - 1))
  }
  @objc func moveLeft() { if let tv = current { moveTab(tv, -1) } }
  @objc func moveRight() { if let tv = current { moveTab(tv, 1) } }
}

/// The page's `Notification`, bridged to native notifications. `HOME_ORIGIN` is
/// the tab's own origin (permission starts granted there, like Linux pre-grants).
let notificationJS = """
  (() => {
    const post = m => window.webkit.messageHandlers.webtabs.postMessage(m);
    // A tab shows one native notification at a time, so only the newest one can
    // be clicked or closed; older ones aren't kept around.
    let latest = null;
    let permission = location.origin === HOME_ORIGIN ? 'granted' : 'default';
    class Notification extends EventTarget {
      static get permission() { return permission; }
      static async requestPermission(callback) {
        permission = await post({op: 'permission'});
        callback?.(permission);
        return permission;
      }
      constructor(title, options = {}) {
        super();
        this.title = String(title);
        this.body = String(options.body ?? '');
        this.tag = String(options.tag ?? '');
        this.onclick = this.onclose = this.onshow = this.onerror = null;
        this.id = Date.now() + '-' + Math.random().toString(36).slice(2);
        if (permission !== 'granted') return;
        latest = this;
        post({op: 'notify', id: this.id, title: this.title, body: this.body});
      }
      close() {
        if (latest !== this) return;
        latest = null;
        post({op: 'close', id: this.id});
      }
    }
    window.__webtabsClick = id => {
      const n = latest;
      if (!n || n.id !== id) return;
      latest = null;
      const e = new Event('click');
      n.dispatchEvent(e);
      n.onclick?.call(n, e);
    };
    Object.defineProperty(window, 'Notification', {value: Notification, configurable: true, writable: true});
  })();
  """

let app = NSApplication.shared
let delegate = App()
app.delegate = delegate
app.run()
