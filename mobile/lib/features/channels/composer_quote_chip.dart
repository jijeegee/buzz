import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'composer_quote_provider.dart';

/// Removable "Quoting {author}" chip shown above a composer while its next
/// send carries a NIP-18 quote. Renders nothing when [scope] has no quote.
class ComposerQuoteChip extends ConsumerWidget {
  final ComposerQuoteScope scope;

  const ComposerQuoteChip({super.key, required this.scope});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final target = ref.watch(composerQuoteProvider(scope));
    if (target == null) return const SizedBox.shrink();

    return Padding(
      padding: const EdgeInsets.fromLTRB(Grid.twelve, 0, Grid.twelve, Grid.xxs),
      child: Semantics(
        container: true,
        label: 'Quoting ${target.author}',
        child: DecoratedBox(
          key: const ValueKey('composer-quote-chip'),
          decoration: BoxDecoration(
            color: context.colors.surfaceContainerHigh,
            borderRadius: BorderRadius.circular(Radii.lg),
            border: Border.all(color: context.colors.outlineVariant),
          ),
          child: Padding(
            padding: const EdgeInsets.only(
              left: Grid.twelve,
              top: Grid.xxs,
              bottom: Grid.xxs,
            ),
            child: Row(
              children: [
                Icon(
                  LucideIcons.quote,
                  size: 16,
                  color: context.colors.onSurfaceVariant,
                ),
                const SizedBox(width: Grid.xxs),
                Expanded(
                  child: ExcludeSemantics(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          'Quoting ${target.author}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: replyPreviewTextStyle.copyWith(
                            color: context.colors.onSurface,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                        Text(
                          target.excerpt,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: replyPreviewTextStyle.copyWith(
                            color: context.colors.onSurfaceVariant,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                IconButton(
                  key: const ValueKey('composer-quote-cancel'),
                  tooltip: 'Cancel quote',
                  visualDensity: VisualDensity.compact,
                  icon: Icon(
                    LucideIcons.x,
                    size: 18,
                    color: context.colors.onSurfaceVariant,
                  ),
                  onPressed: () =>
                      ref.read(composerQuoteProvider(scope).notifier).clear(),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
