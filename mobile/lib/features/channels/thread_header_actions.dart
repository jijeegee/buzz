import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/clipboard_utils.dart';
import '../../shared/deeplink/deep_link.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/read_state/read_state_format.dart';
import '../../shared/read_state/read_state_provider.dart';
import '../chats/chat_list_provider.dart';
import '../chats/chat_row_channel.dart';
import 'channel_detail_page.dart';
import 'channels_provider.dart';
import 'send_message_provider.dart';
import 'thread_follows/thread_follows_provider.dart';
import 'timeline_message.dart';

/// Close marker written by `kuzz threads close` and the desktop Close button.
const threadClosedTag = 'kuzz:thread-closed';

/// Reopen marker written by `kuzz threads reopen` and the desktop Reopen.
const threadReopenedTag = 'kuzz:thread-reopened';

/// The thread header's compact buttons: smaller icons, set closer together.
final threadHeaderButtonStyle = IconButton.styleFrom(
  iconSize: 20,
  padding: const EdgeInsets.all(6),
  minimumSize: const Size.square(34),
  tapTargetSize: MaterialTapTargetSize.shrinkWrap,
  visualDensity: VisualDensity.compact,
);

const _threadClosedContent = '✅ Thread closed.';
const _threadReopenedContent = 'Thread reopened.';

/// A thread is closed while its latest reply is a close marker; any later
/// message reopens it (same rule as desktop and the CLI).
bool isThreadClosed(Iterable<TimelineMessage> replies) {
  TimelineMessage? latest;
  for (final reply in replies) {
    if (latest == null || reply.createdAt >= latest.createdAt) latest = reply;
  }
  return latest != null &&
      latest.tags.any(
        (tag) => tag.length > 1 && tag[0] == threadClosedTag && tag[1] != '',
      );
}

/// Closes or reopens a thread with the marker reply desktop and the CLI send.
Future<void> sendThreadCloseMarker(
  WidgetRef ref, {
  required String channelId,
  required String rootId,
  required bool closing,
}) => ref
    .read(sendMessageProvider)
    .call(
      channelId: channelId,
      content: closing ? _threadClosedContent : _threadReopenedContent,
      parentEventId: rootId,
      rootEventId: rootId,
      mentionPubkeys: const [],
      extraTags: [
        [closing ? threadClosedTag : threadReopenedTag, rootId],
      ],
    );

String threadCloseFailure(bool closing, Object error) => closing
    ? appL10n.threadCloseError('$error')
    : appL10n.threadReopenError('$error');

/// The thread header's group button: back to the group this thread is in.
///
/// [channelBelow] says the group page is the route right under this thread
/// (opened from the group), so going there is a pop. Opened straight from
/// Chats, the thread replaced the group page, so the group opens in its place.
class ThreadGroupButton extends ConsumerWidget {
  const ThreadGroupButton({
    super.key,
    required this.channelId,
    required this.channelBelow,
  });

  final String channelId;
  final bool channelBelow;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return IconButton(
      key: const ValueKey('thread-open-group'),
      tooltip: context.l10n.threadOpenGroup,
      icon: const Icon(LucideIcons.messagesSquare),
      onPressed: () {
        final navigator = Navigator.of(context);
        if (channelBelow) {
          navigator.pop();
          return;
        }
        // A group the channel list does not hold yet opens from its Chats
        // row, else by id; the group page loads the rest.
        final channel =
            ref
                .read(channelsProvider)
                .value
                ?.where((channel) => channel.id == channelId)
                .firstOrNull ??
            channelFromChatRows(
              ref.read(chatListProvider).value?.rows ?? const [],
              channelId,
            ) ??
            channelFromId(channelId);
        unawaited(
          navigator.pushReplacement(
            MaterialPageRoute<void>(
              builder: (_) => ChannelDetailPage(channel: channel),
            ),
          ),
        );
      },
    );
  }
}

/// The thread header's `⋯` menu, matching the desktop thread header: rename,
/// follow, copy link, close or reopen, and mark as unread.
class ThreadMoreMenu extends ConsumerWidget {
  const ThreadMoreMenu({
    super.key,
    required this.channelId,
    required this.rootId,
    required this.closed,
    required this.canClose,
    this.onRename,
  });

  final String channelId;
  final String rootId;

  /// Whether the thread currently reads as closed.
  final bool closed;

  /// Close and Reopen are offered to members of a live top-level thread.
  final bool canClose;

  /// Renames the thread; `null` hides the item (not a member, archived or
  /// deleted).
  final VoidCallback? onRename;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final following = ref.watch(threadFollowsProvider).isFollowing(rootId);
    return PopupMenuButton<String>(
      key: const ValueKey('thread-more'),
      tooltip: context.l10n.threadMoreActions,
      icon: const Icon(LucideIcons.ellipsis),
      iconSize: 20,
      padding: const EdgeInsets.all(6),
      style: threadHeaderButtonStyle,
      onSelected: (action) async {
        switch (action) {
          case 'rename':
            onRename?.call();
          case 'follow':
            final notifier = ref.read(threadFollowsProvider.notifier);
            if (following) {
              notifier.unfollowThread(rootId);
            } else {
              notifier.followThread(rootId);
            }
          case 'copy-link':
            await copyToClipboard(
              context,
              buildMessageLink(channelId: channelId, messageId: rootId),
              message: context.l10n.threadLinkCopied,
            );
          case 'close':
            final messenger = ScaffoldMessenger.maybeOf(context);
            try {
              await sendThreadCloseMarker(
                ref,
                channelId: channelId,
                rootId: rootId,
                closing: !closed,
              );
            } catch (error) {
              messenger?.showSnackBar(
                SnackBar(content: Text(threadCloseFailure(!closed, error))),
              );
            }
          case 'mark-unread':
            ref
                .read(readStateProvider.notifier)
                .markContextUnread(
                  threadContextKey(rootId),
                  channelId: channelId,
                );
            // Staying here would read it again at once.
            if (context.mounted) Navigator.of(context).maybePop();
        }
      },
      itemBuilder: (_) => [
        if (onRename != null)
          _item('rename', LucideIcons.pencil, context.l10n.threadEditName),
        _item(
          'follow',
          following ? LucideIcons.bellOff : LucideIcons.bellRing,
          following
              ? context.l10n.messageUnfollowThread
              : context.l10n.messageFollowThread,
        ),
        _item('copy-link', LucideIcons.link2, context.l10n.messageCopyLink),
        if (canClose)
          _item(
            'close',
            closed ? LucideIcons.rotateCcw : LucideIcons.circleCheck,
            closed ? context.l10n.threadReopen : context.l10n.threadClose,
          ),
        _item(
          'mark-unread',
          LucideIcons.circleDot,
          context.l10n.threadMarkUnread,
        ),
      ],
    );
  }

  PopupMenuItem<String> _item(String value, IconData icon, String label) =>
      PopupMenuItem<String>(
        key: ValueKey('thread-more-$value'),
        value: value,
        child: Row(
          children: [
            Icon(icon, size: 18),
            const SizedBox(width: 12),
            Text(label),
          ],
        ),
      );
}
