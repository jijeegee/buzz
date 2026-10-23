part of '../channels_page.dart';

class _EphemeralBadge extends StatelessWidget {
  final Channel channel;

  const _EphemeralBadge({required this.channel});

  @override
  Widget build(BuildContext context) {
    final display = ephemeralChannelDisplay(channel);
    if (display == null) return const SizedBox.shrink();

    return Tooltip(
      message: display.tooltipLabel,
      child: Icon(
        LucideIcons.clockFading,
        key: Key('channel-ephemeral-${channel.id}'),
        size: 16,
        color: context.colors.onSurfaceVariant,
      ),
    );
  }
}

class _ErrorView extends StatelessWidget {
  final Object error;
  final VoidCallback onRetry;

  const _ErrorView({required this.error, required this.onRetry});

  static String _userMessage(AppLocalizations l10n, Object error) {
    if (error is RelayException) {
      if (error.statusCode == 401) {
        return l10n.channelListErrorUnauthorized;
      }
      if (error.statusCode == 403) {
        return l10n.channelListErrorForbidden;
      }
      return l10n.channelListErrorServer(error.statusCode);
    }
    if (error is SocketException) {
      return l10n.channelListErrorUnreachable;
    }
    return l10n.channelListErrorGeneric;
  }

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(Grid.sm),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              LucideIcons.wifiOff,
              size: Grid.xl,
              color: context.colors.error,
            ),
            const SizedBox(height: Grid.xs),
            Text(
              context.l10n.channelListLoadError,
              style: context.textTheme.titleMedium,
            ),
            const SizedBox(height: Grid.xxs),
            Text(
              _userMessage(context.l10n, error),
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
              textAlign: TextAlign.center,
              maxLines: 3,
              overflow: TextOverflow.ellipsis,
            ),
            const SizedBox(height: Grid.xs),
            FilledButton.icon(
              onPressed: onRetry,
              icon: const Icon(LucideIcons.refreshCw),
              label: Text(context.l10n.commonRetry),
            ),
          ],
        ),
      ),
    );
  }
}
