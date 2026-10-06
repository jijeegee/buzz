part of '../channels_page.dart';

class _ChannelsBody extends StatelessWidget {
  final List<Channel>? channels;
  final AsyncValue<List<Channel>> channelsAsync;
  final bool showError;
  final SessionStatus sessionStatus;
  final bool showConnectionSkeleton;
  final String? currentPubkey;
  final double topSectionHeight;
  final bool usesPinnedGradient;
  final ScrollController scrollController;
  final Future<void> Function() onRefresh;
  final Future<void> Function(Channel channel) onSelectChannel;

  const _ChannelsBody({
    required this.channels,
    required this.channelsAsync,
    required this.showError,
    required this.sessionStatus,
    required this.showConnectionSkeleton,
    required this.currentPubkey,
    required this.topSectionHeight,
    required this.usesPinnedGradient,
    required this.scrollController,
    required this.onRefresh,
    required this.onSelectChannel,
  });

  @override
  Widget build(BuildContext context) {
    final barHeight = topSectionHeight;
    final loadedChannels = channels;
    final loading =
        showConnectionSkeleton || (loadedChannels == null && !showError);
    final content = showError && channelsAsync.hasError
        ? Padding(
            padding: EdgeInsets.only(top: barHeight),
            child: _ErrorView(error: channelsAsync.error!, onRetry: onRefresh),
          )
        : loadedChannels == null
        ? const SizedBox.shrink()
        : BeeRefreshIndicator(
            edgeOffset: barHeight,
            onRefresh: onRefresh,
            child: CustomScrollView(
              controller: scrollController,
              // Transparent list gaps must remain hit-testable so a new drag
              // can interrupt ballistic scrolling. The app bar is painted
              // later and retains its community and profile controls.
              hitTestBehavior: HitTestBehavior.translucent,
              slivers: [
                SliverToBoxAdapter(child: SizedBox(height: barHeight)),
                if (usesPinnedGradient)
                  _SliverChannelsList(
                    channels: loadedChannels,
                    currentPubkey: currentPubkey,
                    onSelectChannel: onSelectChannel,
                  )
                else
                  DecoratedSliver(
                    decoration: BoxDecoration(
                      color: context.colors.surface,
                      borderRadius: const BorderRadius.vertical(
                        top: Radius.circular(Radii.dialog),
                      ),
                    ),
                    sliver: _SliverChannelsList(
                      channels: loadedChannels,
                      currentPubkey: currentPubkey,
                      onSelectChannel: onSelectChannel,
                    ),
                  ),
              ],
            ),
          );

    return SkeletonReveal(
      loading: loading,
      shimmerEnabled: sessionStatus != SessionStatus.disconnected,
      skeleton: _ChannelsSkeleton(
        channels: loadedChannels,
        topInset: barHeight,
        status: sessionStatus,
      ),
      content: content,
    );
  }
}

class _SliverChannelsList extends HookConsumerWidget {
  final List<Channel> channels;
  final String? currentPubkey;
  final Future<void> Function(Channel channel) onSelectChannel;

  const _SliverChannelsList({
    required this.channels,
    required this.currentPubkey,
    required this.onSelectChannel,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final readState = ref.watch(readStateProvider);
    final mutesState = ref.watch(channelMutesProvider);
    final mutedChannelIds = {
      for (final entry in mutesState.store.channels.entries)
        if (entry.value.muted) entry.key,
    };
    final starsState = ref.watch(channelStarsProvider);
    final starredChannelIds = {
      for (final entry in starsState.store.channels.entries)
        if (entry.value.starred) entry.key,
    };
    final visibleChannels = channels
        .where(
          (channel) =>
              !channel.isArchived &&
              (channel.isDm || channel.isStream && channel.isMember),
        )
        .toList();
    final chats = sortChats(visibleChannels, starredChannelIds);
    final chatsExpanded = useState(true);
    final initialSeedComplete = useState(false);
    final seededPubkey = useRef<String?>(null);
    final seedCompleteForPubkey =
        seededPubkey.value == readState.pubkey && initialSeedComplete.value;

    useEffect(() {
      if (!readState.isReady) {
        return null;
      }

      return deferReadStateUpdate(context, () {
        if (seededPubkey.value != readState.pubkey) {
          seededPubkey.value = readState.pubkey;
          initialSeedComplete.value = false;
        }

        if (initialSeedComplete.value) {
          return;
        }

        final notifier = ref.read(readStateProvider.notifier);
        for (final channel in visibleChannels) {
          if (readState.effectiveTimestamp(channel.id) != null) {
            continue;
          }

          final lastMessageAt = dateTimeToUnixSeconds(channel.lastMessageAt);
          if (lastMessageAt != null) {
            notifier.seedContextRead(channel.id, lastMessageAt);
          }
        }
        initialSeedComplete.value = true;
      });
    }, [readState.isReady, readState.pubkey, visibleChannels]);

    final unreadState = _computeUnreadChannelState(
      channels: visibleChannels,
      readState: readState,
      channelsNotifier: ref.read(channelsProvider.notifier),
    );
    final unreadChannelIds = {
      for (final channelId in unreadState.ids)
        if (seedCompleteForPubkey ||
            readState.effectiveTimestamp(channelId) != null)
          channelId,
    };
    return SliverPadding(
      padding: EdgeInsets.only(
        top: Grid.xxs,
        bottom: MediaQuery.paddingOf(context).bottom,
      ),
      sliver: SliverList.list(
        children: [
          if (starsState.errorMessage case final error?)
            ListTile(
              title: Text(error),
              trailing: IconButton(
                tooltip: 'Retry personal pins',
                icon: const Icon(LucideIcons.refreshCw),
                onPressed: () => ref.invalidate(channelStarsProvider),
              ),
            ),
          if (visibleChannels.isEmpty)
            const _EmptyState()
          else
            _ChannelSection(
              title: 'Chats',
              icon: LucideIcons.messagesSquare,
              showTopDivider: false,
              expanded: chatsExpanded.value,
              onToggle: () => chatsExpanded.value = !chatsExpanded.value,
              channels: chats,
              unreadChannelIds: unreadChannelIds,
              mutedChannelIds: mutedChannelIds,
              currentPubkey: currentPubkey,
              emptyLabel: 'No chats yet',
              onSelectChannel: onSelectChannel,
            ),
        ],
      ),
    );
  }
}
