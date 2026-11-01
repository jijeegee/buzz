import Foundation
import XCTest

@testable import KuzzPushKit

/// The app exports `inboxToken` per community only while the push inbox is
/// on (`KUZZ_PUSH_INBOX`); snapshots without it keep decoding.
final class PushInboxTokenTests: XCTestCase {
  func testSnapshotCommunityDecodesWithAndWithoutInboxToken() throws {
    let token = String(repeating: "ab", count: 16)
    let with = try JSONDecoder().decode(
      PushLeaseCommunity.self,
      from: Data(
        """
        {"id":"c","name":"C","relayUrl":"wss://relay.example","policies":[],\
        "inboxToken":"\(token)"}
        """.utf8))
    XCTAssertEqual(with.inboxToken, token)

    let without = try JSONDecoder().decode(
      PushLeaseCommunity.self,
      from: Data(#"{"id":"c","name":"C","relayUrl":"wss://relay.example","policies":[]}"#.utf8))
    XCTAssertNil(without.inboxToken)
    XCTAssertEqual(
      try JSONDecoder().decode(
        PushLeaseCommunity.self, from: JSONEncoder().encode(with)),
      with)
  }
}
