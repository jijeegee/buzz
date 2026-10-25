part of '../channel_detail_page.dart';

/// Fetch deep-link targets that may be outside the loaded channel window.
Future<void> _loadDeepLinkEvents(
  WidgetRef ref,
  String channelId,
  Set<String> eventIds,
) async {
  try {
    await ref
        .read(channelMessagesProvider(channelId).notifier)
        .loadEventsById(eventIds);
  } catch (error) {
    debugPrint('deep-link: failed to load target messages: $error');
  }
}

// Start the newest replies alongside the head. The thread page watches the
// same provider, including its stored page and any background refresh.
void _useDirectThreadReplies(WidgetRef ref, Channel channel, String? rootId) {
  useEffect(() {
    if (channel.isForum || rootId == null) return null;
    final subscription = ref.listenManual(
      threadRepliesProvider(
        ThreadRepliesArgs(channelId: channel.id, rootId: rootId),
      ),
      (_, _) {},
    );
    return subscription.close;
  }, [channel.id, rootId]);
}

NostrEvent? _validThreadSnapshot(
  String channelId,
  String? rootId,
  NostrEvent? snapshot,
) =>
    snapshot?.id == rootId &&
        snapshot?.channelId == channelId &&
        EventKind.channelTimelineContentKinds.contains(snapshot?.kind)
    ? snapshot
    : null;

NostrEvent? _directThreadHead(
  WidgetRef ref,
  String channelId,
  String? rootId,
  List<NostrEvent> events,
  NostrEvent? snapshot,
) {
  if (rootId == null) return null;
  return events.where((event) => event.id == rootId).firstOrNull ??
      ref
          .read(channelMessagesProvider(channelId).notifier)
          .deepLinkEvent(rootId) ??
      snapshot;
}

/// The replacement route owns a cached head's refresh. If the temporary
/// channel route owned it, that route's disposal would release the target
/// before the response arrived and the cached head would never be refreshed.
class _DirectThreadContent extends HookConsumerWidget {
  const _DirectThreadContent({
    required this.channelId,
    required this.refreshRootId,
    required this.child,
  });

  final String channelId;
  final String? refreshRootId;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final status = ref.watch(relaySessionProvider.select((s) => s.status));
    useEffect(() {
      final rootId = refreshRootId;
      if (rootId == null) return null;
      final notifier = ref.read(channelMessagesProvider(channelId).notifier);
      if (status == SessionStatus.connected) {
        unawaited(_loadDeepLinkEvents(ref, channelId, {rootId}));
      }
      return () => notifier.releaseDeepLinkEvents({rootId});
    }, [channelId, refreshRootId, status]);
    return child;
  }
}

/// Swaps the stand-in route for the thread without a second slide, so the
/// placeholder and the thread read as one screen. Back still animates.
class _DirectThreadRoute<T> extends MaterialPageRoute<T> {
  _DirectThreadRoute({required super.builder});

  @override
  Duration get transitionDuration => Duration.zero;

  @override
  Duration get reverseTransitionDuration => const Duration(milliseconds: 300);
}

/// What a thread opened straight from Inbox or a notification shows while its
/// head is on the way: the thread's title and loading row, never the channel.
class _DirectThreadPlaceholder extends ConsumerWidget {
  final String channelId;
  final String rootId;

  const _DirectThreadPlaceholder({
    required this.channelId,
    required this.rootId,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final threadName = ref
        .watch(threadNameProvider((channelId: channelId, headId: rootId)))
        .value
        ?.content;
    final usesNativeIosGlassBackButton =
        Navigator.canPop(context) &&
        Theme.of(context).platform == TargetPlatform.iOS;
    return FrostedScaffold(
      key: const ValueKey('direct-thread-placeholder'),
      appBar: FrostedAppBar(
        leading: usesNativeIosGlassBackButton
            ? IosGlassNavigationButton(
                key: const ValueKey('thread-ios-glass-back'),
                icon: IosGlassNavigationIcon.back,
                semanticLabel: context.l10n.commonBack,
                onPressed: () => Navigator.of(context).maybePop(),
                width: iosGlassChannelHeaderLeadingWidth,
                buttonCenterX: iosGlassChannelHeaderButtonCenterX,
              )
            : null,
        iconColor: context.colors.primary,
        title: Padding(
          padding: EdgeInsetsDirectional.only(
            start: usesNativeIosGlassBackButton
                ? iosGlassChannelHeaderTitleSpacing
                : 0,
          ),
          child: Text(
            threadName == null || threadName.isEmpty
                ? context.l10n.threadTitleFallback
                : threadName,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        ),
        titleStyle: channelTitleTextStyle,
      ),
      body: Align(
        alignment: AlignmentDirectional.bottomStart,
        child: Padding(
          padding: EdgeInsetsDirectional.only(
            start: chatListLeftInset,
            end: chatListRightInset,
            bottom: Grid.xs + MediaQuery.paddingOf(context).bottom,
          ),
          child: Text(
            context.l10n.threadLoadingReplies,
            key: const ValueKey('thread-replies-loading'),
            style: context.textTheme.labelMedium?.copyWith(
              color: context.colors.onSurfaceVariant,
              fontWeight: FontWeight.w600,
            ),
          ),
        ),
      ),
    );
  }
}
