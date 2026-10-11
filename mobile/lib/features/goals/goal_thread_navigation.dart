import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../channels/channel_detail_page.dart';
import '../channels/channels_provider.dart';
import 'goal_links.dart';

/// Leave the goals page and open a goal's [target] in [channelId]: a thread,
/// a reply highlighted in its thread, or a main-timeline message jumped to
/// and highlighted. From a thread ([replaceCurrent]) the destination takes
/// the thread's place, so Back still returns to the channel instead of
/// stacking threads.
void openGoalLink(
  BuildContext context,
  WidgetRef ref, {
  required String channelId,
  required GoalLinkTarget target,
  required bool replaceCurrent,
}) {
  final channel = ref
      .read(channelsProvider)
      .asData
      ?.value
      .where((c) => c.id == channelId)
      .firstOrNull;
  if (channel == null) {
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(
      SnackBar(
        content: Text(
          target.messageId == null
              ? 'Could not open the thread'
              : 'Could not open the message',
        ),
      ),
    );
    return;
  }
  final navigator = Navigator.of(context);
  navigator.pop();
  final route = MaterialPageRoute<void>(
    builder: (_) => ChannelDetailPage(
      channel: channel,
      initialMessageId: target.messageId,
      initialThreadRootId: target.threadRootId,
      initialThreadRouteBehavior:
          InitialThreadRouteBehavior.replaceCurrentRoute,
      highlightInitialMessage: target.threadRootId == null,
    ),
  );
  if (replaceCurrent) {
    navigator.pushReplacement(route);
  } else {
    navigator.push(route);
  }
}
