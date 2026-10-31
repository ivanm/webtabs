import AppKit
import CryptoKit
import ServiceManagement
import UserNotifications
import WebKit

@_silgen_name("webtabs_request") func coreRequest(_ request: UnsafePointer<CChar>)
  -> UnsafeMutablePointer<CChar>
@_silgen_name("webtabs_free") func coreFree(_ response: UnsafeMutablePointer<CChar>)

func core(_ request: [String: Any]) throws -> Any {
  let data = try JSONSerialization.data(withJSONObject: request)
  let pointer = String(decoding: data, as: UTF8.self).withCString { coreRequest($0) }
  defer { coreFree(pointer) }
  let result =
    try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as! [String: Any]
  guard result["ok"] as? Bool == true else {
    throw NSError(
      domain: "WebTabs", code: 1,
      userInfo: [NSLocalizedDescriptionKey: result["error"] as? String ?? "Core error"])
  }
  return result["result"] as Any
}
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
final class TabState {
  var spec: [String: Any]
  let web: WKWebView
  var observation: NSKeyValueObservation?
  var badge = ""
  var count = 0
  var unread = false
  var icon: NSImage?
  init(_ spec: [String: Any], _ web: WKWebView) {
    self.spec = spec
    self.web = web
  }
  var name: String { spec["name"] as? String ?? "Tab" }
  var home: String { spec["url"] as? String ?? "" }
  var identity: String { home + "\n" + (spec["session"] as? String ?? "") }
}

final class App: NSObject, NSApplicationDelegate, NSWindowDelegate, WKNavigationDelegate,
  WKUIDelegate, WKScriptMessageHandlerWithReply, UNUserNotificationCenterDelegate,
  WKDownloadDelegate
{
  var window: NSWindow!
  var bar = NSStackView()
  var content = NSView()
  var config: [String: Any] = [:]
  var tabs: [TabState] = []
  var selected = 0
  var status: NSStatusItem?
  var timer: Timer?
  var configPath = ""
  var configText = ""
  var popups: [NSWindow] = []
  var popupHomes: [ObjectIdentifier: String] = [:]
  var smoke = CommandLine.arguments.contains("--smoke-test")
  var smokeComplete = false
  var downloads: [WKDownload] = []
  func enabled(_ key: String) -> Bool { config[key] as? Bool ?? true }
  var current: TabState? { tabs.indices.contains(selected) ? tabs[selected] : nil }

  func applicationDidFinishLaunching(_ notification: Notification) {
    NSApp.setActivationPolicy(.regular)
    makeMenu()
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 1120, height: 780),
      styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
    )
    window.title = "WebTabs"
    window.minSize = NSSize(width: 540, height: 400)
    window.delegate = self
    window.isReleasedWhenClosed = false
    window.setFrameAutosaveName("WebTabsMain")
    window.center()
    let root = window.contentView!
    let strip = NSScrollView()
    strip.hasHorizontalScroller = true
    strip.drawsBackground = false
    bar.orientation = .horizontal
    bar.spacing = 6
    bar.edgeInsets = NSEdgeInsets(top: 6, left: 8, bottom: 6, right: 8)
    strip.documentView = bar
    for view in [strip, content] {
      view.translatesAutoresizingMaskIntoConstraints = false
      root.addSubview(view)
    }
    bar.translatesAutoresizingMaskIntoConstraints = false
    NSLayoutConstraint.activate([
      strip.topAnchor.constraint(equalTo: root.topAnchor),
      strip.leadingAnchor.constraint(equalTo: root.leadingAnchor),
      strip.trailingAnchor.constraint(equalTo: root.trailingAnchor),
      strip.heightAnchor.constraint(equalToConstant: 48),
      bar.leadingAnchor.constraint(equalTo: strip.contentView.leadingAnchor),
      bar.topAnchor.constraint(equalTo: strip.contentView.topAnchor),
      bar.heightAnchor.constraint(equalToConstant: 38),
      content.topAnchor.constraint(equalTo: strip.bottomAnchor),
      content.leadingAnchor.constraint(equalTo: root.leadingAnchor),
      content.trailingAnchor.constraint(equalTo: root.trailingAnchor),
      content.bottomAnchor.constraint(equalTo: root.bottomAnchor),
    ])
    UNUserNotificationCenter.current().delegate = self
    do { try load() } catch { showError(error) }
    if !CommandLine.arguments.contains("--hidden") || !enabled("tray") { showWindow() }
    timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
      guard let self, let text = try? String(contentsOfFile: self.configPath, encoding: .utf8),
        text != self.configText
      else { return }
      self.configText = text
      do { try self.load() } catch { self.showError(error) }
    }
    if smoke {
      DispatchQueue.main.asyncAfter(deadline: .now() + 20) {
        if !self.smokeComplete {
          fputs("SMOKE FAIL: WebKit timed out\n", stderr)
          exit(1)
        }
      }
    }
  }
  func load() throws {
    let value = try core(["op": "load"]) as! [String: Any]
    configPath = value["path"] as! String
    configText = (try? String(contentsOfFile: configPath, encoding: .utf8)) ?? ""
    apply(value["config"] as! [String: Any])
  }
  func save(_ candidate: [String: Any]) {
    do {
      let saved = try core(["op": "save", "config": candidate]) as! [String: Any]
      configText = (try? String(contentsOfFile: configPath, encoding: .utf8)) ?? ""
      apply(saved)
    } catch { showError(error) }
  }
  func apply(_ cfg: [String: Any]) {
    let oldCurrent = current
    config = cfg
    var old = tabs
    tabs = (cfg["tab"] as? [[String: Any]] ?? []).map { spec in
      let id = (spec["url"] as? String ?? "") + "\n" + (spec["session"] as? String ?? "")
      let tab: TabState
      if let index = old.firstIndex(where: { $0.identity == id }) {
        tab = old.remove(at: index)
        tab.spec = spec
      } else {
        let wc = WKWebViewConfiguration()
        let key =
          (ProcessInfo.processInfo.environment["WEBTABS_DATA"] ?? "default") + ":"
          + (spec["session"] as? String ?? "shared")
        wc.websiteDataStore = WKWebsiteDataStore(forIdentifier: profileID(key))
        wc.userContentController.addScriptMessageHandler(self, contentWorld: .page, name: "webtabs")
        wc.userContentController.addUserScript(
          WKUserScript(
            source: notificationJS, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let web = WKWebView(frame: .zero, configuration: wc)
        web.navigationDelegate = self
        web.uiDelegate = self
        web.isInspectable = ProcessInfo.processInfo.environment["WEBTABS_DEBUG"] != nil
        web.allowsBackForwardNavigationGestures = true
        tab = TabState(spec, web)
        tab.observation = web.observe(\.title, options: [.new]) { [weak self, weak tab] _, _ in
          guard let self, let tab else { return }
          self.updateBadge(tab)
        }
      }
      let ua = try? core(["op": "user_agent", "config": cfg, "tab": spec]) as? String
      let changedUA = tab.web.customUserAgent != ua
      tab.web.customUserAgent = ua
      if tab.web.url == nil {
        if smoke {
          tab.web.loadHTMLString(
            "<html><head><title>(3) Smoke Test</title></head><body><h1>WebTabs native WebKit smoke test</h1><input id='editor' value='working'></body></html>",
            baseURL: URL(string: "https://webtabs.test"))
        } else if let url = URL(string: tab.home) {
          tab.web.load(URLRequest(url: url))
        }
      } else if changedUA {
        tab.web.reload()
      }
      return tab
    }
    for tab in old {
      if let session = tab.spec["session"] as? String,
        !tabs.contains(where: { $0.spec["session"] as? String == session })
      {
        tab.web.configuration.websiteDataStore.removeData(
          ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast
        ) {}
      }
      tab.web.stopLoading()
      tab.web.configuration.userContentController.removeScriptMessageHandler(
        forName: "webtabs", contentWorld: .page)
      tab.observation = nil
      tab.web.removeFromSuperview()
    }
    selected =
      oldCurrent.flatMap { current in tabs.firstIndex(where: { $0 === current }) }
      ?? min(selected, max(0, tabs.count - 1))
    select(selected)
    configureStatus()
  }
  func updateBadge(_ tab: TabState) {
    if let b = try? core(["op": "badge", "title": tab.web.title ?? ""]) as? [String: Any] {
      tab.badge = b["label"] as? String ?? ""
      tab.count = b["count"] as? Int ?? 0
      tab.unread = b["unread"] as? Bool ?? false
    }
    rebuildBar()
  }
  func rebuildBar() {
    for view in bar.arrangedSubviews {
      bar.removeArrangedSubview(view)
      view.removeFromSuperview()
    }
    for (index, tab) in tabs.enumerated() {
      let label = tab.spec["label"] as? String ?? tab.name
      let button = NSButton(
        title: label + (tab.badge.isEmpty ? "" : "  \(tab.badge)"), target: self,
        action: #selector(tabClicked(_:)))
      button.bezelStyle = .rounded
      button.setButtonType(.pushOnPushOff)
      button.image = tab.icon
      button.imagePosition = .imageLeading
      button.tag = index
      button.state = index == selected ? .on : .off
      button.toolTip = tab.name + " — " + tab.home
      button.setAccessibilityLabel(tab.name)
      bar.addArrangedSubview(button)
    }
    bar.addArrangedSubview(NSButton(title: "+", target: self, action: #selector(addTab)))
    bar.addArrangedSubview(NSButton(title: "Settings", target: self, action: #selector(settings)))
    let count = tabs.reduce(0) { $0 + $1.count }
    let label = count > 0 ? String(count) : (tabs.contains(where: { $0.unread }) ? "•" : "")
    window.title = (enabled("unread_in_title") && !label.isEmpty ? "(\(label)) " : "") + "WebTabs"
    NSApp.dockTile.badgeLabel = label.isEmpty ? nil : label
    status?.button?.title = label.isEmpty ? "" : " \(label)"
  }
  func select(_ index: Int) {
    selected = index
    content.subviews.filter { $0 is NSTextField }.forEach { $0.removeFromSuperview() }
    for tab in tabs {
      let web = tab.web
      if web.superview !== content {
        web.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(web)
        NSLayoutConstraint.activate([
          web.leadingAnchor.constraint(equalTo: content.leadingAnchor),
          web.trailingAnchor.constraint(equalTo: content.trailingAnchor),
          web.topAnchor.constraint(equalTo: content.topAnchor),
          web.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
      }
      web.isHidden = tab !== current
    }
    content.layoutSubtreeIfNeeded()
    if let tab = current {
      window.makeFirstResponder(tab.web)
    } else {
      let label = NSTextField(labelWithString: "Add a web app with + or ⌘T")
      label.frame = NSRect(x: 30, y: 50, width: 400, height: 40)
      content.addSubview(label)
    }
    rebuildBar()
  }
  @objc func tabClicked(_ sender: NSButton) { select(sender.tag) }
  @objc func showWindow() {
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
  }
  func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool
  {
    showWindow()
    return true
  }
  func windowShouldClose(_ sender: NSWindow) -> Bool {
    if sender === window {
      if enabled("close_to_tray") && enabled("tray") {
        sender.orderOut(nil)
      } else {
        NSApp.terminate(nil)
      }
      return false
    }
    return true
  }
  func windowWillClose(_ notification: Notification) {
    guard let closed = notification.object as? NSWindow, closed !== window else { return }
    if let web = closed.contentView as? WKWebView {
      popupHomes.removeValue(forKey: ObjectIdentifier(web))
      web.stopLoading()
      web.navigationDelegate = nil
      web.uiDelegate = nil
    }
    popups.removeAll { $0 === closed }
  }
  func configureStatus() {
    if enabled("tray") {
      if status == nil {
        status = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        status?.button?.image = NSImage(
          systemSymbolName: "rectangle.on.rectangle", accessibilityDescription: "WebTabs")
        let menu = NSMenu()
        menu.addItem(item("Show WebTabs", #selector(showWindow), ""))
        menu.addItem(item("Settings…", #selector(settings), ""))
        menu.addItem(.separator())
        menu.addItem(item("Quit WebTabs", #selector(quit), "q"))
        status?.menu = menu
      }
    } else if let status {
      NSStatusBar.system.removeStatusItem(status)
      self.status = nil
    }
    rebuildBar()
  }
  func item(
    _ title: String, _ action: Selector, _ key: String,
    _ modifiers: NSEvent.ModifierFlags = .command
  ) -> NSMenuItem {
    let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
    item.target = self
    item.keyEquivalentModifierMask = modifiers
    return item
  }
  func makeMenu() {
    let main = NSMenu()
    func menu(_ title: String, _ items: [NSMenuItem]) {
      let top = NSMenuItem()
      let sub = NSMenu(title: title)
      items.forEach { sub.addItem($0) }
      top.submenu = sub
      main.addItem(top)
    }
    menu(
      "WebTabs",
      [
        item("About WebTabs", #selector(about), ""), item("Settings…", #selector(settings), ","),
        .separator(), item("Hide WebTabs", #selector(hideApp), "h"),
        item("Quit WebTabs", #selector(quit), "q"),
      ])
    menu(
      "File",
      [
        item("New Tab…", #selector(addTab), "t"), item("Edit Tab…", #selector(editTab), "e"),
        item("Remove Tab…", #selector(removeTab), "w", [.command, .shift]), .separator(),
        item("Close Window", #selector(closeWindow), "w"),
      ])
    let edit = NSMenuItem()
    let em = NSMenu(title: "Edit")
    for (title, action, key) in [
      ("Undo", "undo:", "z"), ("Redo", "redo:", "Z"), ("Cut", "cut:", "x"), ("Copy", "copy:", "c"),
      ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a"),
    ] { em.addItem(NSMenuItem(title: title, action: Selector(action), keyEquivalent: key)) }
    edit.submenu = em
    main.addItem(edit)
    menu(
      "View",
      [
        item("Reload", #selector(reload), "r"),
        item("Reload Without Cache", #selector(hardReload), "r", [.command, .shift]),
        item("Back", #selector(back), "["), item("Forward", #selector(forward), "]"),
        item("Home", #selector(home), "h", [.command, .shift]), .separator(),
        item("Zoom In", #selector(zoomIn), "="), item("Zoom Out", #selector(zoomOut), "-"),
        item("Actual Size", #selector(zoomReset), "0"),
      ])
    var tabItems = [
      item("Next Tab", #selector(nextTab), "\t", .control),
      item("Previous Tab", #selector(previousTab), "\t", [.control, .shift]),
      item("Move Tab Left", #selector(moveLeft), "[", [.command, .shift]),
      item("Move Tab Right", #selector(moveRight), "]", [.command, .shift]),
    ]
    for n in 1...9 {
      let i = item("Tab \(n == 9 ? "Last" : String(n))", #selector(numberTab(_:)), String(n))
      i.tag = n
      tabItems.append(i)
    }
    menu("Tabs", tabItems)
    menu(
      "Window",
      [item("Show WebTabs", #selector(showWindow), ""), item("Minimize", #selector(minimize), "m")])
    NSApp.mainMenu = main
  }
  @objc func about() { NSApp.orderFrontStandardAboutPanel(nil) }
  @objc func hideApp() { NSApp.hide(nil) }
  @objc func quit() { NSApp.terminate(nil) }
  @objc func closeWindow() { window.performClose(nil) }
  @objc func minimize() { window.miniaturize(nil) }
  @objc func reload() { current?.web.reload() }
  @objc func hardReload() { current?.web.reloadFromOrigin() }
  @objc func back() { current?.web.goBack() }
  @objc func forward() { current?.web.goForward() }
  @objc func home() {
    if let tab = current, let url = URL(string: tab.home) { tab.web.load(URLRequest(url: url)) }
  }
  @objc func zoomIn() { if let w = current?.web { w.pageZoom = min(3, w.pageZoom + 0.1) } }
  @objc func zoomOut() { if let w = current?.web { w.pageZoom = max(0.5, w.pageZoom - 0.1) } }
  @objc func zoomReset() { current?.web.pageZoom = 1 }
  @objc func nextTab() { if !tabs.isEmpty { select((selected + 1) % tabs.count) } }
  @objc func previousTab() { if !tabs.isEmpty { select((selected + tabs.count - 1) % tabs.count) } }
  @objc func numberTab(_ sender: NSMenuItem) {
    let index = sender.tag == 9 ? tabs.count - 1 : sender.tag - 1
    if tabs.indices.contains(index) { select(index) }
  }
  @objc func moveLeft() { move(-1) }
  @objc func moveRight() { move(1) }
  func move(_ delta: Int) {
    guard tabs.indices.contains(selected + delta) else { return }
    var specs = tabs.map(\.spec)
    specs.swapAt(selected, selected + delta)
    var cfg = config
    cfg["tab"] = specs
    save(cfg)
  }
  @objc func addTab() { edit(nil) }
  @objc func editTab() { if tabs.indices.contains(selected) { edit(selected) } }
  func edit(_ index: Int?) {
    let spec = index.map { tabs[$0].spec } ?? [:]
    let alert = NSAlert()
    alert.messageText = index == nil ? "Add web app" : "Edit web app"
    alert.addButton(withTitle: "Save")
    alert.addButton(withTitle: "Cancel")
    let stack = NSStackView()
    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 8
    let name = NSTextField(string: spec["name"] as? String ?? "")
    let url = NSTextField(string: spec["url"] as? String ?? "")
    let label = NSTextField(string: spec["label"] as? String ?? "")
    let agent = NSTextField(string: spec["user_agent"] as? String ?? "")
    let presets = NSPopUpButton()
    presets.addItem(withTitle: "Choose a preset…")
    let choices = (try? core(["op": "presets"]) as? [[String: String]]) ?? []
    for choice in choices { presets.addItem(withTitle: choice["name"]!) }
    stack.addArrangedSubview(presets)
    for (caption, field) in [
      ("Name", name), ("URL", url), ("Short label (optional)", label),
      ("User agent (blank uses default)", agent),
    ] {
      stack.addArrangedSubview(NSTextField(labelWithString: caption))
      field.widthAnchor.constraint(equalToConstant: 420).isActive = true
      stack.addArrangedSubview(field)
    }
    let muted = NSButton(checkboxWithTitle: "Mute notifications", target: nil, action: nil)
    muted.state = spec["notifications"] as? Bool == false ? .on : .off
    let isolated = NSButton(checkboxWithTitle: "Isolated login", target: nil, action: nil)
    isolated.state = spec["session"] != nil ? .on : .off
    stack.addArrangedSubview(muted)
    stack.addArrangedSubview(isolated)
    let handler = ClosureAction {
      let i = presets.indexOfSelectedItem - 1
      if choices.indices.contains(i) {
        name.stringValue = choices[i]["name"]!
        url.stringValue = choices[i]["url"]!
      }
    }
    presets.target = handler
    presets.action = #selector(ClosureAction.run)
    stack.frame = NSRect(x: 0, y: 0, width: 420, height: 340)
    alert.accessoryView = stack
    withExtendedLifetime(handler) {
      guard alert.runModal() == .alertFirstButtonReturn else { return }
      var new: [String: Any] = [
        "name": name.stringValue, "url": url.stringValue, "notifications": muted.state != .on,
      ]
      if !label.stringValue.isEmpty { new["label"] = label.stringValue }
      if !agent.stringValue.isEmpty { new["user_agent"] = agent.stringValue }
      if isolated.state == .on {
        new["session"] =
          spec["session"]
          ?? (try? core([
            "op": "session_key", "name": name.stringValue,
            "taken": tabs.compactMap { $0.spec["session"] as? String },
          ]))
      }
      var specs = tabs.map(\.spec)
      if let index { specs[index] = new } else { specs.append(new) }
      var cfg = config
      cfg["tab"] = specs
      save(cfg)
      if index == nil && tabs.count == specs.count { select(tabs.count - 1) }
    }
  }
  @objc func removeTab() {
    guard let tab = current else { return }
    let alert = NSAlert()
    alert.messageText = "Remove \(tab.name)?"
    alert.informativeText = "An isolated login will be cleared if no other tab uses it."
    alert.addButton(withTitle: "Remove")
    alert.addButton(withTitle: "Cancel")
    if alert.runModal() == .alertFirstButtonReturn {
      var specs = tabs.map(\.spec)
      specs.remove(at: selected)
      var cfg = config
      cfg["tab"] = specs
      save(cfg)
    }
  }
  @objc func settings() {
    let alert = NSAlert()
    alert.messageText = "WebTabs Settings"
    alert.addButton(withTitle: "Save")
    alert.addButton(withTitle: "Cancel")
    alert.addButton(withTitle: "Open Config")
    let stack = NSStackView()
    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 12
    var switches: [String: NSButton] = [:]
    for (key, title) in [
      ("notifications", "Allow desktop notifications"), ("tray", "Show menu bar icon"),
      ("close_to_tray", "Keep running when the window closes"),
      ("unread_in_title", "Show unread count in window title"),
    ] {
      let button = NSButton(checkboxWithTitle: title, target: nil, action: nil)
      button.state = enabled(key) ? .on : .off
      switches[key] = button
      stack.addArrangedSubview(button)
    }
    let login = NSButton(checkboxWithTitle: "Start at login", target: nil, action: nil)
    login.state = SMAppService.mainApp.status == .enabled ? .on : .off
    stack.addArrangedSubview(login)
    stack.addArrangedSubview(
      NSTextField(
        wrappingLabelWithString:
          "Use the Tabs menu to switch or reorder tabs, and ⌘E to edit the selected tab."))
    stack.frame = NSRect(x: 0, y: 0, width: 390, height: 220)
    alert.accessoryView = stack
    let result = alert.runModal()
    if result == .alertThirdButtonReturn {
      NSWorkspace.shared.open(URL(fileURLWithPath: configPath))
      return
    }
    guard result == .alertFirstButtonReturn else { return }
    var cfg = config
    for (key, button) in switches { cfg[key] = button.state == .on }
    save(cfg)
    do {
      if login.state == .on && SMAppService.mainApp.status != .enabled {
        try SMAppService.mainApp.register()
      } else if login.state == .off && SMAppService.mainApp.status == .enabled {
        try SMAppService.mainApp.unregister()
      }
    } catch { showError(error) }
  }
  func showError(_ error: Error) {
    let alert = NSAlert(error: error)
    alert.runModal()
  }
  func homeFor(_ web: WKWebView) -> String {
    tabs.first(where: { $0.web === web })?.home ?? popupHomes[ObjectIdentifier(web)] ?? ""
  }
  func inApp(_ web: WKWebView, _ url: URL?) -> Bool {
    (try? core(["op": "route", "home": homeFor(web), "url": url?.absoluteString ?? ""]) as? Bool)
      ?? false
  }
  func external(_ url: URL?) {
    guard let url,
      ["http", "https", "mailto", "tel", "slack", "zoommtg"].contains(
        url.scheme?.lowercased() ?? "")
    else { return }
    NSWorkspace.shared.open(url)
  }
  func webView(
    _ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
    decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
  ) {
    let popup = popupHomes[ObjectIdentifier(webView)] != nil
    if (action.navigationType == .linkActivated || popup)
      && action.targetFrame?.isMainFrame != false && !inApp(webView, action.request.url)
    {
      external(action.request.url)
      decisionHandler(.cancel)
    } else if action.shouldPerformDownload {
      decisionHandler(.download)
    } else {
      decisionHandler(.allow)
    }
  }
  func webView(
    _ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
    for action: WKNavigationAction, windowFeatures: WKWindowFeatures
  ) -> WKWebView? {
    guard inApp(webView, action.request.url) else {
      external(action.request.url)
      return nil
    }
    let web = WKWebView(frame: .zero, configuration: configuration)
    web.navigationDelegate = self
    web.uiDelegate = self
    web.customUserAgent = webView.customUserAgent
    popupHomes[ObjectIdentifier(web)] = homeFor(webView)
    let popup = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 640, height: 760),
      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    popup.title = "Sign in — WebTabs"
    popup.contentView = web
    popup.delegate = self
    popup.isReleasedWhenClosed = false
    popups.append(popup)
    popup.center()
    popup.makeKeyAndOrderFront(nil)
    return web
  }
  func webViewDidClose(_ webView: WKWebView) {
    popups.first(where: { $0.contentView === webView })?.close()
  }
  func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { webView.reload() }
  func webView(
    _ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
    initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping () -> Void
  ) {
    let a = NSAlert()
    a.messageText = frame.securityOrigin.host
    a.informativeText = message
    a.runModal()
    completionHandler()
  }
  func webView(
    _ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
    initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void
  ) {
    let a = NSAlert()
    a.messageText = frame.securityOrigin.host
    a.informativeText = message
    a.addButton(withTitle: "OK")
    a.addButton(withTitle: "Cancel")
    completionHandler(a.runModal() == .alertFirstButtonReturn)
  }
  func webView(
    _ webView: WKWebView, runJavaScriptTextInputPanelWithPrompt prompt: String,
    defaultText: String?, initiatedByFrame frame: WKFrameInfo,
    completionHandler: @escaping (String?) -> Void
  ) {
    let a = NSAlert()
    a.messageText = prompt
    let field = NSTextField(string: defaultText ?? "")
    field.frame = NSRect(x: 0, y: 0, width: 340, height: 24)
    a.accessoryView = field
    a.addButton(withTitle: "OK")
    a.addButton(withTitle: "Cancel")
    completionHandler(a.runModal() == .alertFirstButtonReturn ? field.stringValue : nil)
  }
  func webView(
    _ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters,
    initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping ([URL]?) -> Void
  ) {
    let panel = NSOpenPanel()
    panel.allowsMultipleSelection = parameters.allowsMultipleSelection
    panel.canChooseDirectories = parameters.allowsDirectories
    panel.begin { response in completionHandler(response == .OK ? panel.urls : nil) }
  }
  func webView(
    _ webView: WKWebView, decidePolicyFor response: WKNavigationResponse,
    decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void
  ) { decisionHandler(response.canShowMIMEType ? .allow : .download) }
  func webView(
    _ webView: WKWebView, navigationAction: WKNavigationAction, didBecome download: WKDownload
  ) {
    downloads.append(download)
    download.delegate = self
  }
  func webView(
    _ webView: WKWebView, navigationResponse: WKNavigationResponse, didBecome download: WKDownload
  ) {
    downloads.append(download)
    download.delegate = self
  }
  func download(
    _ download: WKDownload, decideDestinationUsing response: URLResponse, suggestedFilename: String,
    completionHandler: @escaping (URL?) -> Void
  ) {
    let panel = NSSavePanel()
    panel.nameFieldStringValue = suggestedFilename
    panel.begin { result in completionHandler(result == .OK ? panel.url : nil) }
  }
  func downloadDidFinish(_ download: WKDownload) { downloads.removeAll { $0 === download } }
  func download(_ download: WKDownload, didFailWithError error: Error, resumeData: Data?) {
    downloads.removeAll { $0 === download }
    showError(error)
  }
  func webView(
    _ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
    withError error: Error
  ) {
    guard (error as NSError).code != NSURLErrorCancelled else { return }
    NSLog("WebTabs navigation failed for %@: %@", homeFor(webView), error.localizedDescription)
    if let tab = tabs.first(where: { $0.web === webView }) {
      let message = NSTextField(
        wrappingLabelWithString:
          "Could not load \(tab.name): \(error.localizedDescription)\nPress ⌘R to retry.")
      message.frame = NSRect(x: 30, y: 60, width: 460, height: 90)
      webView.addSubview(message)
    }
  }
  func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
    webView.subviews.filter { $0 is NSTextField }.forEach { $0.removeFromSuperview() }
  }
  func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
    if !smoke, let tab = tabs.first(where: { $0.web === webView }),
      let script = try? core(["op": "favicon_script"]) as? String
    {
      webView.evaluateJavaScript(script) { [weak self, weak tab] result, _ in
        guard let self, let tab, let links = result as? String,
          let candidates = try? core([
            "op": "favicons", "url": webView.url?.absoluteString ?? tab.home, "links": links,
          ]) as? [String],
          let candidate = candidates.first, let url = URL(string: candidate),
          ["https", "http"].contains(url.scheme ?? "")
        else { return }
        URLSession.shared.dataTask(with: url) { data, _, _ in
          guard let data, data.count < 2_000_000 else { return }
          DispatchQueue.main.async {
            if let icon = NSImage(data: data) {
              icon.size = NSSize(width: 16, height: 16)
              tab.icon = icon
              self.rebuildBar()
            }
          }
        }.resume()
      }
    }
    guard smoke && !smokeComplete else { return }
    smokeComplete = true
    webView.evaluateJavaScript("document.getElementById('editor').value + ':' + document.title") {
      value, error in
      guard error == nil, value as? String == "working:(3) Smoke Test",
        self.tabs.contains(where: { $0.count == 3 })
      else {
        fputs("SMOKE FAIL: DOM or title badge\n", stderr)
        exit(1)
      }
      self.smokeComplete = true
      let existing = self.current!.web
      self.apply(self.config)
      precondition(self.current!.web === existing, "Config reconciliation discarded a live tab")
      self.nextTab()
      precondition(
        self.current!.web.frame.width > 0 && !self.current!.web.isHidden,
        "Tab switch has no viewport")
      let cookies = webView.configuration.websiteDataStore.httpCookieStore
      cookies.getAllCookies { stored in
        if CommandLine.arguments.contains("--verify-profile") {
          guard stored.contains(where: { $0.name == "webtabs-smoke" && $0.value == "persistent" })
          else {
            fputs("SMOKE FAIL: cookie did not survive app restart\n", stderr)
            exit(1)
          }
        }
        let cookie = HTTPCookie(properties: [
          .domain: "webtabs.test", .path: "/", .name: "webtabs-smoke", .value: "persistent",
          .expires: Date(timeIntervalSinceNow: 86400),
        ])!
        cookies.setCookie(cookie) {
          print(
            "SMOKE PASS: WebKit DOM, Rust badges, tab switching, live-view reconciliation, persistent cookie"
          )
          DispatchQueue.main.asyncAfter(deadline: .now() + 1) { NSApp.terminate(nil) }
        }
      }
    }
  }
  func userContentController(
    _ userContentController: WKUserContentController, didReceive message: WKScriptMessage,
    replyHandler: @escaping (Any?, String?) -> Void
  ) {
    guard message.frameInfo.isMainFrame,
      let tab = tabs.first(where: { $0.web === message.webView }), enabled("notifications"),
      tab.spec["notifications"] as? Bool != false, let body = message.body as? [String: Any]
    else {
      replyHandler("denied", nil)
      return
    }
    let center = UNUserNotificationCenter.current()
    if body["op"] as? String == "permission" {
      center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
        DispatchQueue.main.async { replyHandler(granted ? "granted" : "denied", nil) }
      }
      return
    }
    if body["op"] as? String == "notify" {
      let c = UNMutableNotificationContent()
      c.title = String((body["title"] as? String ?? tab.name).prefix(256))
      c.body = String((body["body"] as? String ?? "").prefix(4096))
      c.sound = .default
      c.userInfo = ["tab": tab.identity, "id": body["id"] as? String ?? ""]
      center.add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil))
    }
    replyHandler("ok", nil)
  }
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
  ) { completionHandler([.banner, .sound]) }
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    DispatchQueue.main.async {
      if let index = self.tabs.firstIndex(where: {
        $0.identity == response.notification.request.content.userInfo["tab"] as? String
      }) {
        self.select(index)
        self.showWindow()
        let id = response.notification.request.content.userInfo["id"] as? String ?? ""
        if let data = try? JSONSerialization.data(withJSONObject: [id]),
          let arg = String(data: data, encoding: .utf8)
        {
          self.current?.web.evaluateJavaScript("window.__webtabsClick?.(\(arg)[0])")
        }
      }
      completionHandler()
    }
  }
}
final class ClosureAction: NSObject {
  let body: () -> Void
  init(_ body: @escaping () -> Void) { self.body = body }
  @objc func run() { body() }
}
let notificationJS = """
  (() => {
    const instances = new Map();
    let permission = 'default';
    class NativeNotification extends EventTarget {
      static get permission() { return permission; }
      static async requestPermission(callback) {
        permission = await window.webkit.messageHandlers.webtabs.postMessage({op:'permission'});
        callback?.(permission); return permission;
      }
      constructor(title, options = {}) {
        super(); this.title = String(title); this.body = String(options.body || '');
        const id = crypto.randomUUID(); instances.set(id, this); this.id = id;
        if (permission === 'granted') window.webkit.messageHandlers.webtabs.postMessage({op:'notify', id, title:this.title, body:this.body});
        setTimeout(() => instances.delete(id), 86400000);
      }
      close() { instances.delete(this.id); }
    }
    window.__webtabsClick = id => { const n = instances.get(id); if(n) { const e = new Event('click'); n.dispatchEvent(e); n.onclick?.(e); } };
    Object.defineProperty(window, 'Notification', {value:NativeNotification, configurable:true});
  })();
  """
let app = NSApplication.shared
let delegate = App()
app.delegate = delegate
app.run()
