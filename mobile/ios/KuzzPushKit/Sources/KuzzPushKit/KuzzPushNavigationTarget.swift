import Foundation

/// A stable destination attached by the notification service extension after
/// it resolves and verifies the event that produced a push wake.
public struct KuzzPushNavigationTarget: Codable, Equatable, Sendable {
  public static let userInfoKey = "kuzz_push_navigation"

  public let eventID: String
  public let communityID: String
  public let channelID: String

  public init(eventID: String, communityID: String, channelID: String) {
    self.eventID = eventID
    self.communityID = communityID
    self.channelID = channelID
  }

  public var userInfoValue: [String: String] {
    [
      "event_id": eventID,
      "community_id": communityID,
      "channel_id": channelID,
    ]
  }

  /// Decodes a target without trusting other fields from the APNs payload.
  public static func decodeIfPresent(
    from userInfo: [AnyHashable: Any]
  ) -> KuzzPushNavigationTarget? {
    guard let raw = userInfo[userInfoKey] as? [String: Any],
      raw.count == 3,
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
      channelID: channelID
    )
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
