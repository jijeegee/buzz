import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';

/// One option of [showGroupChoiceSheet].
@immutable
class GroupChoice<T> {
  const GroupChoice({required this.value, required this.label, this.key});

  final T value;
  final String label;
  final Key? key;
}

/// A short single-choice sheet; returns the picked value, or null.
Future<T?> showGroupChoiceSheet<T>({
  required BuildContext context,
  required String title,
  required T selected,
  required List<GroupChoice<T>> options,
}) => showKuzzModalBottomSheet<T>(
  context: context,
  title: title,
  showDragHandle: true,
  builder: (context) => SafeArea(
    top: false,
    child: Padding(
      padding: const EdgeInsets.only(bottom: Grid.xs),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final option in options)
            ListTile(
              key: option.key,
              contentPadding: const EdgeInsets.symmetric(
                horizontal: Grid.gutter,
              ),
              title: Text(option.label),
              trailing: option.value == selected
                  ? Icon(LucideIcons.check, color: context.colors.primary)
                  : null,
              onTap: () => Navigator.of(context).pop(option.value),
            ),
        ],
      ),
    ),
  ),
);
