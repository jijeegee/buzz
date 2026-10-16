import Foundation
import Testing

@testable import KuzzPushKit

@Test func `Round-trip opaque navigation target through notification user info`() {
  let target = KuzzPushNavigationTarget(
    eventID: "MESSAGE-ID",
    communityID: "community-id",
    channelID: "CHANNEL/GENERAL"
  )

  #expect(
    KuzzPushNavigationTarget.decodeIfPresent(
      from: [KuzzPushNavigationTarget.userInfoKey: target.userInfoValue]
    ) == target
  )
  #expect(target.eventID == "MESSAGE-ID")
  #expect(target.channelID == "CHANNEL/GENERAL")
}

@Test func `Reject incomplete or malformed navigation target`() {
  #expect(
    KuzzPushNavigationTarget.decodeIfPresent(
      from: [
        KuzzPushNavigationTarget.userInfoKey: [
          "event_id": "message-id",
          "community_id": "community-id",
        ]
      ]
    ) == nil
  )

  #expect(
    KuzzPushNavigationTarget.decodeIfPresent(
      from: [
        KuzzPushNavigationTarget.userInfoKey: [
          "event_id": "",
          "community_id": "community-id",
          "channel_id": "channel-id",
        ]
      ]
    ) == nil
  )

  #expect(
    KuzzPushNavigationTarget.decodeIfPresent(
      from: [
        KuzzPushNavigationTarget.userInfoKey: [
          "event_id": "message-id",
          "community_id": "community-id",
          "channel_id": "",
        ]
      ]
    ) == nil
  )
}

@Test func `Buffer preserves cold-start target until consumed`() {
  let first = KuzzPushNavigationTarget(
    eventID: String(repeating: "a", count: 64),
    communityID: "community-id",
    channelID: "123e4567-e89b-42d3-a456-426614174000"
  )
  let second = KuzzPushNavigationTarget(
    eventID: String(repeating: "b", count: 64),
    communityID: "community-id",
    channelID: "123e4567-e89b-42d3-a456-426614174000"
  )
  let buffer = KuzzPushNavigationBuffer()

  buffer.record(first)
  buffer.remove(ifMatching: second)
  #expect(buffer.peek() == first)
  #expect(buffer.take() == first)
  #expect(buffer.take() == nil)
}
