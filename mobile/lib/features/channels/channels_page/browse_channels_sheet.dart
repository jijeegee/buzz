part of '../channels_page.dart';

/// Letters typed before public groups you're not in are listed.
const _kFindGroupsMinQueryLength = 2;

/// Pause after typing before public groups are searched.
const _kFindGroupsSearchDebounce = Duration(milliseconds: 250);

/// Public groups shown at most.
const _kFindGroupsMaxResults = 20;

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
    final queryController = useTextEditingController();
    useListenable(queryController);
    final rawQuery = queryController.text.trim();
    final query = rawQuery.toLowerCase();
    final searchesPublic = query.length >= _kFindGroupsMinQueryLength;
    final retry = useState(0);
    final publicResults = useState<AsyncValue<List<GroupSummary>>?>(null);

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
            !channel.isPersonChat &&
            !channel.isArchived &&
            matches(channel))
          channel,
    ]..sort(byName);

    // Public groups come from the relay's search, 250 ms after typing stops.
    useEffect(() {
      if (!searchesPublic) {
        publicResults.value = null;
        return null;
      }
      if (publicResults.value?.hasValue != true) {
        publicResults.value = const AsyncLoading();
      }
      var cancelled = false;
      final timer = Timer(_kFindGroupsSearchDebounce, () async {
        try {
          final groups = await ref
              .read(groupsApiProvider)
              .search(rawQuery, limit: _kFindGroupsMaxResults);
          if (!cancelled) publicResults.value = AsyncData(groups);
        } catch (error, stackTrace) {
          if (!cancelled) {
            publicResults.value = AsyncError(error, stackTrace);
          }
        }
      });
      return () {
        cancelled = true;
        timer.cancel();
      };
    }, [rawQuery, searchesPublic, retry.value]);

    final publicGroups = publicResults.value?.asData?.value;
    final publicIsLoading =
        publicResults.value == null || publicResults.value!.isLoading;
    final publicHasError = publicResults.value?.hasError == true;
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
                    hintText: context.l10n.channelFindGroupsSearchHint,
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
                title: Text(context.l10n.channelNewGroup),
                onTap: () =>
                    Navigator.of(context).pop(_FindGroupsAction.newGroup),
              ),
            ),
            SliverToBoxAdapter(
              child: sectionHeader('mine', context.l10n.channelFindGroupsMine),
            ),
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
                      ? context.l10n.channelFindGroupsMineEmpty
                      : context.l10n.channelFindGroupsMineNoMatch,
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
            SliverToBoxAdapter(
              child: sectionHeader(
                'public',
                context.l10n.channelFindGroupsPublic,
              ),
            ),
            if (!searchesPublic)
              SliverToBoxAdapter(
                child: note(
                  'find-groups-public-hint',
                  context.l10n.channelFindGroupsPublicHint,
                ),
              )
            else if (publicIsLoading && publicGroups == null)
              const SliverToBoxAdapter(
                child: Padding(
                  padding: EdgeInsets.all(Grid.sm),
                  child: Center(child: KuzzLoadingIndicator()),
                ),
              )
            else if (publicHasError)
              SliverToBoxAdapter(
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: Grid.sm),
                  child: Column(
                    children: [
                      Text(
                        context.l10n.channelFindGroupsPublicError,
                        textAlign: TextAlign.center,
                        style: mutedStyle,
                      ),
                      const SizedBox(height: Grid.xxs),
                      TextButton(
                        key: const Key('browse-channels-retry'),
                        onPressed: () => retry.value++,
                        child: Text(context.l10n.commonTryAgain),
                      ),
                    ],
                  ),
                ),
              )
            else if (publicGroups == null || publicGroups.isEmpty)
              SliverToBoxAdapter(
                child: note(
                  'find-groups-public-empty',
                  context.l10n.channelFindGroupsPublicNoMatch,
                ),
              )
            else
              SliverList.builder(
                itemCount: publicGroups.length,
                itemBuilder: (context, index) =>
                    _PublicGroupTile(group: publicGroups[index]),
              ),
          ],
        ),
      ),
    );
  }
}

/// A public group search result: letter avatar, name, "N members" and
/// Join (Open when already a member). Tapping the row opens its preview.
/// Pops Find groups with the [Channel] to open.
class _PublicGroupTile extends HookConsumerWidget {
  final GroupSummary group;

  const _PublicGroupTile({required this.group});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final isJoining = useState(false);
    final actionError = useState<String?>(null);

    Future<void> join() async {
      if (isJoining.value) return;
      final navigator = Navigator.of(context);
      isJoining.value = true;
      actionError.value = null;
      try {
        if (!group.isMember) {
          await ref.read(channelActionsProvider).joinChannel(group.id);
        }
        final channel = await joinedChannel(ref, group.id, group);
        if (context.mounted) navigator.pop(channel);
      } catch (_) {
        if (context.mounted) {
          actionError.value = context.l10n.channelJoinGroupError;
        }
      } finally {
        if (context.mounted) isJoining.value = false;
      }
    }

    final channel = group.toChannel();
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        ListTile(
          key: Key('browse-channel-${group.id}'),
          contentPadding: EdgeInsets.zero,
          leading: GroupAvatar(channel: channel, size: 32),
          title: Text(group.name, maxLines: 1, overflow: TextOverflow.ellipsis),
          subtitle: Text(memberCountLabel(group.memberCount)),
          onTap: () => Navigator.of(context).pop(channel),
          trailing: FilledButton.tonal(
            key: Key('browse-channel-join-${group.id}'),
            onPressed: isJoining.value ? null : () => unawaited(join()),
            child: Text(
              group.isMember
                  ? context.l10n.channelFindGroupsOpen
                  : isJoining.value
                  ? context.l10n.channelFindGroupsJoining
                  : context.l10n.channelFindGroupsJoin,
            ),
          ),
        ),
        if (actionError.value case final error?)
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Text(
              error,
              key: Key('browse-channel-error-${group.id}'),
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
      ],
    );
  }
}
