import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../channels/channel_detail_page.dart';
import '../channels/channels_provider.dart';
import 'goal_links.dart';

/// Leave the goals page and open a goal's [target] in [channelId]: a thread,
/// a reply highlighted in its thread, or a main-timeline message jumped to
/// and highlighted. From a thread ([replaceCurrent]) the destination takes
/// the thread's place, so Back still returns to the channel instead of
/// stacking threads. A main-timeline message replaces the channel's open
/// page instead, so Back leaves the channel.
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
  showGoalLinkRoute(
    navigator,
    route,
    channelRoute: target.threadRootId == null
        ? openChannelRoute(channelId)
        : null,
    replaceCurrent: replaceCurrent,
  );
}

/// Leaves the goals page on top of [navigator] for [route]. With a
/// [channelRoute] (the channel's open page) everything above it is closed and
/// [route] takes its place; otherwise [route] replaces the page under the
/// goals page ([replaceCurrent]) or goes above it.
@visibleForTesting
void showGoalLinkRoute(
  NavigatorState navigator,
  Route<void> route, {
  required ModalRoute<Object?>? channelRoute,
  required bool replaceCurrent,
}) {
  if (channelRoute != null && channelRoute.navigator == navigator) {
    navigator.popUntil((open) => open == channelRoute);
    navigator.pushReplacement(route);
    return;
  }
  navigator.pop();
  if (replaceCurrent) {
    navigator.pushReplacement(route);
  } else {
    navigator.push(route);
  }
}
