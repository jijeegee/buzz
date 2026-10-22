import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/read_state/read_state_format.dart';
import '../../shared/read_state/read_state_provider.dart';
import '../channels/thread_follows/thread_follows_provider.dart';
import '../channels/thread_header_actions.dart';
import 'chat_list.dart';
import 'chat_list_provider.dart';
import 'thread_pins_provider.dart';

/// A Chats thread row's long-press menu, the same actions as desktop's row
/// menu: pin or unpin, mark read or unread, unfollow, close or reopen.
Future<void> showChatThreadRowMenu(
  BuildContext context,
  WidgetRef ref,
  ChatListRow row, {
  required bool pinned,
  required bool hasUnread,
}) async {
  final rootId = row.rootId;
  if (!row.isThread || rootId == null) return;
  final messenger = ScaffoldMessenger.maybeOf(context);
  final action = await showModalBottomSheet<String>(
    context: context,
    showDragHandle: true,
    builder: (sheetContext) {
      Widget item(String value, IconData icon, String label) => ListTile(
        key: ValueKey('chat-thread-menu-$value'),
        leading: Icon(icon),
        title: Text(label),
        onTap: () => Navigator.of(sheetContext).pop(value),
      );
      return SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            pinned
                ? item('unpin', LucideIcons.pinOff, 'Unpin')
                : item('pin', LucideIcons.pin, 'Pin'),
            hasUnread
                ? item('mark-read', LucideIcons.circleCheckBig, 'Mark as read')
                : item('mark-unread', LucideIcons.circleDot, 'Mark as unread'),
            item('unfollow', LucideIcons.bellOff, 'Unfollow thread'),
            row.closed
                ? item('reopen', LucideIcons.rotateCcw, 'Reopen thread')
                : item('close', LucideIcons.circleCheck, 'Close thread'),
          ],
        ),
      );
    },
  );
  if (action == null) return;
  final readState = ref.read(readStateProvider.notifier);
  switch (action) {
    case 'pin' || 'unpin':
      final done = ref
          .read(threadPinsProvider.notifier)
          .setPinned(rootId, action == 'pin');
      if (!done && action == 'pin') {
        messenger?.showSnackBar(
          const SnackBar(content: Text('You can pin up to 200 threads.')),
        );
      }
    case 'mark-read':
      readState.markContextRead(
        threadContextKey(rootId),
        row.latest?.createdAt ?? row.at,
      );
    case 'mark-unread':
      readState.markContextUnread(
        threadContextKey(rootId),
        channelId: row.channelId,
      );
    case 'unfollow':
      ref.read(threadFollowsProvider.notifier).unfollowThread(rootId);
    case 'close' || 'reopen':
      final closing = action == 'close';
      try {
        await sendThreadCloseMarker(
          ref,
          channelId: row.channelId,
          rootId: rootId,
          closing: closing,
        );
        // The relay owns the row's closed state; ask for the list again.
        unawaited(refreshChatList(ref));
      } catch (error) {
        messenger?.showSnackBar(
          SnackBar(content: Text(threadCloseFailure(closing, error))),
        );
      }
  }
}
