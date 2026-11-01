import CryptoKit
import Foundation
import XCTest

@testable import KuzzPushKit

/// The NSE's device-tagged NIP-98 events against the relay's shared vectors
/// (`crates/kuzz-auth/src/device_proof_vectors.json`).
final class DeviceEventSigningTests: XCTestCase {
    private let personKey = String(repeating: "0", count: 63) + "1"
    private let url = URL(string: "https://relay.example/query")!

    // MARK: Shared vectors

    func testCanonicalSerializationReproducesVectorMessagesAndIDs() throws {
        let vectors = try Self.vectors()
        let cases = try XCTUnwrap(vectors["event_tags"] as? [[String: Any]])
        XCTAssertEqual(cases.count, 2)
        for entry in cases {
            let event = try Self.event(from: XCTUnwrap(entry["event"] as? [String: Any]))
            let expected = try XCTUnwrap(entry["message"] as? String)
            let message = DeviceEventSigning.proofMessage(
                pubkey: event.pubkey, createdAt: event.createdAt, kind: event.kind,
                tags: event.tags, content: event.content
            )
            XCTAssertEqual(String(decoding: message, as: UTF8.self), expected)
            let serialized = try VerifiedNostrEvent.canonicalSerialization(
                pubkey: event.pubkey, createdAt: event.createdAt, kind: event.kind,
                tags: event.tags, content: event.content
            )
            XCTAssertEqual(VerifiedNostrEvent.hex(SHA256.hash(data: serialized)), event.id)
            XCTAssertTrue(event.hasValidIDAndSignature())
        }
    }

    func testVectorDeviceSignaturesVerifyAndPubkeyCompresses() throws {
        let vectors = try Self.vectors()
        let device = try Self.vectorDeviceKey(vectors)
        let expectedPubkey = try XCTUnwrap(vectors["device_pubkey_hex"] as? String)
        let compressed = try XCTUnwrap(
            DeviceEventSigning.compressedSEC1(device.publicKey.x963Representation))
        XCTAssertEqual(VerifiedNostrEvent.hex(compressed), expectedPubkey)

        for entry in try XCTUnwrap(vectors["event_tags"] as? [[String: Any]]) {
            let event = try Self.event(from: XCTUnwrap(entry["event"] as? [String: Any]))
            let tag = try XCTUnwrap(event.tags.first { $0.first == "device" })
            XCTAssertEqual(tag[1], expectedPubkey)
            let signature = try P256.Signing.ECDSASignature(
                rawRepresentation: XCTUnwrap(VerifiedNostrEvent.hexBytes(tag[2])))
            let message = DeviceEventSigning.proofMessage(
                pubkey: event.pubkey, createdAt: event.createdAt, kind: event.kind,
                tags: event.tags, content: event.content
            )
            XCTAssertTrue(device.publicKey.isValidSignature(signature, for: message))
        }
    }

    func testEscapingVectorCharactersSerializeRaw() throws {
        var out = ""
        VerifiedNostrEvent.appendJSONString(
            "q\" b\\ \u{08}\u{0C}\n\r\t \u{07} \u{1F} \u{7F} / \u{2028} 한", to: &out)
        XCTAssertEqual(
            out,
            "\"q\\\" b\\\\ \\b\\f\\n\\r\\t \\u0007 \\u001f \u{7F} / \u{2028} 한\""
        )
    }

    // MARK: Header

    func testHeaderCarriesVerifiableDeviceTag() throws {
        let device = try Self.vectorDeviceKey(Self.vectors())
        let signer = try XCTUnwrap(Self.signer(device))
        let event = try Self.decode(
            NostrHTTPAuth.authorizationHeader(
                url: url, method: "POST", body: Data("[]".utf8), privateKeyHex: personKey,
                createdAt: 1_700_000_000, deviceSigner: signer
            ))
        XCTAssertTrue(event.hasValidIDAndSignature())
        let deviceTags = event.tags.filter { $0.first == "device" }
        XCTAssertEqual(deviceTags.count, 1)
        let tag = try XCTUnwrap(deviceTags.first)
        XCTAssertEqual(tag.count, 3)
        XCTAssertEqual(tag[1], signer.publicKeyHex)
        let raw = try XCTUnwrap(VerifiedNostrEvent.hexBytes(tag[2]))
        XCTAssertEqual(raw.count, 64)
        let signature = try P256.Signing.ECDSASignature(rawRepresentation: raw)
        let message = DeviceEventSigning.proofMessage(
            pubkey: event.pubkey, createdAt: event.createdAt, kind: event.kind,
            tags: event.tags, content: event.content
        )
        XCTAssertTrue(device.publicKey.isValidSignature(signature, for: message))
        XCTAssertEqual(event.tags.filter { $0.first == "nonce" }.count, 1)
    }

    func testHeaderWithoutSignerOrWithFailingSignerHasNoDeviceTag() throws {
        let plain = try Self.decode(
            NostrHTTPAuth.authorizationHeader(
                url: url, method: "POST", body: Data(), privateKeyHex: personKey))
        XCTAssertFalse(plain.tags.contains { $0.first == "device" })

        let device = try Self.vectorDeviceKey(Self.vectors())
        let failing = try XCTUnwrap(
            DeviceEventSigning(
                publicKeySEC1: device.publicKey.x963Representation,
                signDER: { _ in nil }))
        let fallback = try Self.decode(
            NostrHTTPAuth.authorizationHeader(
                url: url, method: "POST", body: Data(), privateKeyHex: personKey,
                deviceSigner: failing))
        XCTAssertFalse(fallback.tags.contains { $0.first == "device" })
        XCTAssertTrue(fallback.hasValidIDAndSignature())
    }

    func testTwoHeadersInTheSameSecondDiffer() throws {
        let first = try Self.decode(
            NostrHTTPAuth.authorizationHeader(
                url: url, method: "POST", body: Data(), privateKeyHex: personKey,
                createdAt: 1_700_000_000))
        let second = try Self.decode(
            NostrHTTPAuth.authorizationHeader(
                url: url, method: "POST", body: Data(), privateKeyHex: personKey,
                createdAt: 1_700_000_000))
        XCTAssertNotEqual(first.id, second.id)
        let firstNonce = first.tags.first { $0.first == "nonce" }?.last
        let secondNonce = second.tags.first { $0.first == "nonce" }?.last
        XCTAssertNotNil(firstNonce)
        XCTAssertNotEqual(firstNonce, secondNonce)
    }

    // MARK: Helpers

    func testDERSignatureConvertsToRaw() throws {
        let device = try Self.vectorDeviceKey(Self.vectors())
        for index in 0..<16 {
            let signature = try device.signature(for: Data("message \(index)".utf8))
            XCTAssertEqual(
                DeviceEventSigning.rawSignature(fromDER: signature.derRepresentation),
                signature.rawRepresentation
            )
        }
        XCTAssertNil(DeviceEventSigning.rawSignature(fromDER: Data([0x30, 0x02, 0x02, 0x00])))
        XCTAssertNil(DeviceEventSigning.rawSignature(fromDER: Data()))
    }

    func testCompressedSEC1RejectsMalformedKeys() {
        XCTAssertNil(DeviceEventSigning.compressedSEC1(Data(repeating: 4, count: 64)))
        XCTAssertNil(DeviceEventSigning.compressedSEC1(Data([UInt8]([0x05]) + [UInt8](repeating: 1, count: 64))))
        let compressed = Data([UInt8]([0x02]) + [UInt8](repeating: 1, count: 32))
        XCTAssertEqual(DeviceEventSigning.compressedSEC1(compressed), compressed)
    }

    static func signer(_ device: P256.Signing.PrivateKey) -> DeviceEventSigning? {
        DeviceEventSigning(
            publicKeySEC1: device.publicKey.x963Representation,
            signDER: { message in try? device.signature(for: message).derRepresentation }
        )
    }

    static func decode(_ header: String) throws -> VerifiedNostrEvent {
        XCTAssertTrue(header.hasPrefix("Nostr "))
        let data = try XCTUnwrap(Data(base64Encoded: String(header.dropFirst(6))))
        return try JSONDecoder().decode(VerifiedNostrEvent.self, from: data)
    }

    static func vectorDeviceKey(_ vectors: [String: Any]) throws -> P256.Signing.PrivateKey {
        let secret = try XCTUnwrap(vectors["device_secret_hex"] as? String)
        return try P256.Signing.PrivateKey(
            rawRepresentation: XCTUnwrap(VerifiedNostrEvent.hexBytes(secret)))
    }

    static func vectors() throws -> [String: Any] {
        // Tests/KuzzPushKitTests/<file> → repo root is six levels up.
        var root = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { root.deleteLastPathComponent() }
        let url = root.appendingPathComponent("crates/kuzz-auth/src/device_proof_vectors.json")
        let data = try Data(contentsOf: url)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    static func event(from object: [String: Any]) throws -> VerifiedNostrEvent {
        let data = try JSONSerialization.data(withJSONObject: object)
        return try JSONDecoder().decode(VerifiedNostrEvent.self, from: data)
    }
}
