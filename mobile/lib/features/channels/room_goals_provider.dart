import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'channel_management_provider.dart';
import 'channels_provider.dart';

/// Whether goals are on for the room [channelId]; threads follow their room.
///
/// Reads the listed channel first and falls back to the room's own kind:39000
/// metadata when it is not listed. An unknown room counts as on, matching the
/// relay contract where only `["goals","off"]` disables goals.
final roomGoalsEnabledProvider = Provider.family<bool, String>((
  ref,
  channelId,
) {
  final listed = ref.watch(
    channelsProvider.select(
      (channels) => channels.value
          ?.where((channel) => channel.id == channelId)
          .firstOrNull
          ?.goalsEnabled,
    ),
  );
  if (listed != null) return listed;
  return ref.watch(channelDetailsProvider(channelId)).value?.goalsEnabled ??
      true;
});
