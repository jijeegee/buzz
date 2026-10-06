part of '../channels_page.dart';

class _ChannelSection extends StatelessWidget {
  final String title;
  final IconData icon;
  final bool expanded;
  final VoidCallback onToggle;
  final List<Channel> channels;
  final bool showTopDivider;
  final Set<String> unreadChannelIds;
  final Set<String> mutedChannelIds;
  final String? currentPubkey;
  final String emptyLabel;
  final Future<void> Function(Channel channel) onSelectChannel;

  const _ChannelSection({
    required this.title,
    required this.icon,
    required this.expanded,
    required this.onToggle,
    required this.channels,
    required this.showTopDivider,
    required this.unreadChannelIds,
    required this.mutedChannelIds,
    required this.currentPubkey,
    required this.emptyLabel,
    required this.onSelectChannel,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (showTopDivider) const _SectionDivider(),
        _SectionHeader(
          label: title,
          icon: icon,
          expanded: expanded,
          onToggle: onToggle,
        ),
        _AnimatedSectionBody(
          expanded: expanded,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (channels.isEmpty)
                Padding(
                  padding: const EdgeInsets.only(
                    left: _kChannelLabelInset,
                    right: _kChannelSectionInset,
                    top: Grid.half,
                    bottom: Grid.half,
                  ),
                  child: Text(
                    emptyLabel,
                    style: contentListBodyTextStyle.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                )
              else
                for (final channel in channels)
                  _ChannelTile(
                    channel: channel,
                    isUnread: unreadChannelIds.contains(channel.id),
                    isMuted: mutedChannelIds.contains(channel.id),
                    currentPubkey: currentPubkey,
                    onTap: () => onSelectChannel(channel),
                    onMarkRead: null,
                    sectionId: null,
                  ),
              const SizedBox(height: _kExpandedSectionTrailingPadding),
            ],
          ),
        ),
      ],
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState();

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: MediaQuery.sizeOf(context).height * 0.55,
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              LucideIcons.messagesSquare,
              size: Grid.xl,
              color: context.colors.onSurfaceVariant,
            ),
            const SizedBox(height: Grid.xs),
            Text(
              'No conversations yet',
              style: context.textTheme.bodyLarge?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _SectionDivider extends StatelessWidget {
  const _SectionDivider();

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Grid.xxs),
      child: Divider(
        height: 1,
        thickness: 1,
        indent: _kChannelSectionInset,
        endIndent: _kChannelSectionInset,
        color: context.colors.primary.withValues(alpha: 0.15),
      ),
    );
  }
}

class _SectionHeader extends StatelessWidget {
  final String label;
  final IconData icon;
  final bool expanded;
  final VoidCallback onToggle;

  const _SectionHeader({
    required this.label,
    required this.icon,
    required this.expanded,
    required this.onToggle,
  });

  @override
  Widget build(BuildContext context) {
    final sectionColor = navigationSectionForeground(context);

    return InkWell(
      onTap: onToggle,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(
          Grid.gutter,
          _kSectionHeaderVerticalPadding,
          Grid.gutter,
          _kSectionHeaderVerticalPadding,
        ),
        child: Row(
          children: [
            SizedBox(
              width: _kChannelLeadingWidth,
              child: Align(
                alignment: Alignment.centerLeft,
                child: Icon(icon, size: _kChannelIconSize, color: sectionColor),
              ),
            ),
            const SizedBox(width: _kChannelLabelGap),
            Text(
              label,
              style: contentListTitleTextStyle.copyWith(
                color: sectionColor,
                fontWeight: FontWeight.w600,
              ),
            ),
            const Spacer(),
            _SectionChevron(expanded: expanded, color: sectionColor),
          ],
        ),
      ),
    );
  }
}

class _SectionChevron extends StatelessWidget {
  final bool expanded;
  final Color color;

  const _SectionChevron({required this.expanded, required this.color});

  @override
  Widget build(BuildContext context) {
    final reducedMotion = MediaQuery.of(context).disableAnimations;

    return AnimatedRotation(
      turns: expanded ? 0 : -0.25,
      duration: reducedMotion ? Duration.zero : _kSectionExpandDuration,
      curve: _kSectionExpandCurve,
      child: Icon(
        LucideIcons.chevronDown,
        size: _kChannelIconSize,
        color: color,
      ),
    );
  }
}

class _AnimatedSectionBody extends HookWidget {
  final bool expanded;
  final Widget child;

  const _AnimatedSectionBody({required this.expanded, required this.child});

  @override
  Widget build(BuildContext context) {
    final reducedMotion = MediaQuery.of(context).disableAnimations;
    final controller = useAnimationController(
      duration: reducedMotion ? Duration.zero : _kSectionExpandDuration,
      reverseDuration: reducedMotion
          ? Duration.zero
          : _kSectionCollapseDuration,
      initialValue: expanded ? 1 : 0,
    );
    final curvedAnimation = useMemoized(
      () => CurvedAnimation(
        parent: controller,
        curve: _kSectionExpandCurve,
        reverseCurve: _kSectionCollapseCurve,
      ),
      [controller],
    );
    final shouldRender = useState(expanded);

    useEffect(() => curvedAnimation.dispose, [curvedAnimation]);

    useEffect(() {
      void handleStatus(AnimationStatus status) {
        if (status == AnimationStatus.dismissed && !expanded) {
          shouldRender.value = false;
        }
      }

      controller.addStatusListener(handleStatus);
      return () => controller.removeStatusListener(handleStatus);
    }, [controller, expanded]);

    useEffect(() {
      controller.duration = reducedMotion
          ? Duration.zero
          : _kSectionExpandDuration;
      controller.reverseDuration = reducedMotion
          ? Duration.zero
          : _kSectionCollapseDuration;

      if (expanded) {
        shouldRender.value = true;
        if (reducedMotion) {
          controller.value = 1;
        } else {
          unawaited(controller.forward());
        }
      } else if (reducedMotion) {
        controller.value = 0;
        shouldRender.value = false;
      } else {
        unawaited(controller.reverse());
      }

      return null;
    }, [controller, expanded, reducedMotion]);

    return ClipRect(
      child: AnimatedBuilder(
        animation: curvedAnimation,
        child: shouldRender.value ? child : const SizedBox.shrink(),
        builder: (context, child) {
          final value = curvedAnimation.value.clamp(0.0, 1.0);
          final scaleY =
              _kSectionCollapsedScaleY +
              ((1 - _kSectionCollapsedScaleY) * value);

          return Align(
            alignment: Alignment.topCenter,
            heightFactor: value,
            child: Opacity(
              opacity: value,
              child: Transform.scale(
                alignment: Alignment.topCenter,
                scaleY: scaleY,
                child: ExcludeSemantics(
                  excluding: !expanded,
                  child: IgnorePointer(ignoring: !expanded, child: child),
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}
