import Foundation

/// Events the notification extension fetched, kept for the app to take into
/// its message store (docs/message-sync.md#push-inbox).
///
/// Layout under the App Group container:
///
///     push-inbox/<token>/<unixms>-<uuid>.json   one fetch
///     push-inbox/<token>/cursor                 newest created_at fetched
///
/// `<token>` is the community's `inboxToken` (32 lowercase hex), so a
/// signed-out or replaced account never shares a folder. Files are written
/// to a `.tmp` name and renamed, so a reader sees whole files only; readers
/// skip `.tmp` names. File contents: `{v:1, relayUrl, account, fetchedAt,
/// events}` with `fetchedAt` in unix seconds.
public struct PushInbox {
  public struct Limits: Equatable {
    public var maxAgeMilliseconds: Int64
    public var maxEvents: Int
    public var maxBytes: Int

    public init(maxAgeMilliseconds: Int64, maxEvents: Int, maxBytes: Int) {
      self.maxAgeMilliseconds = maxAgeMilliseconds
      self.maxEvents = maxEvents
      self.maxBytes = maxBytes
    }

    /// 7 days, 500 events, 2 MB per token.
    public static let standard = Limits(
      maxAgeMilliseconds: 7 * 86_400_000, maxEvents: 500, maxBytes: 2_097_152)
  }

  public static let folderName = "push-inbox"
  public static let cursorName = "cursor"
  public static let fileVersion = 1

  /// `push-inbox/` inside the container.
  public let root: URL
  public let limits: Limits
  private let now: () -> Date
  private let makeID: () -> String

  public init(
    containerURL: URL,
    limits: Limits = .standard,
    now: @escaping () -> Date = Date.init,
    makeID: @escaping () -> String = { UUID().uuidString.lowercased() }
  ) {
    root = containerURL.appendingPathComponent(Self.folderName, isDirectory: true)
    self.limits = limits
    self.now = now
    self.makeID = makeID
  }

  public static func isValidToken(_ token: String) -> Bool {
    token.utf8.count == 32
      && token.utf8.allSatisfy { (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }
  }

  public func folder(token: String) -> URL? {
    guard Self.isValidToken(token) else { return nil }
    return root.appendingPathComponent(token, isDirectory: true)
  }

  /// Keeps one fetch and prunes the token's folder. Returns the new file,
  /// or nil when there was nothing to keep.
  @discardableResult
  public func write(
    token: String,
    relayUrl: String,
    account: String,
    fetchedAt: Int,
    events: [VerifiedNostrEvent]
  ) throws -> URL? {
    guard !events.isEmpty, let folder = try prepare(token: token) else { return nil }
    let file = PushInboxFile(
      v: Self.fileVersion, relayUrl: relayUrl, account: account,
      fetchedAt: fetchedAt, events: events)
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.withoutEscapingSlashes]
    let data = try encoder.encode(file)
    let millis = Int64((now().timeIntervalSince1970 * 1_000).rounded(.down))
    let target = folder.appendingPathComponent("\(millis)-\(makeID()).json")
    try Self.atomicWrite(data, to: target, makeID: makeID)
    try prune(token: token)
    return target
  }

  /// The token's fetch cursor (unix seconds), or nil when absent or damaged.
  public func readCursor(token: String) -> Int? {
    guard let folder = folder(token: token),
      let data = try? Data(contentsOf: folder.appendingPathComponent(Self.cursorName)),
      let text = String(data: data, encoding: .utf8)
    else { return nil }
    return Int(text.trimmingCharacters(in: .whitespacesAndNewlines))
  }

  /// Replaces the token's cursor atomically; the last writer wins.
  public func writeCursor(token: String, value: Int) throws {
    guard let folder = try prepare(token: token) else { return }
    try Self.atomicWrite(
      Data(String(value).utf8), to: folder.appendingPathComponent(Self.cursorName),
      makeID: makeID)
  }

  /// Drops files older than the age limit, then the oldest files while the
  /// folder holds more events or bytes than allowed. `.tmp` files and the
  /// cursor are neither counted nor removed.
  public func prune(token: String) throws {
    guard let folder = folder(token: token) else { return }
    let fileManager = FileManager.default
    let names = (try? fileManager.contentsOfDirectory(atPath: folder.path)) ?? []
    var entries = names.compactMap { name -> (millis: Int64, name: String)? in
      guard let millis = Self.fetchMillis(fileName: name) else { return nil }
      return (millis, name)
    }.sorted { $0.millis == $1.millis ? $0.name < $1.name : $0.millis < $1.millis }

    let nowMillis = Int64((now().timeIntervalSince1970 * 1_000).rounded(.down))
    while let oldest = entries.first, nowMillis - oldest.millis > limits.maxAgeMilliseconds {
      try? fileManager.removeItem(at: folder.appendingPathComponent(oldest.name))
      entries.removeFirst()
    }

    var sizes = entries.map { entry -> (events: Int, bytes: Int) in
      let url = folder.appendingPathComponent(entry.name)
      guard let data = try? Data(contentsOf: url) else { return (0, 0) }
      return (Self.eventCount(data), data.count)
    }
    var events = sizes.reduce(0) { $0 + $1.events }
    var bytes = sizes.reduce(0) { $0 + $1.bytes }
    while !entries.isEmpty, events > limits.maxEvents || bytes > limits.maxBytes {
      try? fileManager.removeItem(at: folder.appendingPathComponent(entries[0].name))
      events -= sizes[0].events
      bytes -= sizes[0].bytes
      entries.removeFirst()
      sizes.removeFirst()
    }
  }

  /// `<unixms>` of a finished fetch file name, or nil for anything else.
  static func fetchMillis(fileName: String) -> Int64? {
    guard fileName.hasSuffix(".json"),
      let dash = fileName.firstIndex(of: "-")
    else { return nil }
    return Int64(fileName[fileName.startIndex..<dash])
  }

  static func eventCount(_ data: Data) -> Int {
    guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let events = object["events"] as? [Any]
    else { return 0 }
    return events.count
  }

  private func prepare(token: String) throws -> URL? {
    guard let folder = folder(token: token) else { return nil }
    let fileManager = FileManager.default
    if !fileManager.fileExists(atPath: root.path) {
      try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
      var excluded = root
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try? excluded.setResourceValues(values)
    }
    try fileManager.createDirectory(at: folder, withIntermediateDirectories: true)
    return folder
  }

  static func atomicWrite(_ data: Data, to target: URL, makeID: () -> String) throws {
    let temporary = target.deletingLastPathComponent()
      .appendingPathComponent("\(target.lastPathComponent).\(makeID()).tmp")
    #if os(iOS)
      try data.write(to: temporary, options: [.completeFileProtectionUntilFirstUserAuthentication])
    #else
      try data.write(to: temporary)
    #endif
    guard rename(temporary.path, target.path) == 0 else {
      let code = errno
      try? FileManager.default.removeItem(at: temporary)
      throw CocoaError(.fileWriteUnknown, userInfo: ["errno": code])
    }
  }
}

/// One inbox file.
public struct PushInboxFile: Codable, Equatable {
  public let v: Int
  public let relayUrl: String
  public let account: String
  public let fetchedAt: Int
  public let events: [VerifiedNostrEvent]
}

/// The NSE's fetch window and cursor rules (fixture section `cursor`).
public enum PushInboxCursor {
  /// Rows can land on the relay up to this long after their `created_at`.
  public static let lookbackSeconds = 1_865
  /// A fetch never reaches further back than this.
  public static let windowSeconds = 259_200

  public static func since(cursor: Int?, now: Int) -> Int {
    let floor = now - windowSeconds
    guard let cursor else { return floor }
    return max(cursor - lookbackSeconds, floor)
  }

  /// The cursor after a fetch. A truncated answer still advances to the
  /// newest returned row: pushed rows claim no completeness and the app's
  /// catch-up fills gaps. The notification fallback query never moves it.
  public static func next(current: Int?, returned: [Int], fallback: Bool) -> Int? {
    guard !fallback, let newest = returned.max() else { return current }
    return newest
  }
}
