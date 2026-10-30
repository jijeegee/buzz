import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/l10n/l10n.dart';
import 'channels_provider.dart';

/// A small row over a list drawn from saved data when the relay's channel
/// list failed: the list stays, this offers to ask again (the notifier also
/// retries by itself). Nothing while the relay's list is fine.
class ChannelListRetryRow extends ConsumerWidget {
  const ChannelListRetryRow({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (ref.watch(channelListLoadErrorProvider) == null) {
      return const SizedBox.shrink();
    }
    return ListTile(
      key: const Key('channel-list-retry-row'),
      title: Text(context.l10n.channelListLoadError),
      trailing: IconButton(
        tooltip: context.l10n.commonRetry,
        icon: const Icon(LucideIcons.refreshCw),
        onPressed: () =>
            unawaited(ref.read(channelsProvider.notifier).retryRelayList()),
      ),
    );
  }
}
