import 'package:hooks_riverpod/hooks_riverpod.dart';
import '../theme/theme_provider.dart';

/// Device-local display preference. Never published or synced to desktop.
final readAloudEnabledProvider = NotifierProvider<ReadAloudPreference, bool>(
  ReadAloudPreference.new,
);

/// Persists only successful writes; the settings UI reports storage failures.
class ReadAloudPreference extends Notifier<bool> {
  static const key = 'buzz_mobile_read_aloud_enabled';
  @override
  bool build() => ref.watch(savedPrefsProvider).getBool(key) ?? false;

  Future<void> setEnabled(bool enabled) async {
    final saved = await ref.read(savedPrefsProvider).setBool(key, enabled);
    if (!saved) throw StateError('Could not save read-aloud preference');
    if (ref.mounted) state = enabled;
  }
}

/// Android engine package for this app only; empty means the system default.
final readAloudEngineProvider =
    NotifierProvider<ReadAloudEnginePreference, String>(
      ReadAloudEnginePreference.new,
    );

/// Keeps the engine choice on this device without changing Android settings.
class ReadAloudEnginePreference extends Notifier<String> {
  static const key = 'buzz_mobile_read_aloud_engine';

  @override
  String build() => ref.watch(savedPrefsProvider).getString(key) ?? '';

  Future<void> setEngine(String engine) async {
    final saved = await ref.read(savedPrefsProvider).setString(key, engine);
    if (!saved) throw StateError('Could not save speech engine');
    if (ref.mounted) state = engine;
  }
}
