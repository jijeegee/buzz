/// Android Keystore keys of the Kuzz app. The Dart side lives in the app
/// (`lib/shared/device_key/device_key.dart`); this package only registers
/// the native `kuzz/device_key` channel in every Flutter engine.
library;

/// Platform channel of this install's device key.
const kuzzDeviceKeyChannel = 'kuzz/device_key';
