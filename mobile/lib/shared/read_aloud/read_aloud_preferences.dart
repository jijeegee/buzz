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
