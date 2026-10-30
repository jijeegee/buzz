import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'channel_management_provider.dart';
import 'channels_provider.dart';

/// Holds the composer of a room opened from its Chats row or the saved list
/// until the relay has the room (listed in the relay's channel list, or its
/// details loaded): shown disabled, with no loading text, as desktop leaves
/// it blank. Reading the room never waits for this.
class HeldComposer extends ConsumerWidget {
  const HeldComposer({super.key, required this.channelId, required this.child});

  final String channelId;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (!composerHeld(ref, channelId)) return child;
    return IgnorePointer(
      key: const Key('held-composer'),
      child: ExcludeFocus(child: Opacity(opacity: 0.4, child: child)),
    );
  }
}

/// Whether the relay has not confirmed room [channelId] yet.
bool composerHeld(WidgetRef ref, String channelId) {
  final channels = ref.watch(channelsProvider).value;
  final notifier = ref.read(channelsProvider.notifier);
  if (notifier.hasLoaded &&
      !notifier.showsSavedList &&
      (channels?.any((channel) => channel.id == channelId) ?? false)) {
    return false;
  }
  return !ref.watch(channelDetailsProvider(channelId)).hasValue;
}
