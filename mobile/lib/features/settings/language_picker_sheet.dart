import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';

/// Each shipped language's name, written in that language.
String languageNativeName(Locale locale) =>
    lookupAppLocalizations(locale).languageNativeName;

/// Lists "Same as phone" and every shipped language; `null` means follow
/// the phone.
Future<void> showLanguagePickerSheet({
  required BuildContext context,
  required Locale? selected,
  required Locale deviceLocale,
  required ValueChanged<Locale?> onSelected,
}) {
  final l10n = context.l10n;
  final options = <Locale?>[null, ...AppLocalizations.supportedLocales];
  return showKuzzModalBottomSheet<void>(
    context: context,
    title: l10n.settingsLanguage,
    showDragHandle: true,
    isScrollControlled: true,
    builder: (sheetContext) => SafeArea(
      top: false,
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxHeight: MediaQuery.sizeOf(sheetContext).height * 0.7,
        ),
        child: ListView(
          shrinkWrap: true,
          children: [
            for (final option in options)
              ListTile(
                key: ValueKey(
                  'language-option-${option?.toLanguageTag() ?? 'system'}',
                ),
                title: Text(
                  option == null
                      ? l10n.settingsLanguageDevice(
                          languageNativeName(deviceLocale),
                        )
                      : languageNativeName(option),
                ),
                trailing: option == selected
                    ? Icon(
                        LucideIcons.check,
                        color: Theme.of(sheetContext).colorScheme.primary,
                      )
                    : null,
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: Grid.gutter,
                ),
                onTap: () {
                  Navigator.of(sheetContext).pop();
                  onSelected(option);
                },
              ),
          ],
        ),
      ),
    ),
  );
}
