import AppKit
import UserNotifications
import WebKit

/// A small window for OAuth flows, Meet calls, "open in new window" etc.
final class Popup {
  let window: NSWindow
  let web: WKWebView
  /// The tab that (directly or through another popup) opened it.
  weak var owner: TabView?
  let home: String
  /// Opened by a click; only then may it hand its first link to the browser.
  let byUser: Bool
  var title: NSKeyValueObservation?
  var crashes: [Date] = []

  init(window: NSWindow, web: WKWebView, owner: TabView?, home: String, byUser: Bool) {
    (self.window, self.web, self.owner, self.home, self.byUser) = (window, web, owner, home, byUser)
  }
}

/// WebKit plumbing: link routing, popups, permissions, dialogs and downloads.
/// Same rules as src/linux/web.rs.
extension App: WKNavigationDelegate, WKUIDelegate, WKDownloadDelegate {
  /// Crash-reload guard: more crashes than this within a minute stops reloading.
  static let maxCrashes = 3

  /// Home URL of the tab that owns a web view or popup, read live so edits apply.
  func home(of web: WKWebView) -> String {
    if let tv = tab(for: web) { return tv.tab.url }
    let popup = popups[ObjectIdentifier(web)]
    return popup?.owner?.tab.url ?? popup?.home ?? ""
  }

  func tab(for web: WKWebView) -> TabView? { tabs.first { $0.web === web } }

  /// Hands a link to the default handler: http/https/mailto/tel only.
  func openExternal(_ url: URL?) {
    guard let url, Core.safeExternal(url.absoluteString) else {
      return NSLog("webtabs: refusing to open %@", url?.absoluteString ?? "")
    }
    NSWorkspace.shared.open(url)
  }

  /// A real click (WebKit's user-gesture flag), so a page can't push things out
  /// of the app on its own.
  func isUserGesture(_ action: WKNavigationAction) -> Bool {
    let key = "_isUserInitiated"
    if action.responds(to: NSSelectorFromString(key)), let user = action.value(forKey: key) as? Bool
    {
      return user
    }
    return action.navigationType == .linkActivated
  }

  func webView(
    _ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
    decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
  ) {
    let url = action.request.url?.absoluteString ?? ""
    let popup = popups[ObjectIdentifier(webView)]
    // A popup's first real navigation decides whether it was a link to elsewhere.
    let fresh =
      popup != nil && (webView.url == nil || ["", "about:blank"].contains(webView.url!.absoluteString))
    let external: Bool
    switch action.targetFrame {
    case nil:  // new window
      external = !Core.route(home: home(of: webView), url: url)
    case let frame? where frame.isMainFrame:
      // Only user-clicked links leave the app; redirects (SSO) stay in.
      external =
        (action.navigationType == .linkActivated || fresh)
        && !Core.route(home: home(of: webView), url: url)
    default:
      external = false
    }
    if external {
      if fresh ? popup!.byUser : isUserGesture(action) { openExternal(action.request.url) }
      decisionHandler(.cancel)
      if fresh { DispatchQueue.main.async { popup!.window.close() } }
    } else if action.shouldPerformDownload {
      decisionHandler(.download)
    } else {
      decisionHandler(.allow)
    }
  }

  func webView(
    _ webView: WKWebView, decidePolicyFor response: WKNavigationResponse,
    decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void
  ) {
    let disposition =
      (response.response as? HTTPURLResponse)?.value(forHTTPHeaderField: "Content-Disposition")
    let attachment = disposition?.lowercased().hasPrefix("attachment") ?? false
    decisionHandler(response.canShowMIMEType && !(attachment && response.isForMainFrame) ? .allow : .download)
  }

  func webView(
    _ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
    for action: WKNavigationAction, windowFeatures: WKWindowFeatures
  ) -> WKWebView? {
    let url = action.request.url?.absoluteString ?? ""
    let byUser = isUserGesture(action)
    guard Core.route(home: home(of: webView), url: url) else {
      if byUser { openExternal(action.request.url) }
      return nil
    }
    let web = WKWebView(frame: .zero, configuration: configuration)
    web.navigationDelegate = self
    web.uiDelegate = self
    web.customUserAgent = webView.customUserAgent
    web.isInspectable = true
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 1000, height: 760),
      styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: true)
    window.title = "WebTabs"
    window.contentView = web
    window.delegate = self
    window.isReleasedWhenClosed = false
    window.tabbingMode = .disallowed
    if let parent = webView.window ?? self.window {
      window.setFrameOrigin(
        NSPoint(x: parent.frame.midX - 500, y: parent.frame.midY - 380))
    }
    let owner = tab(for: webView) ?? popups[ObjectIdentifier(webView)]?.owner
    let popup = Popup(
      window: window, web: web, owner: owner, home: home(of: webView), byUser: byUser)
    popup.title = web.observe(\.title) { [weak window] web, _ in
      window?.title = web.title.flatMap { $0.isEmpty ? nil : $0 } ?? "WebTabs"
    }
    popups[ObjectIdentifier(web)] = popup
    // Shown only once real content commits (see didCommit); a popup that never
    // gets any (a blocked load) is dropped after a grace period.
    DispatchQueue.main.asyncAfter(deadline: .now() + 15) { [weak window] in
      if let window, !window.isVisible { window.close() }
    }
    return web
  }

  func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
    if let popup = popups[ObjectIdentifier(webView)], !popup.window.isVisible,
      webView.url?.scheme?.hasPrefix("http") == true
    {
      popup.window.makeKeyAndOrderFront(nil)
    }
  }

  func webViewDidClose(_ webView: WKWebView) {
    popups[ObjectIdentifier(webView)]?.window.close()
  }

  func closePopup(_ window: NSWindow) {
    guard let entry = popups.first(where: { $0.value.window === window }) else { return }
    popups[entry.key] = nil
    let popup = entry.value
    popup.title = nil
    popup.web.stopLoading()
    popup.web.navigationDelegate = nil
    popup.web.uiDelegate = nil
  }

  func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
    let recent: [Date]
    if let tv = tab(for: webView) {
      tv.crashes = tv.crashes.filter { $0.timeIntervalSinceNow > -60 } + [Date()]
      recent = tv.crashes
    } else if let popup = popups[ObjectIdentifier(webView)] {
      popup.crashes = popup.crashes.filter { $0.timeIntervalSinceNow > -60 } + [Date()]
      recent = popup.crashes
    } else {
      return
    }
    guard recent.count <= Self.maxCrashes else {
      return NSLog("webtabs: %@ keeps crashing; not reloading", webView.url?.absoluteString ?? "")
    }
    NSLog("webtabs: web process for %@ died, reloading", webView.url?.absoluteString ?? "")
    webView.reload()
  }

  // ---- permissions ----------------------------------------------------------

  /// Camera and microphone for the tab's own site only (Meet from Calendar is the
  /// same site); a page reached through a redirect gets nothing.
  func webView(
    _ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin,
    initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType,
    decisionHandler: @escaping (WKPermissionDecision) -> Void
  ) {
    let page = webView.url?.absoluteString ?? ""
    decisionHandler(Core.sameSite(page, home(of: webView)) ? .grant : .deny)
  }

  // ---- dialogs --------------------------------------------------------------

  /// Page dialogs are sheets on the page's window, after bringing its tab up.
  private func present(_ alert: NSAlert, for webView: WKWebView, _ done: @escaping (Bool) -> Void)
  {
    if let tv = tab(for: webView), tv !== current { select(tv) }
    if let window = webView.window, window.isVisible {
      alert.beginSheetModal(for: window) { done($0 == .alertFirstButtonReturn) }
    } else {
      done(alert.runModal() == .alertFirstButtonReturn)
    }
  }

  private func pageAlert(_ message: String, _ frame: WKFrameInfo) -> NSAlert {
    let alert = NSAlert()
    alert.messageText = frame.securityOrigin.host.isEmpty ? "WebTabs" : frame.securityOrigin.host
    alert.informativeText = message
    return alert
  }

  func webView(
    _ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
    initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping () -> Void
  ) {
    present(pageAlert(message, frame), for: webView) { _ in completionHandler() }
  }

  func webView(
    _ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
    initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void
  ) {
    let alert = pageAlert(message, frame)
    alert.addButton(withTitle: "OK")
    alert.addButton(withTitle: "Cancel")
    present(alert, for: webView, completionHandler)
  }

  func webView(
    _ webView: WKWebView, runJavaScriptTextInputPanelWithPrompt prompt: String,
    defaultText: String?, initiatedByFrame frame: WKFrameInfo,
    completionHandler: @escaping (String?) -> Void
  ) {
    let alert = pageAlert(prompt, frame)
    let field = NSTextField(string: defaultText ?? "")
    field.frame = NSRect(x: 0, y: 0, width: 300, height: 24)
    alert.accessoryView = field
    alert.addButton(withTitle: "OK")
    alert.addButton(withTitle: "Cancel")
    alert.window.initialFirstResponder = field
    present(alert, for: webView) { completionHandler($0 ? field.stringValue : nil) }
  }

  func webView(
    _ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters,
    initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping ([URL]?) -> Void
  ) {
    let panel = NSOpenPanel()
    panel.allowsMultipleSelection = parameters.allowsMultipleSelection
    panel.canChooseDirectories = parameters.allowsDirectories
    let done = { (r: NSApplication.ModalResponse) in completionHandler(r == .OK ? panel.urls : nil) }
    if let window = webView.window, window.isVisible {
      panel.beginSheetModal(for: window, completionHandler: done)
    } else {
      panel.begin(completionHandler: done)
    }
  }

  // ---- load errors ----------------------------------------------------------

  func webView(
    _ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
    withError error: Error
  ) {
    let e = error as NSError
    // Cancelled, or turned into a download / blocked by our own policy.
    guard e.code != NSURLErrorCancelled, !(e.domain == "WebKitErrorDomain" && e.code == 102),
      let tv = tab(for: webView)
    else { return }
    let failed = e.userInfo[NSURLErrorFailingURLErrorKey] as? URL ?? URL(string: tv.tab.url)
    showLoadError(tv, e.localizedDescription, retry: failed)
  }

  func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
    webView.subviews.filter { $0 is LoadError }.forEach { $0.removeFromSuperview() }
  }

  func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
    if let tv = tab(for: webView) {
      fetchIcon(tv)
      if smoke { smokeCheck(tv) }
    }
  }

  private func showLoadError(_ tv: TabView, _ message: String, retry: URL?) {
    tv.web.subviews.filter { $0 is LoadError }.forEach { $0.removeFromSuperview() }
    let overlay = LoadError(title: "Couldn’t load \(tv.tab.name)", detail: message) {
      [weak tv] in
      if let url = retry { tv?.web.load(URLRequest(url: url)) }
    }
    overlay.translatesAutoresizingMaskIntoConstraints = false
    tv.web.addSubview(overlay)
    NSLayoutConstraint.activate([
      overlay.leadingAnchor.constraint(equalTo: tv.web.leadingAnchor),
      overlay.trailingAnchor.constraint(equalTo: tv.web.trailingAnchor),
      overlay.topAnchor.constraint(equalTo: tv.web.topAnchor),
      overlay.bottomAnchor.constraint(equalTo: tv.web.bottomAnchor),
    ])
  }

  // ---- downloads: straight to ~/Downloads, like WebKitGTK -------------------

  func webView(
    _ webView: WKWebView, navigationAction: WKNavigationAction, didBecome download: WKDownload
  ) { download.delegate = self }

  func webView(
    _ webView: WKWebView, navigationResponse: WKNavigationResponse, didBecome download: WKDownload
  ) { download.delegate = self }

  func download(
    _ download: WKDownload, decideDestinationUsing response: URLResponse, suggestedFilename: String,
    completionHandler: @escaping (URL?) -> Void
  ) {
    let dir = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask)[0]
    let name = (suggestedFilename as NSString).lastPathComponent
    let base = (name as NSString).deletingPathExtension
    let ext = (name as NSString).pathExtension
    var dest = dir.appendingPathComponent(name.isEmpty ? "download" : name)
    var n = 2
    while FileManager.default.fileExists(atPath: dest.path) {
      dest = dir.appendingPathComponent("\(base) (\(n))" + (ext.isEmpty ? "" : ".\(ext)"))
      n += 1
    }
    downloads[ObjectIdentifier(download)] = dest
    completionHandler(dest)
  }

  func downloadDidFinish(_ download: WKDownload) {
    guard let dest = downloads.removeValue(forKey: ObjectIdentifier(download)) else { return }
    post(
      "Download complete", body: dest.lastPathComponent, info: ["download": dest.path], id: nil)
  }

  func download(_ download: WKDownload, didFailWithError error: Error, resumeData: Data?) {
    let dest = downloads.removeValue(forKey: ObjectIdentifier(download))
    NSLog("webtabs: download failed: %@", error.localizedDescription)
    post(
      "Download failed", body: [dest?.lastPathComponent, error.localizedDescription]
        .compactMap { $0 }.joined(separator: "\n"), info: [:], id: nil)
  }
}

/// Shown over a tab whose page couldn't load.
final class LoadError: NSView {
  private let retry: () -> Void

  init(title: String, detail: String, retry: @escaping () -> Void) {
    self.retry = retry
    super.init(frame: .zero)
    let heading = NSTextField(labelWithString: title)
    heading.font = .systemFont(ofSize: 18, weight: .bold)
    let text = NSTextField(wrappingLabelWithString: detail)
    text.alignment = .center
    text.textColor = .secondaryLabelColor
    text.preferredMaxLayoutWidth = 420
    let button = NSButton(title: "Try Again", target: self, action: #selector(tryAgain))
    let stack = NSStackView(views: [heading, text, button])
    stack.orientation = .vertical
    stack.spacing = 12
    stack.translatesAutoresizingMaskIntoConstraints = false
    addSubview(stack)
    NSLayoutConstraint.activate([
      stack.centerXAnchor.constraint(equalTo: centerXAnchor),
      stack.centerYAnchor.constraint(equalTo: centerYAnchor),
    ])
  }
  required init?(coder: NSCoder) { fatalError() }

  override func draw(_ dirtyRect: NSRect) {
    NSColor.windowBackgroundColor.setFill()
    bounds.fill()
  }
  @objc private func tryAgain() { retry() }
}
