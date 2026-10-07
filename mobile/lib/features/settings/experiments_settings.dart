import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/theme/grid.dart';
import '../../shared/widgets/app_list_card.dart';
import '../goals/goals_preferences.dart';

/// Experimental features on this device. Every switch defaults to off.
class ExperimentsSettings extends HookConsumerWidget {
  const ExperimentsSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final goalsEnabled = ref.watch(goalsEnabledProvider);
    final saving = useState(false);
    return AppListCard(
      label: '실험 기능',
      verticalPadding: Grid.twelve,
      children: [
        SwitchListTile.adaptive(
          key: const ValueKey('goals-enabled'),
          title: const Text('목표 레이어'),
          subtitle: const Text('채널·DM 상단에 목표 버튼 표시'),
          value: goalsEnabled,
          onChanged: saving.value
              ? null
              : (value) async {
                  saving.value = true;
                  try {
                    await ref
                        .read(goalsEnabledProvider.notifier)
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
        ),
      ],
    );
  }
}
