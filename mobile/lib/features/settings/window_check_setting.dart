import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/relay/window_check.dart';
import '../../shared/theme/theme_provider.dart';
import '../../shared/widgets/app_list.dart';

/// Whether builds show the window check control (developer builds for field
/// tests); release builds leave Settings unchanged.
const windowCheckControlShown = bool.fromEnvironment(
  'KUZZ_WINDOW_CHECK_CONTROL',
);

/// Developer override of the relay's window check mode (device-local). The
/// relay session reads the saved value at each window open.
final windowCheckOverrideProvider =
    NotifierProvider<WindowCheckOverridePreference, WindowCheckOverride>(
      WindowCheckOverridePreference.new,
    );

class WindowCheckOverridePreference extends Notifier<WindowCheckOverride> {
  @override
  WindowCheckOverride build() => windowCheckOverrideOf(
    ref.watch(savedPrefsProvider).getString(windowCheckOverrideKey),
  );

  Future<void> set(WindowCheckOverride value) async {
    final saved = await ref
        .read(savedPrefsProvider)
        .setString(windowCheckOverrideKey, value.name);
    if (!saved) throw StateError('Could not save window check override');
    if (ref.mounted) state = value;
  }
}

String _label(WindowCheckOverride value) => switch (value) {
  WindowCheckOverride.follow => 'Follow server',
  WindowCheckOverride.off => 'Off',
  WindowCheckOverride.shadow => 'Shadow',
  WindowCheckOverride.on => 'On',
};

/// Settings → Experiments row for the window check override.
List<Widget> windowCheckExperimentRows(BuildContext context, WidgetRef ref) {
  final current = ref.watch(windowCheckOverrideProvider);
  return [
    AppListRow(
      key: const ValueKey('experiments-window-check'),
      icon: LucideIcons.listChecks,
      title: 'Message window check',
      value: _label(current),
      onTap: () async {
        final picked = await showDialog<WindowCheckOverride>(
          context: context,
          builder: (dialog) => SimpleDialog(
            title: const Text('Message window check'),
            children: [
              for (final value in WindowCheckOverride.values)
                SimpleDialogOption(
                  onPressed: () => Navigator.of(dialog).pop(value),
                  child: Text(_label(value)),
                ),
            ],
          ),
        );
        if (picked != null) {
          await ref.read(windowCheckOverrideProvider.notifier).set(picked);
        }
      },
    ),
  ];
}
