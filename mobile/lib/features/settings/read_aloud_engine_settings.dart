import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
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
    final l10n = context.l10n;
    return ListTile(
      key: const ValueKey('read-aloud-engine'),
      title: Text(l10n.readAloudEngineTitle),
      subtitle: Text(
        l10n.readAloudEngineSubtitle(speechEngineLabel(l10n, selected)),
      ),
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
                      : l10n.readAloudEngineListFailed;
                }
                if (!context.mounted) return;
                loading.value = false;
                final choice = await showDialog<String>(
                  context: context,
                  builder: (context) => SimpleDialog(
                    title: Text(context.l10n.readAloudEngineTitle),
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
                              Expanded(
                                child: Text(
                                  speechEngineLabel(context.l10n, engine),
                                ),
                              ),
                              if (engine == selected)
                                Icon(
                                  Icons.check,
                                  semanticLabel:
                                      context.l10n.readAloudEngineSelected,
                                ),
                            ],
                          ),
                        ),
                      if (discoveryError == null &&
                          selected.isNotEmpty &&
                          !engines.contains(selected))
                        Padding(
                          padding: const EdgeInsets.all(Grid.xs),
                          child: Text(context.l10n.readAloudEngineGone),
                        ),
                      if (discoveryError == null && engines.isEmpty)
                        Padding(
                          padding: const EdgeInsets.all(Grid.xs),
                          child: Text(context.l10n.readAloudEngineNoneFound),
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
                    SnackBar(
                      content: Text(context.l10n.readAloudEngineSaveFailed),
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
