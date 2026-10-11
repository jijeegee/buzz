import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../channels/channel_detail_page.dart';
import '../channels/channels_provider.dart';

/// Leave the goals page and open [threadRootId] in [channelId]. From a
/// thread ([replaceCurrent]) the new thread takes the old one's place, so
/// Back still returns to the channel instead of stacking threads.
void openGoalThread(
  BuildContext context,
  WidgetRef ref, {
  required String channelId,
  required String threadRootId,
  required bool replaceCurrent,
}) {
  final channel = ref
      .read(channelsProvider)
      .asData
      ?.value
      .where((c) => c.id == channelId)
      .firstOrNull;
  if (channel == null) {
    ScaffoldMessenger.maybeOf(
      context,
    )?.showSnackBar(const SnackBar(content: Text('Could not open the thread')));
    return;
  }
  final navigator = Navigator.of(context);
  navigator.pop();
  final route = MaterialPageRoute<void>(
    builder: (_) => ChannelDetailPage(
      channel: channel,
      initialThreadRootId: threadRootId,
      initialThreadRouteBehavior:
          InitialThreadRouteBehavior.replaceCurrentRoute,
    ),
  );
  if (replaceCurrent) {
    navigator.pushReplacement(route);
  } else {
    navigator.push(route);
  }
}
