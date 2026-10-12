import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:uuid/uuid.dart';

/// This app install's stable device id, sent with every sign-in so the relay
/// reuses the same device (its name, robot and agent records) when this
/// install signs in again instead of listing a new one.
///
/// A random UUID in the platform keystore, kept on this phone only (never
/// synced or restored elsewhere, where it would merge two phones into one
/// device). No hardware identifier is read.
class InstallIdStore {
  InstallIdStore({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage();

  static const key = 'buzz.device.install-id.v1';

  static const _iOptions = IOSOptions(
    accessibility: KeychainAccessibility.first_unlock_this_device,
  );

  final FlutterSecureStorage _storage;

  /// The stored id, created on first use; `null` when the keystore fails
  /// (sign-in then proceeds without one, as a new device).
  Future<String?> readOrCreate() async {
    try {
      final existing = await _storage.read(key: key, iOptions: _iOptions);
      if (existing != null && Uuid.isValidUUID(fromString: existing)) {
        return existing;
      }
      final created = const Uuid().v4();
      await _storage.write(key: key, value: created, iOptions: _iOptions);
      return created;
    } on Object catch (error) {
      debugPrint('Could not read or create the install id: $error');
      return null;
    }
  }
}
