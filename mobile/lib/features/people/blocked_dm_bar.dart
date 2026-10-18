import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/theme/theme.dart';
import 'person_actions.dart';
import 'person_row.dart';

/// Stands in for the composer in a one-to-one DM with someone I blocked:
/// "You blocked Ann." (their name) and an Unblock button. Unblocking takes
/// the same path as Settings > Blocked users; once the list no longer has
/// them the page shows the composer again.
class BlockedDmBar extends HookConsumerWidget {
  const BlockedDmBar({super.key, required this.pubkey});

  /// The other person in the DM.
  final String pubkey;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final normalized = pubkey.toLowerCase();
    final name = personActionName(
      personName(watchPersonProfile(ref, normalized)),
    );
    final busy = useState(false);
    return Container(
      key: const Key('blocked-dm-bar'),
      width: double.infinity,
      padding: EdgeInsets.only(
        left: Grid.gutter,
        right: Grid.xs,
        top: Grid.xxs,
        bottom: MediaQuery.viewPaddingOf(context).bottom + Grid.xxs,
      ),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: context.colors.outlineVariant)),
        color: context.colors.surface,
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              'You blocked $name.',
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ),
          TextButton(
            key: const Key('blocked-dm-unblock'),
            onPressed: busy.value
                ? null
                : () async {
                    busy.value = true;
                    await unblockWithFeedback(
                      context,
                      ref,
                      pubkey: normalized,
                      name: name,
                    );
                    if (context.mounted) busy.value = false;
                  },
            child: const Text('Unblock'),
          ),
        ],
      ),
    );
  }
}
