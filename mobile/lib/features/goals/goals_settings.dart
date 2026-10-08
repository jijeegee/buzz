import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'goals_preferences.dart';

/// Rows the goal layers experiment contributes to Settings → Experiments.
List<Widget> goalsExperimentRows(BuildContext context, WidgetRef ref) => const [
  GoalsToggle(),
];

/// Device-local switch that shows the goals button on channels and DMs.
class GoalsToggle extends HookConsumerWidget {
  const GoalsToggle({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final enabled = ref.watch(goalsEnabledProvider);
    final saving = useState(false);
    return SwitchListTile.adaptive(
      key: const ValueKey('goals-enabled'),
      title: const Text('목표 레이어'),
      subtitle: const Text('채널·DM 상단에 목표 버튼 표시'),
      value: enabled,
      onChanged: saving.value
          ? null
          : (value) async {
              saving.value = true;
              try {
                await ref.read(goalsEnabledProvider.notifier).setEnabled(value);
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
