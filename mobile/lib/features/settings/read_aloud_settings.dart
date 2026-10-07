import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_aloud/read_aloud_preferences.dart';
import '../../shared/theme/grid.dart';
import '../../shared/widgets/app_list_card.dart';
import 'read_aloud_engine_settings.dart';

/// Mobile-only display settings. The preference defaults to disabled.
class ReadAloudSettings extends HookConsumerWidget {
  const ReadAloudSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final enabled = ref.watch(readAloudEnabledProvider);
    final saving = useState(false);
    return AppListCard(
      label: '화면 설정',
      verticalPadding: Grid.twelve,
      children: [
        SwitchListTile.adaptive(
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
        ),
        if (enabled && Platform.isAndroid) const ReadAloudEngineSettings(),
      ],
    );
  }
}
