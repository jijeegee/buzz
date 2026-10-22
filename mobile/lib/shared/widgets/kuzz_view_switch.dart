import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme/theme.dart';

/// One choice of a [KuzzViewSwitch].
class KuzzViewOption<T> {
  const KuzzViewOption({
    required this.value,
    required this.label,
    required this.key,
  });

  final T value;
  final String label;
  final Key key;
}

/// Two or more views of one list, as Chats' "All chats | By channel": a
/// tinted track with the chosen option raised on a white pill in bold.
class KuzzViewSwitch<T> extends StatelessWidget {
  const KuzzViewSwitch({
    super.key,
    required this.options,
    required this.selected,
    required this.onChanged,
    required this.semanticsLabel,
  });

  final List<KuzzViewOption<T>> options;
  final T selected;
  final ValueChanged<T> onChanged;
  final String semanticsLabel;

  @override
  Widget build(BuildContext context) {
    Widget option(KuzzViewOption<T> option) {
      final isSelected = option.value == selected;
      return Expanded(
        child: GestureDetector(
          key: option.key,
          behavior: HitTestBehavior.opaque,
          onTap: () {
            if (!isSelected) {
              unawaited(HapticFeedback.selectionClick());
              onChanged(option.value);
            }
          },
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 160),
            margin: const EdgeInsets.all(3),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: isSelected ? context.colors.surface : Colors.transparent,
              borderRadius: BorderRadius.circular(Grid.xxs),
              boxShadow: isSelected
                  ? [
                      BoxShadow(
                        color: context.colors.shadow.withValues(alpha: 0.08),
                        blurRadius: 3,
                        offset: const Offset(0, 1),
                      ),
                    ]
                  : null,
            ),
            child: Text(
              option.label,
              style: context.textTheme.labelLarge?.copyWith(
                color: isSelected
                    ? context.colors.onSurface
                    : context.colors.onSurfaceVariant,
                fontWeight: isSelected ? FontWeight.w700 : FontWeight.w500,
              ),
            ),
          ),
        ),
      );
    }

    return Semantics(
      container: true,
      label: semanticsLabel,
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: context.colors.surfaceContainerHighest.withValues(alpha: 0.6),
          borderRadius: BorderRadius.circular(Grid.xs),
        ),
        child: Row(children: [for (final o in options) option(o)]),
      ),
    );
  }
}
