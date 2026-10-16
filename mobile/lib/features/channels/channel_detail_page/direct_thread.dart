part of '../channel_detail_page.dart';

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
                semanticLabel: 'Back',
                onPressed: () => Navigator.of(context).maybePop(),
                width: iosGlassChannelHeaderLeadingWidth,
                buttonCenterX: iosGlassChannelHeaderButtonCenterX,
              )
            : null,
        iconColor: context.colors.primary,
        title: Padding(
          padding: EdgeInsets.only(
            left: usesNativeIosGlassBackButton
                ? iosGlassChannelHeaderTitleSpacing
                : 0,
          ),
          child: Text(
            threadName == null || threadName.isEmpty ? 'Thread' : threadName,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        ),
        titleStyle: channelTitleTextStyle,
      ),
      body: Align(
        alignment: Alignment.bottomLeft,
        child: Padding(
          padding: EdgeInsets.only(
            left: chatListLeftInset,
            right: chatListRightInset,
            bottom: Grid.xs + MediaQuery.paddingOf(context).bottom,
          ),
          child: Text(
            'Loading replies…',
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
