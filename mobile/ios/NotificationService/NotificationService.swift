import KuzzPushKit
import Foundation
import Intents
import Security
import UserNotifications

final class NotificationService: UNNotificationServiceExtension {
  /// Shown when the message can't be loaded. Replaces the gateway's fixed
  /// English text, which talks about relays.
  private static let genericBody = NSLocalizedString(
    "New message",
    comment: "Notification text when a new message arrived but could not be loaded."
  )

  private var handoff: KuzzNotificationHandoff<UNNotificationContent>?
  private var bestAttemptContent: UNMutableNotificationContent?
  private var restrictedFallbackContent: UNMutableNotificationContent?
  private lazy var interactionCleanup = KuzzInteractionCleanupRetry(
    containerURL: ageRestrictionContainerURL, deletion: interactionDeletionDeadline)
  private lazy var communicationPresenter = KuzzCommunicationNotificationPresenter(
    donate: { interaction, completion in interaction.donate(completion: completion) },
    deleteAllInteractions: { [cleanup = interactionCleanup] completion in
      cleanup.request(completion: completion)
    },
    updateContent: { content, intent in try content.updating(from: intent) }
  )
  private let interactionDeletionDeadline = KuzzInteractionDeletionDeadline(
    timeout: 5,
    deleteAllInteractions: { completion in
      INInteraction.deleteAll(completion: completion)
    },
    scheduleTimeout: { delay, action in
      DispatchQueue.global(qos: .utility).asyncAfter(
        deadline: .now() + delay,
        execute: action
      )
    }
  )
  private lazy var appGroupIdentifier =
    Bundle.main.object(
      forInfoDictionaryKey: "KuzzAppGroupIdentifier"
    ) as? String
  private lazy var resolver: KuzzPushNotificationResolving = {
    let keychainAccessGroup =
      Bundle.main.object(
        forInfoDictionaryKey: "KuzzKeychainAccessGroup"
      ) as? String
    // Read once per run, off the request path. Without it requests are
    // signed as before, with no device tag.
    let deviceSigner = Self.loadDeviceSigner(keychainAccessGroup: keychainAccessGroup)
    // Events fetched for communities with an inbox token are kept here for
    // the app (docs/message-sync.md#push-inbox).
    let container = ageRestrictionContainerURL
    return KuzzPushNotificationResolver(
      session: .shared,
      loadCommunitiesData: { [self] in
        Self.loadPushSnapshotData(appGroupIdentifier: self.appGroupIdentifier)
      },
      loadPrivateKey: { communityID in
        Self.loadPrivateKey(
          communityID: communityID,
          keychainAccessGroup: keychainAccessGroup
        )
      },
      loadPresentationCacheData: { [self] in
        Self.loadPushSnapshotData(appGroupIdentifier: self.appGroupIdentifier)
      },
      deviceSigner: { deviceSigner },
      pushInbox: container.map { PushInbox(containerURL: $0) },
      isAgeRestricted: {
        KuzzAgeRestrictionSession.isRestricted(containerURL: container)
      },
      titlesByRoom: true
    )
  }()

  override func didReceive(
    _ request: UNNotificationRequest,
    withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
  ) {
    handoff = KuzzNotificationHandoff(handler: contentHandler)
    restrictedFallbackContent = Self.restrictedFallback(from: request.content)
    guard let content = request.content.mutableCopy() as? UNMutableNotificationContent else {
      finish(request.content)
      return
    }
    content.body = Self.genericBody
    bestAttemptContent = content
    var cleanUserInfo = content.userInfo
    cleanUserInfo.removeValue(forKey: KuzzPushNavigationTarget.userInfoKey)
    cleanUserInfo.removeValue(forKey: KuzzDeliveredMessageNotification.createdAtUserInfoKey)
    content.userInfo = cleanUserInfo

    resolver.resolve { [weak self] resolution in
      guard let self else { return }
      if let resolution {
        content.title = resolution.title
        content.body = resolution.body
        if let subtitle = resolution.subtitle {
          content.subtitle = subtitle
        }
        if let threadIdentifier = resolution.threadIdentifier {
          content.threadIdentifier = threadIdentifier
        }
        if let navigationTarget = resolution.navigationTarget {
          var userInfo = content.userInfo
          userInfo[KuzzPushNavigationTarget.userInfoKey] = navigationTarget.userInfoValue
          if let createdAt = resolution.eventCreatedAt {
            userInfo[KuzzDeliveredMessageNotification.createdAtUserInfoKey] = createdAt
          }
          content.userInfo = userInfo
        }
        self.bestAttemptContent = content
        self.communicationPresenter.present(
          ordinaryContent: content,
          resolution: resolution,
          isStillAllowed: { [weak self] in
            !(self?.isAgeRestricted() ?? false)
          },
          onDeletionFailure: { error in
            NSLog("Notification cleanup retry failed: %@", String(describing: error))
          }
        ) { [weak self] specializedContent in
          self?.finish(specializedContent)
        }
        return
      }
      self.finish(content)
    }
  }

  override func serviceExtensionTimeWillExpire() {
    if let bestAttemptContent {
      finish(bestAttemptContent)
    }
  }

  private func finish(_ content: UNNotificationContent) {
    handoff?.finish(
      content,
      restrictedFallback: restrictedFallbackContent ?? Self.restrictedFallback(from: content),
      handoffIfAllowed: { deliver in
        KuzzAgeRestrictionSession.handoffIfAllowed(
          containerURL: ageRestrictionContainerURL, deliver: deliver)
      }
    ) { [self] in
      // The service deadline cannot wait for Intents cleanup. The safe content
      // has already been handed back synchronously, including on expiration.
      let center = UNUserNotificationCenter.current()
      center.removeAllDeliveredNotifications()
      center.removeAllPendingNotificationRequests()
      interactionCleanup.request { error in
        if error != nil { NSLog("Notification cleanup failed; age access is unchanged.") }
        center.removeAllDeliveredNotifications()
        center.removeAllPendingNotificationRequests()
      }
    }
  }

  private var ageRestrictionContainerURL: URL? {
    appGroupIdentifier.flatMap {
      FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: $0)
    }
  }

  private func isAgeRestricted() -> Bool {
    KuzzAgeRestrictionSession.isRestricted(containerURL: ageRestrictionContainerURL)
  }

  private static func restrictedFallback(
    from content: UNNotificationContent
  ) -> UNMutableNotificationContent {
    let fallback =
      (content.mutableCopy() as? UNMutableNotificationContent)
      ?? UNMutableNotificationContent()
    fallback.title = "Kuzz"
    fallback.subtitle = ""
    fallback.body = "Open Kuzz to view this message."
    fallback.threadIdentifier = ""
    var userInfo = fallback.userInfo
    userInfo.removeValue(forKey: KuzzPushNavigationTarget.userInfoKey)
    userInfo.removeValue(forKey: KuzzDeliveredMessageNotification.createdAtUserInfoKey)
    fallback.userInfo = userInfo
    return fallback
  }

  private static func loadPrivateKey(
    communityID: String,
    keychainAccessGroup: String?
  ) -> String? {
    var query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "kuzz.push.nse.signing",
      kSecAttrAccount as String: communityID,
      kSecReturnData as String: true,
      kSecMatchLimit as String: kSecMatchLimitOne,
    ]
    if let keychainAccessGroup, !keychainAccessGroup.isEmpty {
      query[kSecAttrAccessGroup as String] = keychainAccessGroup
    }
    var item: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
      let data = item as? Data
    else { return nil }
    return String(data: data, encoding: .utf8)
  }

  /// This install's device key (`DeviceKeyPlugin`, tag `kuzz.device-key.v1`),
  /// read from the keychain group the app and this extension share. Never
  /// created here: if the extension can't read the key, nil.
  private static func loadDeviceSigner(keychainAccessGroup: String?) -> DeviceEventSigning? {
    var query: [String: Any] = [
      kSecClass as String: kSecClassKey,
      kSecAttrApplicationTag as String: Data("kuzz.device-key.v1".utf8),
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecAttrKeyClass as String: kSecAttrKeyClassPrivate,
      kSecReturnRef as String: true,
    ]
    if let keychainAccessGroup, !keychainAccessGroup.isEmpty {
      query[kSecAttrAccessGroup as String] = keychainAccessGroup
    }
    var item: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
      let item, CFGetTypeID(item) == SecKeyGetTypeID()
    else { return nil }
    let key = item as! SecKey
    guard let publicKey = SecKeyCopyPublicKey(key),
      let raw = SecKeyCopyExternalRepresentation(publicKey, nil) as Data?
    else { return nil }
    return DeviceEventSigning(
      publicKeySEC1: raw,
      signDER: { message in
        var error: Unmanaged<CFError>?
        let signature = SecKeyCreateSignature(
          key, .ecdsaSignatureMessageX962SHA256, message as CFData, &error)
        return signature as Data?
      })
  }

  private static func loadPushSnapshotData(appGroupIdentifier: String?) -> Data? {
    loadAppGroupData(
      fileName: KuzzPushPresentationCacheStore.fileName,
      appGroupIdentifier: appGroupIdentifier,
      maximumBytes: KuzzPushPresentationCacheStore.maximumSnapshotBytes
    )
  }

  private static func loadAppGroupData(
    fileName: String,
    appGroupIdentifier: String?,
    maximumBytes: Int? = nil
  ) -> Data? {
    guard let appGroupIdentifier,
      let container = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: appGroupIdentifier
      )
    else { return nil }
    let fileURL = container.appendingPathComponent(fileName)
    if let maximumBytes {
      guard let values = try? fileURL.resourceValues(forKeys: [.fileSizeKey]),
        let fileSize = values.fileSize,
        fileSize <= maximumBytes
      else { return nil }
    }
    return try? Data(contentsOf: fileURL)
  }
}
