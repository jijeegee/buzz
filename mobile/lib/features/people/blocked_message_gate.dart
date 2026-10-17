import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'blocked_users_provider.dart';

/// Shows [child] (a message) unless [authorPubkey] is blocked. A blocked
/// author's message collapses to one line that reveals it on tap.
class BlockedMessageGate extends HookConsumerWidget {
  const BlockedMessageGate({
    super.key,
    required this.authorPubkey,
    required this.child,
  });

  final String authorPubkey;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pk = authorPubkey.toLowerCase();
    final blocked = ref.watch(
      blockedPubkeysProvider.select((ids) => ids.contains(pk)),
    );
    final revealed = useState(false);
    if (!blocked || revealed.value) return child;
    final color = context.colors.onSurfaceVariant;
    return Semantics(
      button: true,
      label: 'Message from a blocked user. Double tap to show.',
      excludeSemantics: true,
      child: InkWell(
        key: const Key('blocked-message'),
        onTap: () => revealed.value = true,
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: Grid.gutter,
            vertical: Grid.xxs,
          ),
          child: Row(
            children: [
              Icon(LucideIcons.eyeOff, size: 16, color: color),
              const SizedBox(width: Grid.xxs),
              Expanded(
                child: Text(
                  'Message from a blocked user',
                  style: context.textTheme.bodySmall?.copyWith(
                    color: color,
                    fontStyle: FontStyle.italic,
                  ),
                ),
              ),
              Text(
                'Show',
                style: context.textTheme.labelMedium?.copyWith(color: color),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
