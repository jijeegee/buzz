part of '../channels_page.dart';

/// Letters typed before public groups you're not in are listed.
const _kFindGroupsMinQueryLength = 2;

/// What Find groups closed with, besides a group to open.
enum _FindGroupsAction { newGroup }

/// Find groups: your groups always, public groups you're not in only once
/// the search has [_kFindGroupsMinQueryLength] or more letters. Pops with the
/// [Channel] to open or [_FindGroupsAction.newGroup].
class _BrowseChannelsSheet extends HookConsumerWidget {
  const _BrowseChannelsSheet();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final channelsAsync = ref.watch(channelsProvider);
    final directoryState = ref.watch(channelDirectoryLoadStatusProvider);
    final activeDirectoryScope = channelDirectoryScope(
      ref.watch(relayConfigProvider).baseUrl,
      ref.watch(myPubkeyProvider),
    );
    final directoryStatus = directoryState.scope == activeDirectoryScope
        ? directoryState.status
        : ChannelDirectoryLoadStatus.idle;
    final queryController = useTextEditingController();
    useListenable(queryController);
    final query = queryController.text.trim().toLowerCase();
    final searchesPublic = query.length >= _kFindGroupsMinQueryLength;

    bool matches(Channel channel) =>
        query.isEmpty ||
        channel.name.toLowerCase().contains(query) ||
        channel.description.toLowerCase().contains(query);
    int byName(Channel left, Channel right) =>
        left.name.toLowerCase().compareTo(right.name.toLowerCase());

    final all = channelsAsync.asData?.value;
    final myGroups = [
      for (final channel in all ?? const <Channel>[])
        if (channel.isMember &&
            !channel.isDm &&
            !channel.isArchived &&
            matches(channel))
          channel,
    ]..sort(byName);
    final publicGroups = all == null || !searchesPublic
        ? null
        : ([
            for (final channel in all)
              if (channel.canJoin && matches(channel)) channel,
          ]..sort(byName));

    useEffect(() {
      unawaited(
        Future<void>.microtask(
          ref.read(channelsProvider.notifier).ensureDirectoryLoaded,
        ),
      );
      return null;
    }, const []);

    final directoryIsLoading =
        directoryStatus == ChannelDirectoryLoadStatus.idle ||
        directoryStatus == ChannelDirectoryLoadStatus.loading;
    final directoryHasError =
        directoryStatus == ChannelDirectoryLoadStatus.error ||
        channelsAsync.hasError;
    final mutedStyle = context.textTheme.bodyMedium?.copyWith(
      color: context.colors.onSurfaceVariant,
    );

    Widget sectionHeader(String key, String title) => Padding(
      key: Key('find-groups-section-$key'),
      padding: const EdgeInsets.only(top: Grid.xs, bottom: Grid.half),
      child: Text(
        title,
        style: context.textTheme.labelLarge?.copyWith(
          color: context.colors.onSurfaceVariant,
          fontWeight: FontWeight.w600,
        ),
      ),
    );

    Widget note(String key, String text) => Padding(
      key: Key(key),
      padding: const EdgeInsets.symmetric(vertical: Grid.xxs),
      child: Text(text, style: mutedStyle),
    );

    return SafeArea(
      top: false,
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          MediaQuery.viewInsetsOf(context).bottom + Grid.xs,
        ),
        child: CustomScrollView(
          shrinkWrap: true,
          slivers: [
            SliverToBoxAdapter(
              child: _CreateChannelFieldShell(
                child: TextField(
                  key: const Key('find-groups-search'),
                  controller: queryController,
                  autocorrect: false,
                  enableSuggestions: false,
                  textInputAction: TextInputAction.search,
                  decoration: InputDecoration(
                    hintText: 'Search groups',
                    prefixIcon: Icon(
                      LucideIcons.search,
                      size: 18,
                      color: context.colors.onSurfaceVariant,
                    ),
                    border: InputBorder.none,
                    enabledBorder: InputBorder.none,
                    focusedBorder: InputBorder.none,
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: Grid.twelve,
                      vertical: Grid.xs,
                    ),
                  ),
                ),
              ),
            ),
            SliverToBoxAdapter(
              child: ListTile(
                key: const Key('find-groups-new-group'),
                contentPadding: EdgeInsets.zero,
                leading: SizedBox.square(
                  dimension: 32,
                  child: Icon(
                    LucideIcons.plus,
                    size: 20,
                    color: context.colors.primary,
                  ),
                ),
                title: const Text('New group'),
                onTap: () =>
                    Navigator.of(context).pop(_FindGroupsAction.newGroup),
              ),
            ),
            SliverToBoxAdapter(child: sectionHeader('mine', 'My groups')),
            if (all == null && channelsAsync.isLoading)
              const SliverToBoxAdapter(
                child: Padding(
                  padding: EdgeInsets.all(Grid.sm),
                  child: Center(child: KuzzLoadingIndicator()),
                ),
              )
            else if (myGroups.isEmpty)
              SliverToBoxAdapter(
                child: note(
                  'find-groups-mine-empty',
                  query.isEmpty
                      ? 'You’re not in any groups yet.'
                      : 'None of your groups match.',
                ),
              )
            else
              SliverList.builder(
                itemCount: myGroups.length,
                itemBuilder: (context, index) {
                  final channel = myGroups[index];
                  return ListTile(
                    key: Key('find-groups-mine-${channel.id}'),
                    contentPadding: EdgeInsets.zero,
                    leading: GroupAvatar(channel: channel, size: 32),
                    title: Text(
                      channel.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    onTap: () => Navigator.of(context).pop(channel),
                  );
                },
              ),
            SliverToBoxAdapter(child: sectionHeader('public', 'Public groups')),
            if (!searchesPublic)
              SliverToBoxAdapter(
                child: note(
                  'find-groups-public-hint',
                  'Type 2 or more letters to find public groups.',
                ),
              )
            else if (directoryIsLoading &&
                (publicGroups == null || publicGroups.isEmpty))
              const SliverToBoxAdapter(
                child: Padding(
                  padding: EdgeInsets.all(Grid.sm),
                  child: Center(child: KuzzLoadingIndicator()),
                ),
              )
            else if (directoryHasError &&
                (publicGroups == null || publicGroups.isEmpty))
              SliverToBoxAdapter(
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: Grid.sm),
                  child: Column(
                    children: [
                      Text(
                        'Couldn’t load public groups.',
                        textAlign: TextAlign.center,
                        style: mutedStyle,
                      ),
                      const SizedBox(height: Grid.xxs),
                      TextButton(
                        key: const Key('browse-channels-retry'),
                        onPressed: () => unawaited(
                          ref.read(channelsProvider.notifier).retryDirectory(),
                        ),
                        child: const Text('Try again'),
                      ),
                    ],
                  ),
                ),
              )
            else if (publicGroups == null || publicGroups.isEmpty)
              SliverToBoxAdapter(
                child: note(
                  'find-groups-public-empty',
                  'No public groups match.',
                ),
              )
            else
              SliverList.builder(
                itemCount: publicGroups.length,
                itemBuilder: (context, index) => _JoinableChannelTile(
                  channel: publicGroups[index],
                  closeAfterJoin: true,
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _JoinableChannelTile extends HookConsumerWidget {
  final Channel channel;
  final bool closeAfterJoin;

  const _JoinableChannelTile({
    required this.channel,
    required this.closeAfterJoin,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final isJoining = useState(false);
    final actionError = useState<String?>(null);

    Future<void> join() async {
      if (isJoining.value) return;
      isJoining.value = true;
      actionError.value = null;
      try {
        await ref.read(channelActionsProvider).joinChannel(channel.id);
        if (closeAfterJoin && context.mounted) Navigator.of(context).pop();
      } catch (error) {
        actionError.value = error.toString();
      } finally {
        isJoining.value = false;
      }
    }

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        ListTile(
          key: Key('browse-channel-${channel.id}'),
          contentPadding: EdgeInsets.zero,
          leading: GroupAvatar(channel: channel, size: 32),
          title: Text(channel.name),
          subtitle: channel.description.trim().isEmpty
              ? null
              : Text(
                  channel.description,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
          trailing: FilledButton.tonal(
            key: Key('browse-channel-join-${channel.id}'),
            onPressed: isJoining.value ? null : () => unawaited(join()),
            child: Text(isJoining.value ? 'Joining…' : 'Join'),
          ),
        ),
        if (actionError.value case final error?)
          Align(
            alignment: Alignment.centerLeft,
            child: Text(
              error,
              key: Key('browse-channel-error-${channel.id}'),
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
      ],
    );
  }
}
