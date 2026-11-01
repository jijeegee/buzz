import CryptoKit
import Flutter
import Foundation
import Security

/// This install's device key (device-bound keys plan §2.1): one P-256 key
/// pair in the Secure Enclave, the iOS counterpart of the Android Keystore
/// key behind the same `kuzz/device_key` channel. It is created on first
/// use, cannot be exported, and only works on this iPhone
/// (`AfterFirstUnlockThisDeviceOnly`, so background reconnects can sign).
/// Flutter only receives the public key and signatures (DER,
/// ecdsaSignatureMessageX962SHA256, as Android's SHA256withECDSA).
///
/// Where there is no Secure Enclave (the Simulator) the key is an ordinary
/// keychain key reported as `hardware: false`. An existing key is always
/// reused, whatever its kind; a Secure Enclave failure on a device that has
/// one is an error for Dart to retry, never a quiet software key.
final class DeviceKeyPlugin {
  static let channelName = "kuzz/device_key"
  /// Keychain application tag; the same name as the Android Keystore alias.
  private static let tag = Data("kuzz.device-key.v1".utf8)

  private let channel: FlutterMethodChannel
  /// One key operation at a time, off the main thread.
  private let worker = DispatchQueue(label: "app.kuzz.device-key")

  init(messenger: FlutterBinaryMessenger) {
    channel = FlutterMethodChannel(
      name: DeviceKeyPlugin.channelName,
      binaryMessenger: messenger
    )
    channel.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result: result)
    }
  }

  deinit {
    channel.setMethodCallHandler(nil)
  }

  private func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    switch call.method {
    case "getOrCreate":
      runOffMain(result) {
        let payload = try Self.publicKeyPayload()
        Self.logAccessGroupOnce()
        return payload
      }
    case "accessGroup":
      runOffMain(result) { try Self.accessGroup() }
    case "sign":
      guard
        let args = call.arguments as? [String: Any],
        let message = args["message"] as? FlutterStandardTypedData
      else {
        result(FlutterError(code: "bad_args", message: "message is required", details: nil))
        return
      }
      runOffMain(result) { try Self.sign(message.data) }
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  /// Run `work` on the worker queue and answer `result` exactly once on the
  /// main queue; any error becomes `device_key_failed`.
  private func runOffMain(_ result: @escaping FlutterResult, _ work: @escaping () throws -> Any?) {
    worker.async {
      let reply: Any?
      do {
        reply = try work()
      } catch {
        // Never include key material; the error kind is enough.
        reply = FlutterError(
          code: "device_key_failed",
          message: String(describing: type(of: error)),
          details: nil
        )
      }
      DispatchQueue.main.async { result(reply) }
    }
  }

  enum DeviceKeyError: Error {
    case status(OSStatus)
    case create(Int)
    case publicKey
    case signature(Int)
  }

  private static func publicKeyPayload() throws -> [String: Any] {
    let key = try privateKey()
    guard
      let publicKey = SecKeyCopyPublicKey(key),
      let raw = SecKeyCopyExternalRepresentation(publicKey, nil) as Data?,
      raw.count == 65
    else {
      throw DeviceKeyError.publicKey
    }
    return [
      "publicKey": FlutterStandardTypedData(bytes: raw),
      "hardware": isSecureEnclaveKey(key),
    ]
  }

  private static func sign(_ message: Data) throws -> FlutterStandardTypedData {
    let key = try privateKey()
    var error: Unmanaged<CFError>?
    guard
      let der = SecKeyCreateSignature(
        key,
        .ecdsaSignatureMessageX962SHA256,
        message as CFData,
        &error
      ) as Data?
    else {
      let code = error.map { CFErrorGetCode($0.takeRetainedValue()) } ?? 0
      throw DeviceKeyError.signature(code)
    }
    return FlutterStandardTypedData(bytes: der)
  }

  /// Read-only diagnostic: the keychain access group the existing key lives
  /// in (nil when there is no key yet). The notification extension can read
  /// the key only from the group both targets share
  /// (`KuzzKeychainAccessGroup`). Never creates or moves the key.
  private static func accessGroup() throws -> String? {
    let query: [String: Any] = [
      kSecClass as String: kSecClassKey,
      kSecAttrApplicationTag as String: tag,
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecAttrKeyClass as String: kSecAttrKeyClassPrivate,
      kSecReturnAttributes as String: true,
    ]
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    switch status {
    case errSecSuccess:
      return (item as? [String: Any])?[kSecAttrAccessGroup as String] as? String
    case errSecItemNotFound:
      return nil
    default:
      throw DeviceKeyError.status(status)
    }
  }

  private static var loggedAccessGroup = false

  /// Logs once per launch whether the key sits in the group the
  /// notification extension reads. Only the group name, never key material.
  private static func logAccessGroupOnce() {
    guard !loggedAccessGroup else { return }
    loggedAccessGroup = true
    let expected = Bundle.main.object(forInfoDictionaryKey: "KuzzKeychainAccessGroup") as? String
    let actual = (try? accessGroup()) ?? nil
    NSLog(
      "Device key access group: %@ (shared with notifications: %@)",
      actual ?? "none",
      actual != nil && actual == expected ? "yes" : "no"
    )
  }

  /// The key pair's private half, creating the pair on first use.
  private static func privateKey() throws -> SecKey {
    if let existing = try find() {
      return existing
    }
    return try generate(secureEnclave: SecureEnclave.isAvailable)
  }

  private static func find() throws -> SecKey? {
    let query: [String: Any] = [
      kSecClass as String: kSecClassKey,
      kSecAttrApplicationTag as String: tag,
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecAttrKeyClass as String: kSecAttrKeyClassPrivate,
      kSecReturnRef as String: true,
    ]
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    switch status {
    case errSecSuccess:
      // A key query with kSecReturnRef always yields a SecKey.
      return (item as! SecKey)
    case errSecItemNotFound:
      return nil
    default:
      throw DeviceKeyError.status(status)
    }
  }

  private static func generate(secureEnclave: Bool) throws -> SecKey {
    var accessError: Unmanaged<CFError>?
    guard
      let access = SecAccessControlCreateWithFlags(
        kCFAllocatorDefault,
        kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        secureEnclave ? .privateKeyUsage : [],
        &accessError
      )
    else {
      let code = accessError.map { CFErrorGetCode($0.takeRetainedValue()) } ?? 0
      throw DeviceKeyError.create(code)
    }
    var attributes: [String: Any] = [
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecAttrKeySizeInBits as String: 256,
      kSecPrivateKeyAttrs as String: [
        kSecAttrIsPermanent as String: true,
        kSecAttrApplicationTag as String: tag,
        kSecAttrAccessControl as String: access,
      ] as [String: Any],
    ]
    if secureEnclave {
      attributes[kSecAttrTokenID as String] = kSecAttrTokenIDSecureEnclave
    }
    var error: Unmanaged<CFError>?
    guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error) else {
      let code = error.map { CFErrorGetCode($0.takeRetainedValue()) } ?? 0
      throw DeviceKeyError.create(code)
    }
    return key
  }

  private static func isSecureEnclaveKey(_ key: SecKey) -> Bool {
    guard let attributes = SecKeyCopyAttributes(key) as? [String: Any] else {
      return false
    }
    return (attributes[kSecAttrTokenID as String] as? String)
      == (kSecAttrTokenIDSecureEnclave as String)
  }
}
