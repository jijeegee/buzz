import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import 'thread_name_provider.dart';

Future<void> showThreadNameEditor(BuildContext context, ThreadNameKey target) =>
    showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (_) => ThreadNameEditor(target: target),
    );

class ThreadNameEditor extends HookConsumerWidget {
  final ThreadNameKey target;
  const ThreadNameEditor({super.key, required this.target});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final current = ref.watch(threadNameProvider(target));
    final storage = ref.watch(threadNameStorageProvider);
    final controller = useTextEditingController(
      text: storage.draft(target) ?? current.value?.content ?? '',
    );
    useListenable(controller);
    final saving = useState(false);
    final failure = useState<String?>(null);
    final writes = useRef<Future<void>>(Future.value());
    final error = threadNameError(controller.text.trim());

    Future<void> save() async {
      final name = controller.text;
      if (saving.value || threadNameError(name.trim()) != null) return;
      saving.value = true;
      failure.value = null;
      // A Korean keyboard can keep the final syllable composing indefinitely.
      // Explicit Save/Done accepts the visible text, including that syllable.
      FocusScope.of(context).unfocus();
      try {
        await writes.value;
        await ref.read(threadNameProvider(target).notifier).save(name);
        if (context.mounted) Navigator.of(context).pop();
      } catch (e) {
        if (context.mounted) failure.value = e.toString();
      } finally {
        if (context.mounted) saving.value = false;
      }
    }

    return PopScope(
      canPop: !saving.value,
      child: AlertDialog(
        title: Text(context.l10n.threadNameTitle),
        content: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(context.l10n.threadNameBody),
              const SizedBox(height: Grid.md),
              TextField(
                key: const ValueKey('thread-name-input'),
                controller: controller,
                autofocus: true,
                enabled: !saving.value,
                maxLines: 1,
                textInputAction: TextInputAction.done,
                decoration: InputDecoration(
                  labelText: context.l10n.threadNameLabel,
                  errorText: error,
                  counterText: context.l10n.threadNameCounter(
                    threadNameWeight(controller.text.trim()),
                  ),
                ),
                onChanged: (text) {
                  writes.value = writes.value
                      .then((_) => storage.saveDraft(target, text))
                      .catchError((Object e) {
                        if (context.mounted) failure.value = e.toString();
                      });
                },
                onSubmitted: (_) => unawaited(save()),
              ),
              if (failure.value ?? current.error?.toString()
                  case final message?) ...[
                const SizedBox(height: Grid.sm),
                Text(message, style: TextStyle(color: context.colors.error)),
              ],
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: saving.value ? null : () => Navigator.of(context).pop(),
            child: Text(context.l10n.commonClose),
          ),
          FilledButton(
            key: const ValueKey('thread-name-save'),
            onPressed: saving.value || error != null ? null : save,
            child: Text(
              saving.value
                  ? context.l10n.commonSavingEllipsis
                  : context.l10n.commonSave,
            ),
          ),
        ],
      ),
    );
  }
}
