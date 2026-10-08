import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/theme/theme_provider.dart';

/// Device-local switch for the experimental goal layers. Off by default, so
/// channels and DMs look exactly as before until the user opts in.
final goalsEnabledProvider = NotifierProvider<GoalsPreference, bool>(
  GoalsPreference.new,
);

/// Persists only successful writes; the settings UI reports storage failures.
class GoalsPreference extends Notifier<bool> {
  static const key = 'buzz_mobile_goals_enabled';
  @override
  bool build() => ref.watch(savedPrefsProvider).getBool(key) ?? false;

  Future<void> setEnabled(bool enabled) async {
    final saved = await ref.read(savedPrefsProvider).setBool(key, enabled);
    if (!saved) throw StateError('Could not save goals preference');
    if (ref.mounted) state = enabled;
  }
}
