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

@Test func `Round-trip thread reply target and drop a malformed thread`() {
  let thread = String(repeating: "c", count: 64)
  let target = KuzzPushNavigationTarget(
    eventID: "message-id",
    communityID: "community-id",
    channelID: "channel-id",
    threadRootID: thread
  )

  #expect(target.userInfoValue["thread_root_id"] == thread)
  #expect(
    KuzzPushNavigationTarget.decodeIfPresent(
      from: [KuzzPushNavigationTarget.userInfoKey: target.userInfoValue]
    ) == target
  )

  let malformed = KuzzPushNavigationTarget.decodeIfPresent(
    from: [
      KuzzPushNavigationTarget.userInfoKey: [
        "event_id": "message-id",
        "community_id": "community-id",
        "channel_id": "channel-id",
        "thread_root_id": "not-an-event-id",
      ]
    ]
  )
  #expect(malformed?.channelID == "channel-id")
  #expect(malformed?.threadRootID == nil)
}

@Test func `Thread of a reply follows its tags`() {
  let head = String(repeating: "a", count: 64)
  let parent = String(repeating: "b", count: 64)

  #expect(KuzzPushNavigationTarget.threadRootID(tags: [["h", "room"]]) == nil)
  #expect(KuzzPushNavigationTarget.threadRootID(tags: [["e", head, "", "reply"]]) == head)
  #expect(
    KuzzPushNavigationTarget.threadRootID(
      tags: [["e", head, "", "root"], ["e", parent, "", "reply"]]
    ) == parent
  )
  #expect(
    KuzzPushNavigationTarget.threadRootID(
      tags: [["e", head, "", "reply"], ["broadcast", "1"]]
    ) == nil
  )
  #expect(KuzzPushNavigationTarget.threadRootID(tags: [["e", "short", "", "reply"]]) == nil)
  #expect(
    KuzzPushNavigationTarget.threadRootID(
      tags: [["e", String(repeating: "A", count: 64), "", "reply"]]
    ) == nil
  )
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
