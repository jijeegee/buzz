import Foundation

/// A stable destination attached by the notification service extension after
/// it resolves and verifies the event that produced a push wake.
public struct KuzzPushNavigationTarget: Codable, Equatable, Sendable {
  public static let userInfoKey = "kuzz_push_navigation"

  public let eventID: String
  public let communityID: String
  public let channelID: String
  /// The message whose thread holds [eventID] when the event is a thread
  /// reply; nil for messages on the room's main timeline.
  public let threadRootID: String?

  public init(
    eventID: String,
    communityID: String,
    channelID: String,
    threadRootID: String? = nil
  ) {
    self.eventID = eventID
    self.communityID = communityID
    self.channelID = channelID
    self.threadRootID = threadRootID.flatMap { Self.isEventID($0) ? $0 : nil }
  }

  public var userInfoValue: [String: String] {
    var value = [
      "event_id": eventID,
      "community_id": communityID,
      "channel_id": channelID,
    ]
    if let threadRootID {
      value["thread_root_id"] = threadRootID
    }
    return value
  }

  /// The thread a reply opens in, read from its NIP-10 tags like the app's
  /// `threadReferenceOf`: the `reply` parent, unless the reply is broadcast
  /// to the room's main timeline.
  public static func threadRootID(tags: [[String]]) -> String? {
    if tags.contains(where: { $0.count >= 2 && $0[0] == "broadcast" && $0[1] == "1" }) {
      return nil
    }
    guard
      let parent = tags.last(where: { $0.count >= 4 && $0[0] == "e" && $0[3] == "reply" })?[1],
      isEventID(parent)
    else { return nil }
    return parent
  }

  static func isEventID(_ value: String) -> Bool {
    value.utf8.count == 64
      && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
  }

  /// Decodes a target without trusting other fields from the APNs payload.
  public static func decodeIfPresent(
    from userInfo: [AnyHashable: Any]
  ) -> KuzzPushNavigationTarget? {
    guard let raw = userInfo[userInfoKey] as? [String: Any],
      raw.count == 3 || (raw.count == 4 && raw["thread_root_id"] is String),
      let eventID = raw["event_id"] as? String,
      let communityID = raw["community_id"] as? String,
      let channelID = raw["channel_id"] as? String,
      !eventID.isEmpty,
      !communityID.isEmpty,
      !channelID.isEmpty
    else {
      return nil
    }
    return KuzzPushNavigationTarget(
      eventID: eventID,
      communityID: communityID,
      channelID: channelID,
      threadRootID: raw["thread_root_id"] as? String
    )
  }

}

/// A message notification still in Notification Center, as the app reads it
/// to remove the ones whose message was already read.
public enum KuzzDeliveredMessageNotification {
  /// userInfo key of the event's `created_at`, set next to the navigation target.
  public static let createdAtUserInfoKey = "kuzz_push_created_at"

  /// Arguments for Flutter, or nil for notifications without a message
  /// target (generic text, older builds without `created_at`).
  public static func flutterArguments(
    identifier: String,
    userInfo: [AnyHashable: Any]
  ) -> [String: Any]? {
    guard let target = KuzzPushNavigationTarget.decodeIfPresent(from: userInfo),
      let createdAt = (userInfo[createdAtUserInfoKey] as? NSNumber)?.intValue,
      createdAt > 0
    else { return nil }
    var arguments: [String: Any] = [
      "key": identifier,
      "eventId": target.eventID,
      "communityId": target.communityID,
      "channelId": target.channelID,
      "createdAt": createdAt,
    ]
    if let threadRootID = target.threadRootID {
      arguments["threadRootId"] = threadRootID
    }
    return arguments
  }
}

/// Thread-safe one-item buffer spanning notification delivery and Flutter
/// engine startup during a cold notification launch.
public final class KuzzPushNavigationBuffer: @unchecked Sendable {
  private let lock = NSLock()
  private var target: KuzzPushNavigationTarget?

  public init() {}

  public func record(_ target: KuzzPushNavigationTarget) {
    lock.lock()
    self.target = target
    lock.unlock()
  }

  public func peek() -> KuzzPushNavigationTarget? {
    lock.lock()
    defer { lock.unlock() }
    return target
  }

  public func take() -> KuzzPushNavigationTarget? {
    lock.lock()
    defer { lock.unlock() }
    let current = target
    target = nil
    return current
  }

  public func remove(ifMatching expected: KuzzPushNavigationTarget) {
    lock.lock()
    defer { lock.unlock() }
    if target == expected {
      target = nil
    }
  }
}
