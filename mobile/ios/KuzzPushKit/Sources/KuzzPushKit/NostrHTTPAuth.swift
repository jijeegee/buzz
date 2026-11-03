import CryptoKit
import Foundation
import P256K

public enum NostrHTTPAuthError: Error, Equatable {
    case invalidHex
    case signingFailed
}

public struct VerifiedNostrEvent: Codable, Equatable, Sendable {
    public let id: String
    public let pubkey: String
    public let createdAt: Int
    public let kind: Int
    public let tags: [[String]]
    public let content: String
    public let sig: String

    enum CodingKeys: String, CodingKey {
        case id, pubkey, kind, tags, content, sig
        case createdAt = "created_at"
    }

    public init(
        id: String, pubkey: String, createdAt: Int, kind: Int,
        tags: [[String]], content: String, sig: String
    ) {
        self.id = id
        self.pubkey = pubkey
        self.createdAt = createdAt
        self.kind = kind
        self.tags = tags
        self.content = content
        self.sig = sig
    }

    public func hasValidIDAndSignature() -> Bool {
        guard let idBytes = Self.hexBytes(id), idBytes.count == 32,
              let pubkeyBytes = Self.hexBytes(pubkey), pubkeyBytes.count == 32,
              let signatureBytes = Self.hexBytes(sig), signatureBytes.count == 64,
              let serialized = try? Self.canonicalSerialization(
                pubkey: pubkey.lowercased(), createdAt: createdAt, kind: kind,
                tags: tags, content: content
              )
        else { return false }
        let digest = Array(SHA256.hash(data: serialized))
        guard digest == idBytes,
              let signature = try? P256K.Schnorr.SchnorrSignature(
                dataRepresentation: Data(signatureBytes)
              )
        else { return false }
        var message = digest
        let key = P256K.Schnorr.XonlyKey(dataRepresentation: pubkeyBytes)
        return key.isValid(signature, for: &message)
    }

    /// NIP-01 id serialization `[0,pubkey,created_at,kind,tags,content]`,
    /// written by hand so it matches the relay (serde_json) and Dart byte for
    /// byte (`device_proof_vectors.json`): no whitespace; strings escape only
    /// quote, backslash, `\b`, `\f`, `\n`, `\r`, `\t` and other characters
    /// below U+0020 as lower-case `\u00xx`; `/`, U+007F, U+2028 and all
    /// non-ASCII stay raw UTF-8.
    static func canonicalSerialization(
        pubkey: String, createdAt: Int, kind: Int, tags: [[String]], content: String
    ) throws -> Data {
        Data(canonicalText(
            pubkey: pubkey, createdAt: createdAt, kind: kind, tags: tags, content: content
        ).utf8)
    }

    static func canonicalText(
        pubkey: String, createdAt: Int, kind: Int, tags: [[String]], content: String
    ) -> String {
        var out = "[0,"
        appendJSONString(pubkey, to: &out)
        out += ",\(createdAt),\(kind),["
        for (tagIndex, tag) in tags.enumerated() {
            if tagIndex > 0 { out += "," }
            out += "["
            for (valueIndex, value) in tag.enumerated() {
                if valueIndex > 0 { out += "," }
                appendJSONString(value, to: &out)
            }
            out += "]"
        }
        out += "],"
        appendJSONString(content, to: &out)
        out += "]"
        return out
    }

    private static let hexDigits = Array("0123456789abcdef")

    static func appendJSONString(_ value: String, to out: inout String) {
        let quote: Unicode.Scalar = "\u{22}"
        let backslash: Unicode.Scalar = "\u{5C}"
        out.unicodeScalars.append(quote)
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 0x22, 0x5C:
                out.unicodeScalars.append(backslash)
                out.unicodeScalars.append(scalar)
            case 0x08: out.unicodeScalars.append(backslash); out += "b"
            case 0x0C: out.unicodeScalars.append(backslash); out += "f"
            case 0x0A: out.unicodeScalars.append(backslash); out += "n"
            case 0x0D: out.unicodeScalars.append(backslash); out += "r"
            case 0x09: out.unicodeScalars.append(backslash); out += "t"
            case 0..<0x20:
                out.unicodeScalars.append(backslash)
                out += "u00"
                out.append(hexDigits[Int(scalar.value >> 4)])
                out.append(hexDigits[Int(scalar.value & 0x0F)])
            default:
                out.unicodeScalars.append(scalar)
            }
        }
        out.unicodeScalars.append(quote)
    }

    static func hexBytes(_ value: String) -> [UInt8]? {
        guard value.count.isMultiple(of: 2) else { return nil }
        var result: [UInt8] = []
        result.reserveCapacity(value.count / 2)
        var index = value.startIndex
        while index < value.endIndex {
            let end = value.index(index, offsetBy: 2)
            guard let byte = UInt8(value[index..<end], radix: 16) else { return nil }
            result.append(byte)
            index = end
        }
        return result
    }

    static func hex(_ bytes: some Sequence<UInt8>) -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }
}

public enum NostrHTTPAuth {
    /// A NIP-98 `Authorization` header for one request.
    ///
    /// Every event carries a fresh `["nonce", <uuid>]` tag so two requests
    /// signed in the same second never share an id (the relay replay-checks
    /// NIP-98 ids). With a `deviceSigner` the event also carries this
    /// install's `["device", <pubkey>, <sig>]` tag; if the device signature
    /// fails the event is signed exactly as without one.
    public static func authorizationHeader(
        url: URL,
        method: String,
        body: Data,
        privateKeyHex: String,
        createdAt: Int = Int(Date().timeIntervalSince1970),
        auxiliaryRandomness: [UInt8]? = nil,
        nonce: String = UUID().uuidString.lowercased(),
        deviceSigner: DeviceEventSigning? = nil
    ) throws -> String {
        guard let privateKeyBytes = VerifiedNostrEvent.hexBytes(privateKeyHex),
              privateKeyBytes.count == 32
        else { throw NostrHTTPAuthError.invalidHex }
        do {
            let privateKey = try P256K.Schnorr.PrivateKey(
                dataRepresentation: privateKeyBytes
            )
            let pubkey = VerifiedNostrEvent.hex(privateKey.xonly.bytes)
            let payload = VerifiedNostrEvent.hex(SHA256.hash(data: body))
            let baseTags = [
                ["u", url.absoluteString],
                ["method", method.uppercased()],
                ["payload", payload],
                ["nonce", nonce],
            ]
            let tags = deviceSigner?.tagged(
                tags: baseTags, pubkey: pubkey, createdAt: createdAt,
                kind: 27235, content: ""
            ) ?? baseTags
            let serialized = try VerifiedNostrEvent.canonicalSerialization(
                pubkey: pubkey, createdAt: createdAt, kind: 27235,
                tags: tags, content: ""
            )
            let digest = Array(SHA256.hash(data: serialized))
            var message = digest
            let signature: P256K.Schnorr.SchnorrSignature
            if var randomness = auxiliaryRandomness {
                guard randomness.count == 32 else { throw NostrHTTPAuthError.signingFailed }
                signature = try privateKey.signature(
                    message: &message, auxiliaryRand: &randomness
                )
            } else {
                signature = try privateKey.signature(
                    message: &message, auxiliaryRand: nil
                )
            }
            let event = VerifiedNostrEvent(
                id: VerifiedNostrEvent.hex(digest),
                pubkey: pubkey,
                createdAt: createdAt,
                kind: 27235,
                tags: tags,
                content: "",
                sig: VerifiedNostrEvent.hex(signature.dataRepresentation)
            )
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.withoutEscapingSlashes]
            return "Nostr " + (try encoder.encode(event)).base64EncodedString()
        } catch let error as NostrHTTPAuthError {
            throw error
        } catch {
            throw NostrHTTPAuthError.signingFailed
        }
    }
}
