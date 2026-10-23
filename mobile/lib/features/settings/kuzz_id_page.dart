import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../people/people_errors.dart';

/// A Kuzz ID: 3 to 32 lowercase letters, digits or underscores.
final kuzzIdPattern = RegExp(r'^[a-z0-9_]{3,32}$');

/// Why [value] is not a valid Kuzz ID, or null when it is.
String? kuzzIdError(String value) {
  if (value.length < 3 || value.length > 32) {
    return appL10n.settingsKuzzIdLengthError;
  }
  if (!kuzzIdPattern.hasMatch(value)) {
    return appL10n.settingsKuzzIdCharactersError;
  }
  return null;
}

/// Settings → Kuzz ID: the handle friends type to find me.
class KuzzIdPage extends HookConsumerWidget {
  const KuzzIdPage({super.key, required this.origin});

  final String origin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final current = ref.watch(accountProfileProvider(origin)).value?.username;
    final controller = useTextEditingController(text: current ?? '');
    final saving = useState(false);
    final error = useState<String?>(null);
    // The ID can arrive after the page opens; fill it in unless edited.
    useEffect(() {
      if (current != null && controller.text.isEmpty) {
        controller.text = current;
      }
      return null;
    }, [current]);

    Future<void> save() async {
      final value = controller.text.trim().replaceFirst(RegExp('^@'), '');
      final invalid = kuzzIdError(value);
      if (invalid != null) {
        error.value = invalid;
        return;
      }
      if (value == current) {
        Navigator.of(context).pop();
        return;
      }
      error.value = null;
      saving.value = true;
      final container = ProviderScope.containerOf(context, listen: false);
      final api = ref.read(accountApiProvider(origin));
      try {
        await api.setUsername(value);
      } catch (failure) {
        if (!context.mounted) return;
        saving.value = false;
        error.value = friendlyError(
          failure,
          fallback: context.l10n.settingsKuzzIdSaveFailed,
        );
        return;
      }
      container.invalidate(accountProfileProvider(origin));
      if (!context.mounted) return;
      ScaffoldMessenger.maybeOf(context)?.showSnackBar(
        SnackBar(content: Text(context.l10n.settingsKuzzIdSaved)),
      );
      Navigator.of(context).pop();
    }

    return Scaffold(
      appBar: AppBar(title: Text(context.l10n.settingsKuzzIdTitle)),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            Text(
              context.l10n.settingsKuzzIdExplanation,
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: Grid.xs),
            TextField(
              key: const Key('kuzz-id-field'),
              controller: controller,
              enabled: !saving.value,
              autofocus: current == null,
              autocorrect: false,
              enableSuggestions: false,
              maxLength: 32,
              textInputAction: TextInputAction.done,
              onChanged: (_) {
                if (error.value != null) error.value = null;
              },
              onSubmitted: (_) => unawaited(save()),
              decoration: InputDecoration(
                labelText: context.l10n.settingsKuzzIdTitle,
                prefixText: '@',
                helperText: context.l10n.settingsKuzzIdHelper,
                errorText: error.value,
              ),
            ),
            const SizedBox(height: Grid.sm),
            FilledButton(
              key: const Key('kuzz-id-save'),
              onPressed: saving.value ? null : () => unawaited(save()),
              child: saving.value
                  ? KuzzLoadingIndicator(
                      size: 20,
                      semanticLabel: context.l10n.commonSaving,
                    )
                  : Text(context.l10n.commonSave),
            ),
          ],
        ),
      ),
    );
  }
}
