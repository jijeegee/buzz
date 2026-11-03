import Foundation
import XCTest

@testable import KuzzPushKit

/// `PushInbox` against `test-fixtures/push-inbox.json` (sections `caps` and
/// `cursor`) plus the file schema the app's reader expects.
final class PushInboxTests: XCTestCase {
  private let token = String(repeating: "a1", count: 16)
  private var container: URL!

  override func setUpWithError() throws {
    container = FileManager.default.temporaryDirectory
      .appendingPathComponent("push-inbox-tests-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: container, withIntermediateDirectories: true)
  }

  override func tearDownWithError() throws {
    try? FileManager.default.removeItem(at: container)
  }

  // MARK: Files

  func testWriteKeepsOneWholeFileWithTheSchema() throws {
    let inbox = PushInbox(
      containerURL: container, now: { Date(timeIntervalSince1970: 1_700_000_000.5) },
      makeID: { "id" })
    let event = Self.event(index: 1, content: "hello / world")
    let url = try XCTUnwrap(
      inbox.write(
        token: token, relayUrl: "wss://relay.example", account: String(repeating: "aa", count: 32),
        fetchedAt: 1_700_000_000, events: [event]))
    XCTAssertEqual(url.lastPathComponent, "1700000000500-id.json")
    let names = try FileManager.default.contentsOfDirectory(
      atPath: url.deletingLastPathComponent().path)
    XCTAssertEqual(names, ["1700000000500-id.json"], "no temporary file is left")

    let object = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
    XCTAssertEqual(object["v"] as? Int, 1)
    XCTAssertEqual(object["relayUrl"] as? String, "wss://relay.example")
    XCTAssertEqual(object["account"] as? String, String(repeating: "aa", count: 32))
    XCTAssertEqual(object["fetchedAt"] as? Int, 1_700_000_000)
    let events = try XCTUnwrap(object["events"] as? [[String: Any]])
    XCTAssertEqual(events.count, 1)
    XCTAssertEqual(events[0]["created_at"] as? Int, event.createdAt)
    XCTAssertEqual(
      try JSONDecoder().decode(PushInboxFile.self, from: Data(contentsOf: url)).events, [event])

    #if canImport(Darwin)
      let values = try inbox.root.resourceValues(forKeys: [.isExcludedFromBackupKey])
      XCTAssertEqual(values.isExcludedFromBackup, true)
    #endif
  }

  func testNothingIsWrittenForNoEventsOrABadToken() throws {
    let inbox = PushInbox(containerURL: container)
    XCTAssertNil(
      try inbox.write(
        token: token, relayUrl: "wss://r", account: "a", fetchedAt: 1, events: []))
    for bad in ["", "../" + token, token.uppercased(), String(token.dropLast())] {
      XCTAssertNil(
        try inbox.write(
          token: bad, relayUrl: "wss://r", account: "a", fetchedAt: 1,
          events: [Self.event(index: 1)]))
    }
    XCTAssertFalse(FileManager.default.fileExists(atPath: inbox.root.path))
  }

  func testCursorRoundTripsAndLastWriterWins() throws {
    let inbox = PushInbox(containerURL: container)
    XCTAssertNil(inbox.readCursor(token: token))
    try inbox.writeCursor(token: token, value: 100)
    try inbox.writeCursor(token: token, value: 50)
    XCTAssertEqual(inbox.readCursor(token: token), 50)
    let folder = try XCTUnwrap(inbox.folder(token: token))
    XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: folder.path), ["cursor"])
  }

  // MARK: Fixture

  func testCapsFixture() throws {
    let fixture = try Self.fixture()
    let limits = try XCTUnwrap(fixture["caps_limits"] as? [String: Any])
    XCTAssertEqual(
      PushInbox.Limits.standard,
      PushInbox.Limits(
        maxAgeMilliseconds: try XCTUnwrap(limits["max_age_ms"] as? Int64),
        maxEvents: try XCTUnwrap(limits["max_events"] as? Int),
        maxBytes: try XCTUnwrap(limits["max_bytes"] as? Int)))

    for entry in try XCTUnwrap(fixture["caps"] as? [[String: Any]]) {
      let name = try XCTUnwrap(entry["name"] as? String)
      let nowMillis = try XCTUnwrap(entry["now_ms"] as? Double)
      let caseContainer = container.appendingPathComponent(UUID().uuidString)
      let inbox = PushInbox(
        containerURL: caseContainer,
        now: { Date(timeIntervalSince1970: nowMillis / 1_000) })
      let folder = try XCTUnwrap(inbox.folder(token: token))
      try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
      for file in try XCTUnwrap(entry["files"] as? [[String: Any]]) {
        let fileName = try XCTUnwrap(file["name"] as? String)
        let data: Data
        if let raw = file["raw"] as? String {
          data = Data(raw.utf8)
        } else {
          let count = try XCTUnwrap(file["events"] as? Int)
          let contentBytes = file["content_bytes"] as? Int ?? 1
          let events = (0..<count).map {
            Self.event(index: $0, content: String(repeating: "x", count: contentBytes))
          }
          data = try JSONEncoder().encode(
            PushInboxFile(
              v: 1, relayUrl: "wss://relay.example", account: "a", fetchedAt: 1, events: events))
        }
        try data.write(to: folder.appendingPathComponent(fileName))
      }
      try inbox.prune(token: token)
      let kept = try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted()
      XCTAssertEqual(kept, try XCTUnwrap(entry["kept"] as? [String]).sorted(), name)
    }
  }

  func testCursorFixture() throws {
    let fixture = try Self.fixture()
    let limits = try XCTUnwrap(fixture["cursor_limits"] as? [String: Any])
    XCTAssertEqual(PushInboxCursor.lookbackSeconds, limits["lookback_secs"] as? Int)
    XCTAssertEqual(PushInboxCursor.windowSeconds, limits["window_secs"] as? Int)
    for entry in try XCTUnwrap(fixture["cursor"] as? [[String: Any]]) {
      let name = try XCTUnwrap(entry["name"] as? String)
      let now = try XCTUnwrap(entry["now"] as? Int)
      let cursor = entry["cursor"] as? Int
      if let since = entry["since"] as? Int {
        XCTAssertEqual(PushInboxCursor.since(cursor: cursor, now: now), since, name)
      }
      if let returned = entry["returned"] as? [Int] {
        XCTAssertEqual(
          PushInboxCursor.next(
            current: cursor, returned: returned, fallback: entry["fallback"] as? Bool ?? false),
          entry["next"] as? Int, name)
      }
    }
  }

  // MARK: Helpers

  static func event(index: Int, content: String = "m", createdAt: Int = 1_000) -> VerifiedNostrEvent {
    VerifiedNostrEvent(
      id: String(format: "%064x", index), pubkey: String(repeating: "bb", count: 32),
      createdAt: createdAt, kind: 9, tags: [["h", "room"]], content: content,
      sig: String(repeating: "0", count: 128))
  }

  static func fixture() throws -> [String: Any] {
    // Tests/KuzzPushKitTests/<file> → repo root is six levels up.
    var root = URL(fileURLWithPath: #filePath)
    for _ in 0..<6 { root.deleteLastPathComponent() }
    let data = try Data(contentsOf: root.appendingPathComponent("test-fixtures/push-inbox.json"))
    return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
  }
}
