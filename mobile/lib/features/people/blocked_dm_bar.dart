import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../channels/channel.dart';
import 'person_actions.dart';
import 'person_row.dart';

/// The other person in [channel] when it is a one-to-one DM and I blocked
/// them; null otherwise. Such a DM shows [BlockedDmBar] instead of its
/// composers and leaves out that person's typing.
String? blockedDmCounterpart(
  Channel? channel, {
  required String? currentPubkey,
  required Set<String> blocked,
}) {
  if (channel == null || !channel.isOneToOneDm) return null;
  final me = currentPubkey?.toLowerCase();
  for (final pubkey in channel.participantPubkeys) {
    final normalized = pubkey.trim().toLowerCase();
    if (normalized.isEmpty || normalized == me) continue;
    if (blocked.contains(normalized)) return normalized;
  }
  return null;
}

/// Stands in for the composer (room or thread) in a one-to-one DM with
/// someone I blocked:
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
              context.l10n.friendsYouBlocked(name),
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
            child: Text(context.l10n.friendsUnblock),
          ),
        ],
      ),
    );
  }
}
