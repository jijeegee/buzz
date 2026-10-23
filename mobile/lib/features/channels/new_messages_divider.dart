import 'package:flutter/material.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';

/// Marks where the unread messages start in a window opened at them.
class NewMessagesDivider extends StatelessWidget {
  const NewMessagesDivider({super.key});

  @override
  Widget build(BuildContext context) {
    final color = context.colors.primary;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Grid.xxs),
      child: Row(
        children: [
          Expanded(child: Divider(height: 1, color: color)),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: Grid.xs),
            child: Text(
              context.l10n.messageNewMessagesDivider,
              style: context.textTheme.labelSmall?.copyWith(
                color: color,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          Expanded(child: Divider(height: 1, color: color)),
        ],
      ),
    );
  }
}
