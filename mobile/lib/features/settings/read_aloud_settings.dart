import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/read_aloud/read_aloud_preferences.dart';
import 'read_aloud_engine_settings.dart';

/// Settings rows for read-aloud: the toggle and, when enabled on Android,
/// the engine picker. The preference defaults to disabled.
List<Widget> readAloudSettingsRows(BuildContext context, WidgetRef ref) => [
  const ReadAloudToggle(),
  if (ref.watch(readAloudEnabledProvider) && Platform.isAndroid)
    const ReadAloudEngineSettings(),
];

/// Device-local switch that shows the per-message read-aloud controls.
class ReadAloudToggle extends HookConsumerWidget {
  const ReadAloudToggle({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final enabled = ref.watch(readAloudEnabledProvider);
    final saving = useState(false);
    return SwitchListTile.adaptive(
      key: const ValueKey('read-aloud-enabled'),
      title: Text(context.l10n.settingsReadAloud),
      subtitle: Text(context.l10n.settingsReadAloudSubtitle),
      value: enabled,
      onChanged: saving.value
          ? null
          : (value) async {
              saving.value = true;
              try {
                await ref
                    .read(readAloudEnabledProvider.notifier)
                    .setEnabled(value);
              } catch (_) {
                if (context.mounted) {
                  ScaffoldMessenger.of(context).showSnackBar(
                    SnackBar(content: Text(context.l10n.settingsSaveFailed)),
                  );
                }
              } finally {
                if (context.mounted) saving.value = false;
              }
            },
    );
  }
}
