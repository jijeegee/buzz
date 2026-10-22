import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/theme/grid.dart';
import '../../shared/theme/message_typography.dart';
import '../../shared/theme/theme_extensions.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'failed_local_messages_provider.dart';
import 'send_message_provider.dart';

/// Size of the pending clock and the failed mark in an own bubble's meta.
const _localSendStatusIconSize = 12.0;

/// The meta mark for an own message the relay doesn't have yet: a muted
/// clock while it sends, a red "Not sent" once it failed. Tapping the failed
/// mark offers retry or delete.
class LocalSendStatusMark extends ConsumerWidget {
  final LocalSendStatus status;
  final String channelId;
  final String eventId;

  const LocalSendStatusMark({
    super.key,
    required this.status,
    required this.channelId,
    required this.eventId,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    switch (status) {
      case LocalSendStatus.sent:
        return const SizedBox.shrink();
      case LocalSendStatus.pending:
        return Semantics(
          container: true,
          label: 'Sending',
          child: Icon(
            Icons.schedule,
            key: ValueKey('local-send-pending-$eventId'),
            size: _localSendStatusIconSize,
            color: context.colors.onSurfaceVariant,
          ),
        );
      case LocalSendStatus.failed:
        final color = context.colors.error;
        return Semantics(
          container: true,
          button: true,
          label: 'Not sent. Tap to retry or delete',
          excludeSemantics: true,
          child: GestureDetector(
            key: ValueKey('local-send-failed-$eventId'),
            behavior: HitTestBehavior.opaque,
            onTap: () => showFailedLocalMessageSheet(
              context: context,
              ref: ref,
              channelId: channelId,
              eventId: eventId,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  Icons.error_outline,
                  size: _localSendStatusIconSize,
                  color: color,
                ),
                const SizedBox(width: Grid.quarter),
                Text(
                  'Not sent',
                  maxLines: 1,
                  style: chatTimestampTextStyle.copyWith(color: color),
                ),
              ],
            ),
          ),
        );
    }
  }
}

/// Offers "Retry" and "Delete" for a failed own message.
void showFailedLocalMessageSheet({
  required BuildContext context,
  required WidgetRef ref,
  required String channelId,
  required String eventId,
}) {
  final sendMessage = ref.read(sendMessageProvider);
  final messenger = ScaffoldMessenger.maybeOf(context);
  showKuzzModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    builder: (sheetContext) => SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          Grid.xs,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              key: const ValueKey('local-send-retry'),
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.refresh),
              title: const Text('Retry'),
              onTap: () {
                Navigator.of(sheetContext).pop();
                unawaited(
                  sendMessage
                      .retry(channelId: channelId, eventId: eventId)
                      .catchError((Object error) {
                        // Only a changed account or community gets here; a
                        // failed publish just marks the row failed again.
                        debugPrint('[LocalSendStatusMark] retry: $error');
                        messenger?.showSnackBar(
                          const SnackBar(
                            content: Text(
                              'Message not sent: the community changed',
                            ),
                          ),
                        );
                      }),
                );
              },
            ),
            ListTile(
              key: const ValueKey('local-send-delete'),
              contentPadding: EdgeInsets.zero,
              leading: Icon(
                Icons.delete_outline,
                color: sheetContext.colors.error,
              ),
              title: Text(
                'Delete',
                style: TextStyle(color: sheetContext.colors.error),
              ),
              onTap: () {
                Navigator.of(sheetContext).pop();
                sendMessage.discard(channelId: channelId, eventId: eventId);
              },
            ),
          ],
        ),
      ),
    ),
  );
}
