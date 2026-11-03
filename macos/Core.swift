import Foundation

@_silgen_name("webtabs_request") private func coreRequest(_ request: UnsafePointer<CChar>)
  -> UnsafeMutablePointer<CChar>
@_silgen_name("webtabs_free") private func coreFree(_ response: UnsafeMutablePointer<CChar>)

/// The shared Rust core (src/lib.rs): the same config, routing, badge, favicon and
/// preset code the Linux build uses, behind an owned JSON/C interface.
enum Core {
  struct Failure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
  }

  static func call<T: Decodable>(_ op: String, _ args: [String: Any] = [:]) throws -> T {
    var request = args
    request["op"] = op
    let data = try JSONSerialization.data(withJSONObject: request)
    let pointer = String(decoding: data, as: UTF8.self).withCString { coreRequest($0) }
    defer { coreFree(pointer) }
    let reply = try JSONDecoder().decode(Reply<T>.self, from: Data(String(cString: pointer).utf8))
    return try reply.value.get()
  }

  /// For operations that cannot fail on well-formed input.
  static func get<T: Decodable>(_ op: String, _ args: [String: Any] = [:], or fallback: T) -> T {
    (try? call(op, args)) ?? fallback
  }

  static func json<T: Encodable>(_ value: T) -> Any {
    (try? JSONEncoder().encode(value)).flatMap { try? JSONSerialization.jsonObject(with: $0) }
      ?? NSNull()
  }

  static func route(home: String, url: String) -> Bool {
    get("route", ["home": home, "url": url], or: false)
  }
  static func sameSite(_ a: String, _ b: String) -> Bool {
    get("same_site", ["a": a, "b": b], or: false)
  }
  static func safeExternal(_ url: String) -> Bool { get("safe_external", ["url": url], or: false) }
  static func badge(_ title: String) -> Badge { get("badge", ["title": title], or: Badge()) }
  static func site(_ url: String) -> String? { get("site", ["url": url], or: nil) }
  static func userAgent(_ config: Config, _ tab: Tab) -> String {
    get("user_agent", ["config": json(config), "tab": json(tab)], or: userAgents.defaultAgent)
  }
  static func favicons(page: String, links: String) -> [String] {
    get("favicons", ["url": page, "links": links], or: [])
  }
  static func iconCachePath(_ url: String) -> String? { get("icon_cache", ["url": url], or: nil) }

  static let presets: [Preset] = get("presets", or: [])
  static let userAgents: UserAgents = get(
    "user_agents", or: UserAgents(defaultAgent: "", presets: []))
  static let faviconScript: String = get("favicon_script", or: "'[]'")
}

private struct Reply<T: Decodable>: Decodable {
  let value: Result<T, Core.Failure>
  enum Key: CodingKey { case ok, result, error }
  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: Key.self)
    if try c.decode(Bool.self, forKey: .ok) {
      value = .success(try c.decode(T.self, forKey: .result))
    } else {
      let message = try c.decodeIfPresent(String.self, forKey: .error) ?? "Core error"
      value = .failure(Core.Failure(message: message))
    }
  }
}

/// One `[[tab]]` entry of tabs.toml (`config::Tab`).
struct Tab: Codable, Equatable {
  var name: String
  var url: String
  var notifications = true
  var label: String?
  var session: String?
  var userAgent: String?

  enum CodingKeys: String, CodingKey {
    case name, url, notifications, label, session
    case userAgent = "user_agent"
  }

  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    name = try c.decode(String.self, forKey: .name)
    url = try c.decode(String.self, forKey: .url)
    // Rust leaves `notifications` out when it is the default (on).
    notifications = try c.decodeIfPresent(Bool.self, forKey: .notifications) ?? true
    label = try c.decodeIfPresent(String.self, forKey: .label)
    session = try c.decodeIfPresent(String.self, forKey: .session)
    userAgent = try c.decodeIfPresent(String.self, forKey: .userAgent)
  }
}

/// The whole tabs.toml (`config::Config`).
struct Config: Codable {
  var notifications = true
  var tray = true
  var closeToTray = true
  var unreadInTitle = true
  var unreadInDock = true
  /// Linux/Windows only; kept so saving from here doesn't drop it.
  var gpuAcceleration = true
  var iconSize = 22
  /// A preset name (`Appearance.tints`) or `#rrggbb`; nil is the system gray.
  var tint: String?
  var userAgent: String?
  var tabs: [Tab] = []

  enum CodingKeys: String, CodingKey {
    case notifications, tray, tint
    case closeToTray = "close_to_tray"
    case unreadInTitle = "unread_in_title"
    case unreadInDock = "unread_in_dock"
    case gpuAcceleration = "gpu_acceleration"
    case iconSize = "icon_size"
    case userAgent = "user_agent"
    case tabs = "tab"
  }

  init() {}

  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    let d = Config()
    // Rust leaves defaults out.
    notifications = try c.decodeIfPresent(Bool.self, forKey: .notifications) ?? d.notifications
    tray = try c.decodeIfPresent(Bool.self, forKey: .tray) ?? d.tray
    closeToTray = try c.decodeIfPresent(Bool.self, forKey: .closeToTray) ?? d.closeToTray
    unreadInTitle = try c.decodeIfPresent(Bool.self, forKey: .unreadInTitle) ?? d.unreadInTitle
    unreadInDock = try c.decodeIfPresent(Bool.self, forKey: .unreadInDock) ?? d.unreadInDock
    gpuAcceleration =
      try c.decodeIfPresent(Bool.self, forKey: .gpuAcceleration) ?? d.gpuAcceleration
    iconSize = try c.decodeIfPresent(Int.self, forKey: .iconSize) ?? d.iconSize
    tint = try c.decodeIfPresent(String.self, forKey: .tint)
    userAgent = try c.decodeIfPresent(String.self, forKey: .userAgent)
    tabs = try c.decodeIfPresent([Tab].self, forKey: .tabs) ?? []
  }
}

/// Appearance limits and tint names from the core (`config::TINTS`, `ICON_SIZES`).
struct Appearance: Decodable {
  struct Sizes: Decodable {
    let defaultSize: Int
    let min: Int
    let max: Int
    enum CodingKeys: String, CodingKey {
      case defaultSize = "default"
      case min, max
    }
  }
  let tints: [String]
  let iconSize: Sizes
  enum CodingKeys: String, CodingKey {
    case tints
    case iconSize = "icon_size"
  }

  static let shared: Appearance = Core.get(
    "appearance",
    or: Appearance(tints: [], iconSize: Sizes(defaultSize: 22, min: 16, max: 32)))
}

/// Unread state parsed from a page title (`badge::Badge`).
struct Badge: Decodable, Equatable {
  var label = ""
  var count = 0
  var unread = false
}

struct Preset: Decodable {
  let name: String
  let url: String
}

struct UserAgents: Decodable {
  let defaultAgent: String
  /// `[name, user agent]` pairs offered under Advanced.
  let presets: [[String]]
  enum CodingKeys: String, CodingKey {
    case defaultAgent = "default"
    case presets
  }
}

struct Loaded: Decodable {
  let config: Config
  let path: String
  let data: String
}

struct MadeTab: Decodable {
  let valid: Bool
  let tab: Tab
}
