import Foundation
import XCTest

@testable import KuzzPushKit

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// The resolver's push inbox path (plan P1.4): bounded fetch from the
/// cursor, the shared byte cap, today's query as the notification fallback,
/// and what is kept for the app.
final class PushInboxResolverTests: XCTestCase {
  typealias Stub = KuzzPushNotificationResolverTests.URLProtocolStub
  typealias Base = KuzzPushNotificationResolverTests

  private let token = String(repeating: "c3", count: 16)
  private var container: URL!
  private var bodies: [[[String: Any]]] = []
  private let bodyLock = NSLock()

  override func setUpWithError() throws {
    Stub.reset()
    bodies = []
    container = FileManager.default.temporaryDirectory
      .appendingPathComponent("push-inbox-resolver-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: container, withIntermediateDirectories: true)
  }

  override func tearDownWithError() throws {
    Stub.reset()
    try? FileManager.default.removeItem(at: container)
  }

  // MARK: Kept events and cursor

  func testKeepsEveryMatchingEventAndAdvancesTheCursor() throws {
    let older = try message(createdAt: Base.now - 30, content: "older")
    let newer = try message(createdAt: Base.now - 10, content: "newer")
    let elsewhere = try message(createdAt: Base.now - 5, content: "x", room: "other-room")
    respond { index, _ in index == 1 ? [older, newer, elsewhere] : [] }

    let inbox = makeInbox()
    let result = try XCTUnwrap(resolve(makeResolver(inbox: inbox)))

    XCTAssertEqual(result.body, "newer", "the notification is the newest message")
    let first = try XCTUnwrap(bodies.first)
    XCTAssertEqual(first.first?["since"] as? Int, Base.now - PushInboxCursor.windowSeconds)
    XCTAssertEqual(first.first?["limit"] as? Int, 50)
    let file = try XCTUnwrap(inboxFiles(inbox).first)
    XCTAssertEqual(inboxFiles(inbox).count, 1)
    XCTAssertEqual(file.v, 1)
    XCTAssertEqual(file.account, Base.ownPubkey)
    XCTAssertEqual(file.relayUrl, "https://relay.example")
    XCTAssertEqual(file.fetchedAt, Base.now)
    XCTAssertEqual(Set(file.events.filter { $0.kind == 9 }.map(\.id)), [older.id, newer.id])
    XCTAssertEqual(inbox.readCursor(token: token), elsewhere.createdAt)
  }

  func testCursorSetsTheWindowAndATruncatedAnswerStillAdvancesIt() throws {
    let inbox = makeInbox()
    try inbox.writeCursor(token: token, value: Base.now - 5_000)
    let rows = try (0..<50).map { try message(createdAt: Base.now - 4_000 + $0, content: "m\($0)") }
    respond { index, _ in index == 1 ? rows : [] }

    _ = resolve(makeResolver(inbox: inbox))

    XCTAssertEqual(bodies.first?.first?["since"] as? Int, Base.now - 5_000 - 1_865)
    XCTAssertEqual(inbox.readCursor(token: token), Base.now - 4_000 + 49)
  }

  func testWriteFinishesBeforeTheNotificationIsHandedBack() throws {
    let event = try message(createdAt: Base.now - 1, content: "hi")
    respond { index, _ in index == 1 ? [event] : [] }
    let inbox = makeInbox()
    let done = expectation(description: "resolved")
    var filesAtCompletion = -1
    makeResolver(inbox: inbox).resolve { _ in
      filesAtCompletion = self.inboxFiles(inbox).count
      done.fulfill()
    }
    wait(for: [done], timeout: 5)
    XCTAssertEqual(filesAtCompletion, 1)
  }

  // MARK: Fallback

  func testByteCapCancelsAndTodaysQueryDecidesTheNotification() throws {
    let big = try (0..<20).map { try message(createdAt: Base.now - 100 + $0, content: "big \($0)") }
    let small = try message(createdAt: Base.now - 50, content: "from fallback")
    respond { index, _ in index == 1 ? big : (index == 2 ? [small] : []) }
    let inbox = makeInbox()

    let result = try XCTUnwrap(resolve(makeResolver(inbox: inbox, byteBudget: 2_000)))

    XCTAssertEqual(result.body, "from fallback")
    XCTAssertGreaterThanOrEqual(bodies.count, 2)
    let fallback = try XCTUnwrap(bodies.dropFirst().first?.first)
    XCTAssertNil(fallback["since"])
    XCTAssertEqual(fallback["limit"] as? Int, 10)
    XCTAssertNil(inbox.readCursor(token: token), "the fallback never moves the cursor")
    XCTAssertEqual(inboxFiles(inbox).flatMap(\.events).map(\.id), [small.id])
  }

  func testDeadlineCancelsTheBoundedFetch() throws {
    let late = try message(createdAt: Base.now - 10, content: "late")
    let fallback = try message(createdAt: Base.now - 20, content: "on time")
    respond { index, _ in
      if index == 1 { Thread.sleep(forTimeInterval: 1.5) }
      return index == 1 ? [late] : (index == 2 ? [fallback] : [])
    }
    let inbox = makeInbox()

    let result = try XCTUnwrap(
      resolve(makeResolver(inbox: inbox, deadlines: (0.3, 4)), timeout: 6))

    XCTAssertEqual(result.body, "on time")
    XCTAssertNil(inbox.readCursor(token: token))
  }

  func testEmptyBoundedAnswerFallsBackAndLeavesTheCursor() throws {
    let inbox = makeInbox()
    try inbox.writeCursor(token: token, value: Base.now - 100)
    let old = try message(createdAt: Base.now - 90_000, content: "older than the cursor")
    respond { index, _ in index == 2 ? [old] : [] }

    let result = try XCTUnwrap(resolve(makeResolver(inbox: inbox)))

    XCTAssertEqual(result.body, "older than the cursor")
    XCTAssertEqual(inbox.readCursor(token: token), Base.now - 100)
  }

  // MARK: Guards

  func testTokenRemovedDuringTheRunKeepsNothing() throws {
    let event = try message(createdAt: Base.now - 1, content: "hi")
    respond { index, _ in index == 1 ? [event] : [] }
    let inbox = makeInbox()
    let withToken = try snapshot(token: token)
    let withoutToken = try snapshot(token: nil)
    var reads = 0
    let resolver = makeResolver(
      inbox: inbox,
      communities: {
        reads += 1
        return reads == 1 ? withToken : withoutToken
      })

    XCTAssertNotNil(resolve(resolver))
    XCTAssertEqual(bodies.first?.first?["limit"] as? Int, 50)
    XCTAssertTrue(inboxFiles(inbox).isEmpty)
    XCTAssertNil(inbox.readCursor(token: token))
  }

  func testAgeRestrictionKeepsNothing() throws {
    let event = try message(createdAt: Base.now - 1, content: "hi")
    respond { _, _ in [event] }
    let inbox = makeInbox()

    XCTAssertNotNil(resolve(makeResolver(inbox: inbox, restricted: { true })))
    XCTAssertNil(bodies.first?.first?["since"], "restricted runs use today's query")
    XCTAssertEqual(bodies.first?.first?["limit"] as? Int, 10)
    XCTAssertTrue(inboxFiles(inbox).isEmpty)

    Stub.reset()
    bodies = []
    respond { _, _ in [event] }
    var checks = 0
    let restrictedLater = makeResolver(
      inbox: inbox,
      restricted: {
        checks += 1
        return checks > 1
      })
    XCTAssertNotNil(resolve(restrictedLater))
    XCTAssertEqual(bodies.first?.first?["limit"] as? Int, 50)
    XCTAssertTrue(inboxFiles(inbox).isEmpty, "a restriction that starts mid-run still wins")
  }

  func testCommunityWithoutTokenUsesTodaysQuery() throws {
    let event = try message(createdAt: Base.now - 1, content: "hi")
    respond { _, _ in [event] }
    let inbox = makeInbox()
    XCTAssertNotNil(
      resolve(makeResolver(inbox: inbox, communities: { try? self.snapshot(token: nil) })))
    XCTAssertEqual(bodies.first?.first?["limit"] as? Int, 10)
    XCTAssertFalse(FileManager.default.fileExists(atPath: inbox.root.path))
  }

  func testRefreshedProfileIsKeptWithTheNotifiedCommunity() throws {
    let event = try message(createdAt: Base.now - 1, content: "hi")
    let profile = try Base.signedEvent(
      privateKey: Base.profilePrivateKey, createdAt: Base.now - 100, kind: 0,
      content: #"{"display_name":"Alice"}"#)
    respond { index, _ in index == 1 ? [event] : [profile] }
    let inbox = makeInbox()

    let result = try XCTUnwrap(resolve(makeResolver(inbox: inbox)))

    XCTAssertEqual(result.title, "Alice")
    let kept = inboxFiles(inbox).flatMap(\.events)
    XCTAssertEqual(Set(kept.map(\.id)), [event.id, profile.id])
  }

  // MARK: Matcher fixture

  func testMatcherFixture() throws {
    let fixture = try PushInboxTests.fixture()
    let policies = try JSONDecoder().decode(
      [PushResolutionPolicy].self,
      from: JSONSerialization.data(withJSONObject: XCTUnwrap(fixture["matcher_policies"])))
    for entry in try XCTUnwrap(fixture["matcher"] as? [[String: Any]]) {
      let name = try XCTUnwrap(entry["name"] as? String)
      let raw = try XCTUnwrap(entry["event"] as? [String: Any])
      let event = VerifiedNostrEvent(
        id: String(repeating: "0", count: 64), pubkey: try XCTUnwrap(raw["pubkey"] as? String),
        createdAt: 1, kind: try XCTUnwrap(raw["kind"] as? Int),
        tags: try XCTUnwrap(raw["tags"] as? [[String]]), content: "", sig: "")
      let matched = policies.contains { PushLeaseMatcher.matches(event: event, policy: $0) }
      XCTAssertEqual(matched, entry["match"] as? Bool, name)
    }
  }

  // MARK: Helpers

  private func message(
    createdAt: Int, content: String, room: String = Base.channelID
  ) throws -> VerifiedNostrEvent {
    try Base.signedEvent(
      privateKey: Base.profilePrivateKey, createdAt: createdAt, kind: 9,
      tags: [["h", room]], content: content)
  }

  /// Answers request `index` (1-based) with the events `rows` returns and
  /// records each request's filters.
  private func respond(_ rows: @escaping (Int, URLRequest) throws -> [VerifiedNostrEvent]) {
    Stub.handler = { [self] request in
      let filters = (try? JSONSerialization.jsonObject(with: Self.body(request))) as? [[String: Any]]
      bodyLock.lock()
      bodies.append(filters ?? [])
      bodyLock.unlock()
      let index = Stub.requests.count
      return Base.response(request, status: 200, data: try JSONEncoder().encode(rows(index, request)))
    }
  }

  private func makeInbox() -> PushInbox {
    PushInbox(containerURL: container, now: { Date(timeIntervalSince1970: TimeInterval(Base.now)) })
  }

  private func snapshot(token: String?) throws -> Data {
    try JSONEncoder().encode(
      PushLeaseSnapshot(communities: [
        PushLeaseCommunity(
          id: "community-id", name: "Community", relayUrl: "https://relay.example",
          pubkey: Base.ownPubkey,
          policies: [
            PushResolutionPolicy(
              filter: PushLeaseFilter(kinds: [9, 40002, 45001, 45003], hTags: [Base.channelID]))
          ],
          inboxToken: token)
      ]))
  }

  private func makeResolver(
    inbox: PushInbox,
    communities: (() -> Data?)? = nil,
    restricted: @escaping () -> Bool = { false },
    byteBudget: Int = 1_048_576,
    deadlines: (bounded: TimeInterval, fallback: TimeInterval) = (10, 17)
  ) -> KuzzPushNotificationResolver {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [Stub.self]
    let data = try? snapshot(token: token)
    return KuzzPushNotificationResolver(
      session: URLSession(configuration: configuration),
      loadCommunitiesData: communities ?? { data },
      loadPrivateKey: { $0 == "community-id" ? Base.privateKey : nil },
      pushInbox: inbox,
      isAgeRestricted: restricted,
      inboxByteBudget: byteBudget,
      inboxDeadlines: deadlines,
      now: { Date(timeIntervalSince1970: TimeInterval(Base.now)) })
  }

  private func resolve(
    _ resolver: KuzzPushNotificationResolver, timeout: TimeInterval = 5
  ) -> KuzzPushResolution? {
    let done = expectation(description: "resolved")
    var result: KuzzPushResolution?
    resolver.resolve {
      result = $0
      done.fulfill()
    }
    wait(for: [done], timeout: timeout)
    return result
  }

  private func inboxFiles(_ inbox: PushInbox) -> [PushInboxFile] {
    guard let folder = inbox.folder(token: token),
      let names = try? FileManager.default.contentsOfDirectory(atPath: folder.path)
    else { return [] }
    return names.filter { $0.hasSuffix(".json") }.sorted().compactMap {
      try? JSONDecoder().decode(
        PushInboxFile.self, from: Data(contentsOf: folder.appendingPathComponent($0)))
    }
  }

  static func body(_ request: URLRequest) throws -> Data {
    if let body = request.httpBody { return body }
    let stream = try XCTUnwrap(request.httpBodyStream)
    stream.open()
    defer { stream.close() }
    var bytes = Data()
    var buffer = [UInt8](repeating: 0, count: 4_096)
    while true {
      let count = stream.read(&buffer, maxLength: buffer.count)
      if count <= 0 { break }
      bytes.append(buffer, count: count)
    }
    return bytes
  }
}
