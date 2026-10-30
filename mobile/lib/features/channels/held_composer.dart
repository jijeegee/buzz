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
    final held = composerHeld(ref, channelId);
    // The same wrappers either way, so lifting the hold keeps the composer's
    // state (draft, shared text).
    return IgnorePointer(
      key: held ? const Key('held-composer') : null,
      ignoring: held,
      child: ExcludeFocus(
        excluding: held,
        child: Opacity(opacity: held ? 0.4 : 1, child: child),
      ),
    );
  }
}

/// Whether the relay has not confirmed room [channelId] yet: held only while
/// its details are still on their way. A failed lookup releases it, as a
/// cached error would otherwise hold rooms the relay's list leaves out
/// (hidden DMs, huddle rooms) for good.
bool composerHeld(WidgetRef ref, String channelId) {
  final channels = ref.watch(channelsProvider).value;
  final notifier = ref.read(channelsProvider.notifier);
  if (notifier.hasLoaded &&
      !notifier.showsSavedList &&
      (channels?.any((channel) => channel.id == channelId) ?? false)) {
    return false;
  }
  final details = ref.watch(channelDetailsProvider(channelId));
  return !details.hasValue && !details.hasError;
}
