import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/deeplink/deep_link.dart';
import '../../shared/deeplink/pending_deep_link_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import 'channel_identity_names_provider.dart';
import 'message_quote.dart';
import 'quoted_message_provider.dart';
import 'timeline_message.dart';

/// Compact header for a message carrying a NIP-18 `q` tag.
///
/// Resolves the quoted message from [loadedMessages] first and falls back to
/// an event lookup by id. Tapping it opens the original message through the
/// same `buzz://message` deep-link navigation used by message links. Renders
/// nothing when [tags] has no well-formed quote.
class MessageQuoteHeader extends ConsumerWidget {
  final String channelId;
  final List<List<String>> tags;
  final List<TimelineMessage>? loadedMessages;

  const MessageQuoteHeader({
    super.key,
    required this.channelId,
    required this.tags,
    this.loadedMessages,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final reference = quoteReferenceOf(tags);
    if (reference == null) return const SizedBox.shrink();

    TimelineMessage? quoted;
    for (final message in loadedMessages ?? const <TimelineMessage>[]) {
      if (message.id.toLowerCase() == reference.eventId) {
        quoted = message;
        break;
      }
    }
    var quotedChannelId = channelId;
    if (quoted == null) {
      final lookup = ref.watch(quotedMessageProvider(reference.eventId));
      if (lookup.isLoading && !lookup.hasValue) {
        return const _QuoteHeaderPlaceholder(text: 'Loading quoted message…');
      }
      final fetched = lookup.value;
      if (fetched == null || fetched.message.isSystem) {
        return const _QuoteHeaderPlaceholder(
          text: 'Original message unavailable',
        );
      }
      quoted = fetched.message;
      quotedChannelId = fetched.channelId ?? channelId;
    }

    // The quoting author recorded the quoted author in the `q` tag; prefer it
    // so a relay-signed message still names its real author.
    final authorPubkey = (reference.authorPubkey ?? quoted.pubkey)
        .toLowerCase();
    if (ref.watch(userCacheProvider.select((cache) => cache[authorPubkey])) ==
        null) {
      ref.read(userCacheProvider.notifier).get(authorPubkey);
    }
    final author = watchChannelIdentityLabel(ref, channelId, authorPubkey);
    final excerpt = quoteExcerpt(quoted.content);
    final threadRootId = quoted.rootId;
    final quotedMessageId = reference.eventId;

    return Padding(
      padding: const EdgeInsets.only(bottom: Grid.half),
      child: Semantics(
        button: true,
        label: 'Jump to quoted message from $author',
        excludeSemantics: true,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            key: ValueKey('message-quote-header-$quotedMessageId'),
            borderRadius: BorderRadius.circular(Radii.md),
            onTap: () => ref
                .read(pendingDeepLinkProvider.notifier)
                .open(
                  Uri.parse(
                    buildMessageLink(
                      channelId: quotedChannelId,
                      messageId: quotedMessageId,
                      threadRootId: threadRootId == quotedMessageId
                          ? null
                          : threadRootId,
                    ),
                  ),
                ),
            child: _QuoteHeaderFrame(
              children: [
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 160),
                  child: Text(
                    author,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: replyPreviewTextStyle.copyWith(
                      color: context.colors.onSurface,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                const SizedBox(width: Grid.half),
                Flexible(
                  child: Text(
                    excerpt,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: replyPreviewTextStyle.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _QuoteHeaderPlaceholder extends StatelessWidget {
  final String text;

  const _QuoteHeaderPlaceholder({required this.text});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: Grid.half),
      child: _QuoteHeaderFrame(
        children: [
          Flexible(
            child: Text(
              text,
              key: const ValueKey('message-quote-header-placeholder'),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: replyPreviewTextStyle.copyWith(
                color: context.colors.onSurfaceVariant,
                fontStyle: FontStyle.italic,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Left-ruled, tinted row shared by the resolved and fallback headers.
class _QuoteHeaderFrame extends StatelessWidget {
  final List<Widget> children;

  const _QuoteHeaderFrame({required this.children});

  @override
  Widget build(BuildContext context) {
    return ClipRRect(
      borderRadius: BorderRadius.circular(Radii.md),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: context.colors.surfaceContainerHighest.withValues(alpha: 0.6),
          border: Border(
            left: BorderSide(color: context.colors.outlineVariant, width: 2),
          ),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: Grid.xxs,
            vertical: Grid.quarter,
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                LucideIcons.quote,
                size: 14,
                color: context.colors.onSurfaceVariant,
              ),
              const SizedBox(width: Grid.half),
              ...children,
            ],
          ),
        ),
      ),
    );
  }
}
