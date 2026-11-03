/// Method channel of the Android push plugin. Present only in builds made
/// with `--dart-define=KUZZ_ANDROID_PUSH=true`; otherwise every call fails
/// with `MissingPluginException`.
const kuzzAndroidPushChannel = 'kuzz/android_push';

/// Dart entry point the background wake engine runs (in `main.dart`).
const kuzzAndroidPushWakeEntrypoint = 'kuzzAndroidPushWake';
