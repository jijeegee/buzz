import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_aloud/read_aloud_preferences.dart';
import 'read_aloud_engine_settings.dart';

/// Experiment rows for mobile read-aloud: the toggle and, when enabled on
/// Android, the engine picker. The preference defaults to disabled.
List<Widget> readAloudExperimentRows(BuildContext context, WidgetRef ref) => [
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
      title: const Text('모바일 읽어주기 버튼'),
      subtitle: const Text('메시지에 음성 재생 버튼 표시 · 무료 기기 내장 음성'),
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
                    const SnackBar(
                      content: Text('설정을 저장하지 못했습니다. 다시 시도해 주세요.'),
                    ),
                  );
                }
              } finally {
                if (context.mounted) saving.value = false;
              }
            },
    );
  }
}
