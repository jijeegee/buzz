import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_aloud/read_aloud_preferences.dart';
import '../../shared/read_aloud/speech_engine.dart';
import '../../shared/read_aloud/system_speech_engine.dart';
import '../../shared/theme/grid.dart';

/// Selects an installed Android TTS engine without changing the system default.
class ReadAloudEngineSettings extends HookConsumerWidget {
  const ReadAloudEngineSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final selected = ref.watch(readAloudEngineProvider);
    final busy = useState(false);
    final loading = useState(false);
    return ListTile(
      key: const ValueKey('read-aloud-engine'),
      title: const Text('읽어주기 음성 엔진'),
      subtitle: Text('${speechEngineLabel(selected)}\n이 앱에만 적용 · 변경하면 재생 중지'),
      trailing: loading.value
          ? const SizedBox.square(
              dimension: Grid.sm,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          : const Icon(Icons.chevron_right),
      onTap: busy.value
          ? null
          : () async {
              busy.value = true;
              loading.value = true;
              try {
                var engines = <String>[];
                String? discoveryError;
                try {
                  engines = await ref
                      .read(systemSpeechEngineProvider)
                      .installedEngines();
                } catch (error) {
                  discoveryError = error is SpeechFailure
                      ? error.message
                      : '설치된 엔진 목록을 불러오지 못했습니다. 기본 설정으로 되돌리거나 이 창을 다시 열어 주세요.';
                }
                if (!context.mounted) return;
                loading.value = false;
                final choice = await showDialog<String>(
                  context: context,
                  builder: (context) => SimpleDialog(
                    title: const Text('읽어주기 음성 엔진'),
                    children: [
                      if (discoveryError != null)
                        Padding(
                          padding: const EdgeInsets.all(Grid.xs),
                          child: Text(discoveryError),
                        ),
                      for (final engine in ['', ...engines])
                        SimpleDialogOption(
                          onPressed: () => Navigator.pop(context, engine),
                          child: Row(
                            children: [
                              Expanded(child: Text(speechEngineLabel(engine))),
                              if (engine == selected)
                                const Icon(Icons.check, semanticLabel: '선택됨'),
                            ],
                          ),
                        ),
                      if (discoveryError == null &&
                          selected.isNotEmpty &&
                          !engines.contains(selected))
                        const Padding(
                          padding: EdgeInsets.all(Grid.xs),
                          child: Text('이전에 선택한 엔진이 없습니다. 다른 엔진을 선택해 주세요.'),
                        ),
                      if (discoveryError == null && engines.isEmpty)
                        const Padding(
                          padding: EdgeInsets.all(Grid.xs),
                          child: Text(
                            '설치된 엔진을 찾지 못했습니다. 음성 엔진을 설치한 뒤 다시 열어 주세요.',
                          ),
                        ),
                    ],
                  ),
                );
                if (!context.mounted || choice == null || choice == selected) {
                  return;
                }
                loading.value = true;
                await ref
                    .read(readAloudEngineProvider.notifier)
                    .setEngine(choice);
              } catch (_) {
                if (context.mounted) {
                  ScaffoldMessenger.of(context).showSnackBar(
                    const SnackBar(
                      content: Text('음성 엔진 설정을 완료하지 못했습니다. 다시 눌러 시도해 주세요.'),
                    ),
                  );
                }
              } finally {
                if (context.mounted) {
                  loading.value = false;
                  busy.value = false;
                }
              }
            },
    );
  }
}
