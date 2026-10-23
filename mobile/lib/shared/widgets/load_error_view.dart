import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../theme/theme.dart';
import '../l10n/l10n.dart';

/// Settled load failure with an explicit Retry. After a relay deadline,
/// Riverpod does not auto-retry, so this is the user's way to re-send.
class LoadErrorView extends StatelessWidget {
  final String message;
  final VoidCallback onRetry;

  const LoadErrorView({
    super.key,
    required this.message,
    required this.onRetry,
  });

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            LucideIcons.triangleAlert,
            size: Grid.lg,
            color: context.colors.error,
          ),
          const SizedBox(height: Grid.xxs),
          Text(
            message,
            style: context.textTheme.bodyMedium?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: Grid.xs),
          FilledButton.icon(
            key: const ValueKey('load-error-retry'),
            onPressed: onRetry,
            icon: const Icon(LucideIcons.refreshCcw, size: 16),
            label: Text(context.l10n.loadErrorRetry),
          ),
        ],
      ),
    );
  }
}

/// One-line load failure above content that is still shown, e.g. a list
/// whose refresh failed, with the same Retry as [LoadErrorView].
class LoadErrorBanner extends StatelessWidget {
  final String message;
  final VoidCallback onRetry;
  final EdgeInsetsGeometry padding;

  const LoadErrorBanner({
    super.key,
    required this.message,
    required this.onRetry,
    this.padding = const EdgeInsets.symmetric(
      horizontal: Grid.gutter,
      vertical: Grid.half,
    ),
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: padding,
      child: Row(
        children: [
          Icon(
            LucideIcons.triangleAlert,
            size: 16,
            color: context.colors.error,
          ),
          const SizedBox(width: Grid.xxs),
          Expanded(
            child: Text(
              message,
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ),
          TextButton(
            key: const ValueKey('load-error-banner-retry'),
            onPressed: onRetry,
            child: Text(context.l10n.loadErrorRetry),
          ),
        ],
      ),
    );
  }
}
