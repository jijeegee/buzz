import Foundation

/// This install's device key as an event signer (device-bound keys plan
/// §2.1), the Swift counterpart of `signDeviceTaggedEvent` in
/// `mobile/lib/shared/device_key/device_proof.dart`.
///
/// An auth event gets a `["device", <compressed SEC1 pubkey hex>, <sig>]`
/// tag. The device signs `kuzz-device-event/v1\n` followed by the event's
/// NIP-01 id serialization with that tag's signature field blanked, then the
/// signature (raw `r || s`, ECDSA P-256 SHA-256) fills the field. The event
/// id and Schnorr signature then cover the proof. Byte-identical to the
/// relay's `kuzz-auth/src/device_proof.rs` (`device_proof_vectors.json`).
///
/// The key itself stays outside this package: the caller passes the public
/// key and a signing closure (the notification extension uses the shared
/// keychain key `kuzz.device-key.v1`).
public struct DeviceEventSigning {
    public static let eventDomain = "kuzz-device-event/v1\n"
    public static let tagName = "device"

    /// Compressed SEC1 public key, lower-case hex (33 bytes).
    public let publicKeyHex: String
    private let signRaw: (Data) -> Data?

    /// - Parameters:
    ///   - publicKeySEC1: the device public key, uncompressed (65 bytes,
    ///     what `SecKeyCopyExternalRepresentation` returns) or compressed.
    ///   - signDER: signs the message bytes with ECDSA P-256 SHA-256 and
    ///     returns the DER signature (`ecdsaSignatureMessageX962SHA256`), or
    ///     nil on failure.
    public init?(publicKeySEC1: Data, signDER: @escaping (Data) -> Data?) {
        guard let compressed = Self.compressedSEC1(publicKeySEC1) else { return nil }
        publicKeyHex = VerifiedNostrEvent.hex(compressed)
        signRaw = { message in signDER(message).flatMap(Self.rawSignature(fromDER:)) }
    }

    /// - Parameter signRaw: returns the raw 64-byte `r || s` signature.
    public init?(publicKeySEC1: Data, signRaw: @escaping (Data) -> Data?) {
        guard let compressed = Self.compressedSEC1(publicKeySEC1) else { return nil }
        publicKeyHex = VerifiedNostrEvent.hex(compressed)
        self.signRaw = { message in
            guard let raw = signRaw(message), raw.count == 64 else { return nil }
            return raw
        }
    }

    /// `tags` plus a signed device tag, or nil when the device signature
    /// fails (the caller then signs without the tag, as today).
    public func tagged(
        tags: [[String]], pubkey: String, createdAt: Int, kind: Int, content: String
    ) -> [[String]]? {
        var tagged = tags.filter { $0.first != Self.tagName }
        tagged.append([Self.tagName, publicKeyHex, ""])
        let message = Self.proofMessage(
            pubkey: pubkey, createdAt: createdAt, kind: kind, tags: tagged, content: content
        )
        guard let signature = signRaw(message), signature.count == 64 else { return nil }
        tagged[tagged.count - 1][2] = VerifiedNostrEvent.hex(signature)
        return tagged
    }

    /// The exact bytes a device signs for an event: the domain, a newline,
    /// then the NIP-01 serialization with the signature field of every
    /// `device` tag blanked.
    public static func proofMessage(
        pubkey: String, createdAt: Int, kind: Int, tags: [[String]], content: String
    ) -> Data {
        let blanked = tags.map { tag -> [String] in
            guard tag.first == tagName, tag.count > 2 else { return tag }
            var copy = tag
            copy[2] = ""
            return copy
        }
        let serialized = VerifiedNostrEvent.canonicalText(
            pubkey: pubkey, createdAt: createdAt, kind: kind, tags: blanked, content: content
        )
        return Data((eventDomain + serialized).utf8)
    }

    /// Compressed SEC1 bytes of an uncompressed (`04 || X || Y`) or already
    /// compressed (`02|03 || X`) P-256 public key.
    public static func compressedSEC1(_ key: Data) -> Data? {
        let bytes = [UInt8](key)
        if bytes.count == 33, bytes[0] == 0x02 || bytes[0] == 0x03 {
            return Data(bytes)
        }
        guard bytes.count == 65, bytes[0] == 0x04 else { return nil }
        let prefix: UInt8 = (bytes[64] & 1) == 1 ? 0x03 : 0x02
        return Data([prefix] + bytes[1...32])
    }

    /// Raw 64-byte `r || s` from a DER `SEQUENCE { INTEGER r, INTEGER s }`.
    public static func rawSignature(fromDER der: Data) -> Data? {
        let bytes = [UInt8](der)
        var index = 0

        func readLength() -> Int? {
            guard index < bytes.count else { return nil }
            let first = Int(bytes[index])
            index += 1
            if first < 0x80 { return first }
            let count = first & 0x7F
            guard count >= 1, count <= 2, index + count <= bytes.count else { return nil }
            var value = 0
            for _ in 0..<count {
                value = (value << 8) | Int(bytes[index])
                index += 1
            }
            return value
        }

        func readInteger() -> [UInt8]? {
            guard index < bytes.count, bytes[index] == 0x02 else { return nil }
            index += 1
            guard let length = readLength(), length >= 1, index + length <= bytes.count
            else { return nil }
            var value = Array(bytes[index..<(index + length)])
            index += length
            while value.count > 1, value[0] == 0 { value.removeFirst() }
            guard value.count <= 32 else { return nil }
            return [UInt8](repeating: 0, count: 32 - value.count) + value
        }

        guard !bytes.isEmpty, bytes[0] == 0x30 else { return nil }
        index = 1
        guard let sequenceLength = readLength(), index + sequenceLength == bytes.count,
              let r = readInteger(), let s = readInteger(), index == bytes.count
        else { return nil }
        return Data(r + s)
    }
}
