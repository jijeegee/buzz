import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'message_quote.dart';

/// Identifies one composer: a channel's main timeline (`threadHeadId == null`)
/// or one thread inside it.
@immutable
class ComposerQuoteScope {
  final String channelId;
  final String? threadHeadId;

  const ComposerQuoteScope({required this.channelId, this.threadHeadId});

  @override
  bool operator ==(Object other) =>
      other is ComposerQuoteScope &&
      other.channelId == channelId &&
      other.threadHeadId == threadHeadId;

  @override
  int get hashCode => Object.hash(channelId, threadHeadId);
}

/// The message a composer will quote on its next send, if any.
class ComposerQuoteNotifier extends Notifier<QuoteTarget?> {
  /// The composer this quote belongs to.
  final ComposerQuoteScope scope;

  ComposerQuoteNotifier(this.scope);

  @override
  QuoteTarget? build() => null;

  /// Quote [target] on the next send, replacing any earlier quote.
  void quote(QuoteTarget target) => state = target;

  /// Remove the pending quote.
  void clear() => state = null;

  /// Remove the pending quote only if it is still [target] (after a send).
  void clearIf(QuoteTarget target) {
    if (state == target) state = null;
  }
}

/// Pending quote per composer. Auto-disposed with its conversation screen, so
/// a quote never follows the user into another channel or thread.
final composerQuoteProvider = NotifierProvider.autoDispose
    .family<ComposerQuoteNotifier, QuoteTarget?, ComposerQuoteScope>(
      ComposerQuoteNotifier.new,
    );
