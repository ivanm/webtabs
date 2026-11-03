import AppKit
import UserNotifications
import WebKit

/// `--smoke-test`: native checks against generated pages, no real sites or
/// accounts. Run it with its own WEBTABS_CONFIG/WEBTABS_DATA (see README). With
/// WEBTABS_SNAPSHOTS=<dir> it also saves PNGs of the native UI it opens.
extension App {
  func smokeFail(_ why: String) -> Never {
    fputs("SMOKE FAIL: \(why)\n", stderr)
    exit(1)
  }

  func smokeCheck(_ tv: TabView) {
    guard !smokeDone else { return }
    smokeDone = true
    // The title change can arrive just after didFinish.
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { self.pageChecks(tv) }
  }

  /// WebKit DOM, fullscreen API, Rust badges, the Linux bar layout, tab menu,
  /// reconciliation, switching.
  private func pageChecks(_ tv: TabView) {
    let probe = "[document.getElementById('editor').value, document.fullscreenEnabled, document.title].join(':')"
    tv.web.evaluateJavaScript(probe) { value, error in
      guard error == nil, let value = value as? String, value.hasPrefix("working:") else {
        self.smokeFail("DOM")
      }
      guard value == "working:true:(3) Smoke Test" else { self.smokeFail("fullscreen API: \(value)") }
      // Page cache off, as on Linux (where this WebKit has the setting).
      let prefs = tv.web.configuration.preferences
      if prefs.responds(to: NSSelectorFromString("_usesPageCache")),
        prefs.value(forKey: "usesPageCache") as? Bool != false
      {
        self.smokeFail("page cache is on")
      }
      guard tv.badge.count == 3, tv.button.badge.text == "3", !tv.button.badge.isHidden else {
        self.smokeFail("title badge")
      }
      self.window.contentView?.layoutSubtreeIfNeeded()
      guard tv.button.frame.size == NSSize(width: 32, height: 40),
        self.tabStack.arrangedSubviews.count == self.tabs.count, tv.button.isActive
      else { self.smokeFail("tab bar layout") }
      // Tabs live in the title bar: a click there must reach the tab, not the
      // title bar, and the traffic lights sit centred beside them.
      let frameView = self.window.contentView!.superview!
      let tabCenter = tv.button.convert(NSPoint(x: 16, y: 19), to: nil)
      guard frameView.hitTest(tabCenter) === tv.button else {
        self.smokeFail("clicks on a tab go to \(String(describing: frameView.hitTest(tabCenter)))")
      }
      self.checkTrafficLights("compact bar")
      // Big icons: the tall title bar, bigger tiles, lights still centred.
      self.updateOptions { $0.iconSize = 30 }
      self.window.contentView?.layoutSubtreeIfNeeded()
      guard tv.button.frame.size == NSSize(width: 40, height: 52), self.bar.frame.height == 52
      else { self.smokeFail("icon size 30: \(tv.button.frame.size)") }
      self.checkTrafficLights("tall bar")
      self.updateOptions { $0.iconSize = 22 }
      self.updateOptions { $0.tint = "#203040" }
      guard self.bar.appearance?.name == .darkAqua, self.config.tint == "#203040" else {
        self.smokeFail("dark custom tint")
      }
      self.updateOptions { $0.tint = "sage" }
      guard self.bar.appearance == nil else { self.smokeFail("preset tint follows the system") }
      self.updateOptions { $0.tint = nil }
      self.window.contentView?.layoutSubtreeIfNeeded()
      guard tv.button.frame.size == NSSize(width: 32, height: 40) else {
        self.smokeFail("icon size back to 22")
      }
      let titles = self.tabMenu(tv).items.map(\.title)
      guard titles.first == "Reload", titles.contains("Edit…"), titles.contains("Settings…") else {
        self.smokeFail("tab menu: \(titles)")
      }
      let existing = tv.web
      self.reconcile(self.config)
      guard self.current?.web === existing else { self.smokeFail("reconcile discarded a live tab") }
      // Dropped files go to the tab you see: the first tab sits lowest in the
      // stack, under the hidden ones.
      self.select(tv)
      guard self.dropTarget().map({ $0 === existing }) ?? true else {
        self.smokeFail("files dropped on the first tab go to a hidden one")
      }
      self.nextTab()
      guard let shown = self.current?.web, shown.frame.width > 0, !shown.isHidden,
        self.emptyState.isHidden
      else { self.smokeFail("tab switch has no viewport") }
      guard self.dropTarget().map({ $0 === shown }) ?? true else {
        self.smokeFail("files dropped after switching go to a hidden tab")
      }
      self.snapshot(self.window.contentView!, "main")
      self.addTabChecks {
        self.reportNotificationSettings { self.profileCheck(existing.configuration.websiteDataStore) }
      }
    }
  }

  /// Add sheet: isolation hint, tag limit, Advanced, validation that keeps input.
  private func addTabChecks(then next: @escaping () -> Void) {
    let before = tabs.count
    addTabDialog()
    guard let d = TabDialog.open.last, d.popover?.isShown == true else {
      smokeFail("add popover did not open")
    }
    snapshot(d.content, "add-tab")
    // The smoke config has a shared Gmail tab: another google.com tab shares its login.
    d.url.stringValue = "calendar.google.com/other"
    d.updateHint()
    guard d.hintVisible else { smokeFail("shared-login hint missing") }
    d.ownLogin.state = .on
    d.updateHint()
    guard !d.hintVisible else { smokeFail("shared-login hint with Own login on") }
    d.ownLogin.state = .off
    d.tag.stringValue = "Work"
    d.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: d.tag))
    guard d.tag.stringValue == "Wo", d.summaryText.contains("Tag Wo") else {
      smokeFail("icon tag longer than 2, or not in the summary: \(d.summaryText)")
    }
    guard !d.optionsVisible else { smokeFail("Options should start folded") }
    d.flipOptions()
    d.ua.selectedSegment = d.ua.segmentCount - 1
    d.syncCustom()
    guard d.optionsVisible, d.customAgentVisible else { smokeFail("Options / custom identity") }
    d.updateHint()
    snapshot(d.content, "add-tab-options")
    d.url.stringValue = "https://"
    d.name.stringValue = "Kept"
    d.submit()
    guard d.isShowing, d.errorVisible, d.name.stringValue == "Kept" else {
      smokeFail("invalid URL closed the form or lost input")
    }
    snapshot(d.content, "add-tab-error")
    d.ua.selectedSegment = 0
    d.syncCustom()
    d.url.stringValue = "example.org"
    d.name.stringValue = ""
    d.submit()
    guard tabs.count == before + 1, let added = current, added.tab.url == "https://example.org",
      added.tab.name == "example.org", added.tab.label == "Wo", added.tab.session == nil,
      added.tab.userAgent == nil,
      (try? String(contentsOfFile: configPath, encoding: .utf8))?.contains("https://example.org")
        == true
    else { smokeFail("adding a tab") }
    // Wait for the sheet to slide away before the next one.
    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.editChecks(added, then: next) }
  }

  /// Edit sheet: turning on isolation moves the tab to its own profile in place.
  private func editChecks(_ added: TabView, then next: @escaping () -> Void) {
    let index = tabs.firstIndex { $0 === added }!
    editTabDialog(added)
    guard let d = TabDialog.open.last, d.url.stringValue == "https://example.org",
      d.tag.stringValue == "Wo"
    else { smokeFail("edit sheet contents") }
    guard d.optionsVisible else { smokeFail("editing should open Options") }
    d.ownLogin.state = .on
    d.submit()
    guard tabs[index] !== added, tabs[index].tab.session == "example-org", current === tabs[index]
    else { smokeFail("isolating a tab") }
    let isolated = tabs[index]
    DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
      self.settingsChecks(isolated, then: next)
    }
  }

  /// The traffic lights sit centred on the tab bar, whatever its height.
  private func checkTrafficLights(_ what: String) {
    guard let close = window.standardWindowButton(.closeButton) else { return }
    let mid = close.convert(NSPoint(x: close.bounds.midX, y: close.bounds.midY), to: nil).y
    let barMid = window.frame.height - TabButton.metrics.height / 2
    guard abs(mid - barMid) < 1.5 else {
      smokeFail("\(what): traffic lights at y \(mid), bar centre \(barMid)")
    }
  }

  /// Settings: one row per tab, instant options, close-to-tray needs the tray.
  private func settingsChecks(_ isolated: TabView, then next: @escaping () -> Void) {
    openSettings()
    guard let s = settingsWindow else { smokeFail("settings window") }
    guard s.table.numberOfRows == tabs.count else {
      smokeFail("settings rows \(s.table.numberOfRows)/\(tabs.count)")
    }
    // Reordering (as a drop in the list does) and back.
    let first = tabs[0]
    moveTab(first, to: tabs.count - 1)
    guard tabs.last === first, tabStack.arrangedSubviews.last === first.button else {
      smokeFail("move tab to end")
    }
    moveTab(first, to: 0)
    guard tabs.first === first else { smokeFail("move tab back") }
    // Appearance controls follow the config, and the slider drives it.
    s.select(.appearance)
    s.slider.integerValue = 26
    s.slider.sendAction(s.slider.action, to: s.slider.target)
    guard config.iconSize == 26, TabButton.metrics.tall else { smokeFail("icon size slider") }
    updateOptions { $0.iconSize = 22; $0.tint = "sky" }
    guard s.slider.integerValue == 22, s.swatches.first(where: \.isActive)?.tint == "sky" else {
      smokeFail("appearance pane out of sync")
    }
    updateOptions { $0.tray = false }
    guard status == nil, !s.closeRow.isEnabled else { smokeFail("tray off") }
    updateOptions { $0.tray = true }
    guard status != nil, s.closeRow.isEnabled else { smokeFail("tray on") }
    updateOptions { $0.unreadInDock = false }
    guard NSApp.dockTile.badgeLabel == nil else { smokeFail("Dock count still shown") }
    updateOptions { $0.unreadInDock = true }
    guard NSApp.dockTile.badgeLabel != nil else { smokeFail("Dock count missing") }
    // The badge-permission warning arrives asynchronously.
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
      for pane in SettingsWindow.Pane.allCases {
        s.select(pane)
        s.window.contentView?.layoutSubtreeIfNeeded()
        self.snapshot(s.window.contentView!, "settings-\(pane.title.lowercased())")
      }
      self.updateOptions { $0.tint = nil }
      s.window.close()
      self.removeChecks(isolated, then: next)
    }
  }

  private func removeChecks(_ isolated: TabView, then next: @escaping () -> Void) {
    // Remove through the confirmation sheet, answering "Remove".
    let pidKey = "_webProcessIdentifier"
    let pid = isolated.web.responds(to: NSSelectorFromString(pidKey))
      ? (isolated.web.value(forKey: pidKey) as? Int32 ?? 0) : 0
    confirmRemove(isolated)
    guard let alert = window.attachedSheet else { smokeFail("remove confirmation") }
    window.endSheet(alert, returnCode: .alertFirstButtonReturn)
    guard !tabs.contains(where: { $0 === isolated }) else { smokeFail("remove") }
    weak let removed = isolated.web
    DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
      // A removed tab must free its web view (and with it the profile).
      guard removed == nil else { self.smokeFail("removed tab's web view leaked") }
      // ...and its web process must be gone, not left running.
      guard pid > 0, kill(pid, 0) != 0 else { self.smokeFail("removed tab's web process \(pid) still runs") }
      self.emptyChecks(then: next)
    }
  }

  /// No tabs: the centered empty state; then everything comes back.
  private func emptyChecks(then next: @escaping () -> Void) {
    let full = config
    var none = config
    none.tabs = []
    reconcile(none)
    guard !emptyState.isHidden, current == nil else { smokeFail("empty state") }
    snapshot(window.contentView!, "empty")
    reconcile(full)
    guard tabs.count == full.tabs.count, current != nil else { smokeFail("restoring tabs") }
    // The removed isolated profile is deleted a moment after its last tab goes.
    DispatchQueue.main.asyncAfter(deadline: .now() + 8) {
      guard !FileManager.default.fileExists(atPath: self.dataDir + "/sessions/example-org") else {
        self.smokeFail("isolated session not deleted")
      }
      next()
    }
  }

  /// What macOS lets this build do; a Dock count needs the badge permission.
  private func reportNotificationSettings(then next: @escaping () -> Void) {
    UNUserNotificationCenter.current().getNotificationSettings { s in
      let status = ["not asked", "denied", "allowed", "provisional", "ephemeral"]
      let setting = ["n/a", "off", "on"]
      print(
        "SMOKE INFO: notifications \(status[safe: s.authorizationStatus.rawValue]), "
          + "alerts \(setting[safe: s.alertSetting.rawValue]), "
          + "badges \(setting[safe: s.badgeSetting.rawValue])")
      DispatchQueue.main.async(execute: next)
    }
  }

  /// Logins survive restarts: a cookie set by one run is read by the next.
  private func profileCheck(_ store: WKWebsiteDataStore) {
    let cookies = store.httpCookieStore
    cookies.getAllCookies { stored in
      if CommandLine.arguments.contains("--verify-profile"),
        !stored.contains(where: { $0.name == "webtabs-smoke" && $0.value == "persistent" })
      {
        self.smokeFail("cookie did not survive app restart")
      }
      let cookie = HTTPCookie(properties: [
        .domain: "webtabs.test", .path: "/", .name: "webtabs-smoke", .value: "persistent",
        .expires: Date(timeIntervalSinceNow: 86400),
      ])!
      cookies.setCookie(cookie) {
        print(
          "SMOKE PASS: WebKit DOM, Rust badges, tab bar, tab menu, reconciliation, switching, "
            + "add/edit sheets, settings, remove, empty state, isolated cleanup, persistent cookie")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { NSApp.terminate(nil) }
      }
    }
  }

  /// The web view AppKit would hand a file dropped mid-page to (nil when this
  /// AppKit lacks the lookup).
  func dropTarget() -> WKWebView? {
    let sel = NSSelectorFromString("_hitTest:dragTypes:")
    guard let frame = window.contentView?.superview, frame.responds(to: sel) else { return nil }
    typealias HitTest = @convention(c) (AnyObject, Selector, UnsafeMutablePointer<CGPoint>, NSSet)
      -> AnyObject?
    let hit = unsafeBitCast(frame.method(for: sel), to: HitTest.self)
    var point = content.convert(NSPoint(x: content.bounds.midX, y: content.bounds.midY), to: nil)
    let types = NSSet(array: [NSPasteboard.PasteboardType.fileURL.rawValue])
    var view = hit(frame, sel, &point, types) as? NSView
    while let v = view, !(v is WKWebView) { view = v.superview }
    return view as? WKWebView
  }

  /// WEBTABS_SCREENSHOT=<file.png>: saves the 1280x800 window as it looks (title
  /// bar, tab bar, current page) for the docs, then quits. AppKit draws the
  /// chrome and WebKit snapshots the page, so no screen-recording permission is
  /// needed. WEBTABS_SCREENSHOT_TAB picks the tab (1-based, default 1).
  func screenshot(to path: String) {
    // A fixed size for the docs, without touching the size saved for normal use.
    window.setFrameAutosaveName("")
    let top = window.frame.maxY
    window.setFrame(
      NSRect(x: window.frame.minX, y: top - 800, width: 1280, height: 800), display: true)
    let number = Int(ProcessInfo.processInfo.environment["WEBTABS_SCREENSHOT_TAB"] ?? "") ?? 1
    guard tabs.indices.contains(number - 1) else { smokeFail("no tab \(number)") }
    let tv = tabs[number - 1]
    select(tv)
    // Time for every tab to load (badges, icons) and the page to settle.
    DispatchQueue.main.asyncAfter(deadline: .now() + 6) {
      // An active window, so the title bar buttons are drawn in colour.
      self.showWindow()
      guard let frame = self.window.contentView?.superview,
        let chrome = frame.bitmapImageRepForCachingDisplay(in: frame.bounds)
      else { self.smokeFail("screenshot") }
      frame.cacheDisplay(in: frame.bounds, to: chrome)
      tv.web.takeSnapshot(with: nil) { page, _ in
        guard let page,
          let out = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: chrome.pixelsWide, pixelsHigh: chrome.pixelsHigh,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
        else { self.smokeFail("page snapshot") }
        let bounds = frame.bounds
        out.size = bounds.size
        var pageRect = tv.web.convert(tv.web.bounds, to: frame)
        if frame.isFlipped { pageRect.origin.y = bounds.height - pageRect.maxY }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: out)
        // Window corners as macOS draws them; outside stays transparent.
        NSBezierPath(roundedRect: bounds, xRadius: 12, yRadius: 12).addClip()
        frame.effectiveAppearance.performAsCurrentDrawingAppearance {
          NSColor.windowBackgroundColor.setFill()
          bounds.fill()
        }
        chrome.draw(
          in: bounds, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: false,
          hints: nil)
        page.draw(in: pageRect)
        // macOS won't hand focus to a background launch, so the title bar was
        // drawn inactive: paint the buttons in their active colours.
        let lights: [(NSWindow.ButtonType, UInt32)] = [
          (.closeButton, 0xff5f57), (.miniaturizeButton, 0xfebc2e), (.zoomButton, 0x28c840),
        ]
        for (type, rgb) in lights {
          guard let button = self.window.standardWindowButton(type) else { continue }
          var r = button.convert(button.bounds, to: frame)
          if frame.isFlipped { r.origin.y = bounds.height - r.maxY }
          let d = min(r.width, r.height, 12)
          NSColor(
            srgbRed: CGFloat(rgb >> 16) / 255, green: CGFloat(rgb >> 8 & 0xff) / 255,
            blue: CGFloat(rgb & 0xff) / 255, alpha: 1
          ).setFill()
          NSBezierPath(ovalIn: NSRect(x: r.midX - d / 2, y: r.midY - d / 2, width: d, height: d))
            .fill()
        }
        NSGraphicsContext.restoreGraphicsState()
        do {
          try out.representation(using: .png, properties: [:])?.write(
            to: URL(fileURLWithPath: path))
          print("SCREENSHOT: \(path) (window active: \(self.window.isKeyWindow && NSApp.isActive))")
        } catch {
          self.smokeFail("screenshot: \(error.localizedDescription)")
        }
        NSApp.terminate(nil)
      }
    }
  }

  func snapshot(_ view: NSView, _ name: String) {
    guard let dir = ProcessInfo.processInfo.environment["WEBTABS_SNAPSHOTS"] else { return }
    view.layoutSubtreeIfNeeded()
    guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
    view.cacheDisplay(in: view.bounds, to: rep)
    // Paint the window background under it, or dark-mode text is white on nothing.
    guard
      let out = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: rep.pixelsWide, pixelsHigh: rep.pixelsHigh,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
    else { return }
    out.size = rep.size
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: out)
    let dark = view.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    NSColor(white: dark ? 0.17 : 0.93, alpha: 1).setFill()
    NSRect(origin: .zero, size: rep.size).fill()
    rep.draw(
      in: NSRect(origin: .zero, size: rep.size), from: .zero, operation: .sourceOver, fraction: 1,
      respectFlipped: false, hints: nil)
    NSGraphicsContext.restoreGraphicsState()
    try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    try? out.representation(using: .png, properties: [:])?.write(
      to: URL(fileURLWithPath: dir).appendingPathComponent("\(name).png"))
  }
}

extension Array where Element == String {
  subscript(safe i: Int) -> String { indices.contains(i) ? self[i] : "\(i)" }
}
