import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../channels/channels_provider.dart';
import 'blocked_users_provider.dart';

/// Whether messages from people I blocked fold in the room [channelId]:
/// everywhere but a one-to-one DM. There the server already leaves out what
/// the other person sent while blocked, and what they sent before stays
/// readable. Group DMs and channels fold; so does a room not loaded yet.
final foldsBlockedAuthorsProvider = Provider.family<bool, String>(
  (ref, channelId) => !ref.watch(
    channelsProvider.select(
      (channels) =>
          channels.value?.any(
            (channel) => channel.id == channelId && channel.isOneToOneDm,
          ) ??
          false,
    ),
  ),
);

/// Shows [child] (a message) unless [authorPubkey] is blocked. A blocked
/// author's message collapses to one line that reveals it on tap, except in
/// a one-to-one DM (see [foldsBlockedAuthorsProvider]) when [channelId] is
/// given.
class BlockedMessageGate extends HookConsumerWidget {
  const BlockedMessageGate({
    super.key,
    required this.authorPubkey,
    this.channelId,
    required this.child,
  });

  final String authorPubkey;
  final String? channelId;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pk = authorPubkey.toLowerCase();
    final blocked = ref.watch(
      blockedPubkeysProvider.select((ids) => ids.contains(pk)),
    );
    final revealed = useState(false);
    if (!blocked || revealed.value) return child;
    final room = channelId;
    if (room != null && !ref.watch(foldsBlockedAuthorsProvider(room))) {
      return child;
    }
    final color = context.colors.onSurfaceVariant;
    return Semantics(
      button: true,
      label: context.l10n.friendsBlockedMessageLabel,
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
                  context.l10n.friendsBlockedMessage,
                  style: context.textTheme.bodySmall?.copyWith(
                    color: color,
                    fontStyle: FontStyle.italic,
                  ),
                ),
              ),
              Text(
                context.l10n.friendsBlockedMessageShow,
                style: context.textTheme.labelMedium?.copyWith(color: color),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
