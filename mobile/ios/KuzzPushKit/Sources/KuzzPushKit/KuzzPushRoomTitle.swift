import Foundation

/// Names a notification like its Chats row (the app's `channelTitle` and
/// `chatThreadTitle`). Mirrors `push_room_title.dart`. The extension has no
/// channel list and no localized words, so anything it cannot name falls back
/// to the sender.
public struct KuzzPushRoom: Equatable, Sendable {
  static let memberLimit = 3

  public let name: String
  public let isDM: Bool
  public let isGroupChat: Bool
  /// The kind-39000 `p` tags, lowercase: a DM's members.
  public let dmMembers: [String]

  public init?(metadata event: VerifiedNostrEvent) {
    guard event.kind == 39_000 else { return nil }
    var name = ""
    var type = ""
    var groupChat = false
    var members: [String] = []
    for tag in event.tags {
      guard let key = tag.first else { continue }
      // A bare one-element marker.
      if key == "group_chat" { groupChat = true }
      guard tag.count >= 2 else { continue }
      switch key {
      case "name": name = tag[1].trimmingCharacters(in: .whitespacesAndNewlines)
      case "t": type = tag[1]
      case "p":
        let pubkey = tag[1].lowercased()
        if !members.contains(pubkey) { members.append(pubkey) }
      default: break
      }
    }
    self.name = name
    isDM = type == "dm"
    isGroupChat = groupChat && type != "dm"
    dmMembers = type == "dm" ? members : []
  }

  /// A DM of exactly two people: titled by the sender.
  public var isOneToOne: Bool { isDM && dmMembers.count == 2 }

  /// Titled by its other members: a group DM (its name is generic) or an
  /// unnamed group chat.
  public var namedByMembers: Bool { (isDM && !isOneToOne) || (isGroupChat && name.isEmpty) }

  /// The members other than `me`, lowercase, in the order given.
  public static func others(_ pubkeys: [String], me: String?) -> [String] {
    let mine = me?.lowercased()
    var seen = Set<String>()
    return pubkeys.map { $0.lowercased() }.filter { $0 != mine && seen.insert($0).inserted }
  }

  /// "Ann, Bob, Cy +2" over `others`. Empty when there is nobody else.
  public static func memberTitle(others: [String], names: [String: String]) -> String {
    let shown = others.prefix(memberLimit).map {
      names[$0] ?? KuzzPushNotificationResolver.shortPubkey($0)
    }.joined(separator: ", ")
    let hidden = others.count - min(others.count, memberLimit)
    return hidden > 0 ? "\(shown) +\(hidden)" : shown
  }

  /// A thread's title: its name, else the root's first line, else the new
  /// message's (`fallback`).
  public static func threadTitle(
    threadName: String?, rootContent: String?, fallback: String
  ) -> String {
    if let name = threadName?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
      return name
    }
    let firstLine = rootContent?
      .components(separatedBy: "\n")
      .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
      .first { !$0.isEmpty }
    guard let firstLine else { return fallback }
    return firstLine.count > 80 ? String(firstLine.prefix(80)) + "…" : firstLine
  }

  /// The notification's room title, or nil when the sender alone titles it:
  /// a 1:1 DM, or a room with no usable name.
  public func conversationTitle(
    memberTitle: String, sender: String, threadTitle: String?
  ) -> String? {
    let roomName: String?
    if isOneToOne {
      roomName = nil
    } else if namedByMembers {
      roomName = memberTitle.isEmpty ? nil : memberTitle
    } else {
      roomName = name.isEmpty ? nil : name
    }
    guard let threadTitle else { return roomName }
    return "\(roomName ?? sender) > \(threadTitle)"
  }
}
