part of '../compose_bar.dart';

String _composerDraftIdentity(WidgetRef ref) =>
    '${ref.watch(relayConfigProvider).baseUrl}'
    ':${ref.watch(myPubkeyProvider) ?? 'anon'}';

void _useComposerFocusRestorer({
  required ValueChanged<VoidCallback>? onChanged,
  required ValueNotifier<bool> isExpanded,
  required FocusNode focusNode,
  required VoidCallback expand,
}) {
  useEffect(() {
    if (onChanged == null) return null;

    var isCurrent = true;
    void restoreFocus() {
      if (!isCurrent) return;
      if (isExpanded.value) {
        focusNode.requestFocus();
      } else {
        expand();
      }
    }

    onChanged(restoreFocus);
    return () => isCurrent = false;
  }, [onChanged, focusNode]);
}

void _useComposerChannelNames(
  _MarkdownEditingController controller,
  AsyncValue<List<Channel>> channelsAsync,
) {
  final channelNames = {
    for (final channel in channelsAsync.asData?.value ?? const <Channel>[])
      channel.name.toLowerCase(): channel.id,
  };
  final channelNamesKey = channelNames.entries
      .map((entry) => '${entry.key}\u0000${entry.value}')
      .join('\u0001');
  useEffect(() {
    controller.setChannelNames(channelNames);
    return null;
  }, [controller, channelNamesKey]);
}

const _typingThrottleMs = 3000;

class _ComposerKeyboardMetricsObserver with WidgetsBindingObserver {
  final FlutterView view;
  final VoidCallback onKeyboardShown;
  final VoidCallback onKeyboardHidden;
  bool _wasVisible;

  _ComposerKeyboardMetricsObserver({
    required this.view,
    required this.onKeyboardShown,
    required this.onKeyboardHidden,
  }) : _wasVisible = view.viewInsets.bottom > 0;

  @override
  void didChangeMetrics() {
    final isVisible = view.viewInsets.bottom > 0;
    if (!_wasVisible && isVisible) onKeyboardShown();
    if (_wasVisible && !isVisible) onKeyboardHidden();
    _wasVisible = isVisible;
  }
}

void _runComposerAction(VoidCallback action) {
  unawaited(HapticFeedback.selectionClick());
  action();
}

void _showComposerEmojiPicker(
  BuildContext context,
  ValueChanged<String> onSelect,
  VoidCallback onDismiss,
) {
  showEmojiPicker(
    context: context,
    onSelect: (emoji) => _runComposerAction(() => onSelect(emoji)),
    onDismiss: onDismiss,
  );
}

void _dismissComposerKeyboard(FocusNode focusNode) {
  focusNode.unfocus();
  unawaited(SystemChannels.textInput.invokeMethod<void>('TextInput.hide'));
}

void _chooseComposerAttachment(
  BuildContext context,
  ValueNotifier<_AttachmentSurface> attachmentSurface,
  ValueNotifier<String?> uploadError,
  Future<void> Function() choose, {
  String? errorMessage,
}) {
  attachmentSurface.value = _AttachmentSurface.closed;
  unawaited(() async {
    try {
      await choose();
    } catch (error) {
      if (context.mounted) {
        uploadError.value = errorMessage ?? _formatUploadError(error);
      }
    }
  }());
}

Duration _composerMotionDuration(
  bool reducedMotion,
  _AttachmentSurface surface,
) => reducedMotion
    ? Duration.zero
    : Duration(
        milliseconds:
            surface == _AttachmentSurface.camera ||
                surface == _AttachmentSurface.photos
            ? 320
            : 250,
      );

void _expandComposer({
  required BuildContext context,
  required ValueNotifier<bool> isExpanded,
  required ValueNotifier<_AttachmentSurface> attachmentSurface,
  required VoidCallback? onFocusRequested,
  required FocusNode focusNode,
  required FlutterView view,
  required ValueNotifier<bool> androidImeTransitionStarted,
  required ObjectRef<Timer?> androidImeFallbackTimer,
}) {
  if (isExpanded.value) return;
  attachmentSurface.value = _AttachmentSurface.closed;
  onFocusRequested?.call();
  isExpanded.value = true;
  // Attach the editor before requesting focus so native restoration cannot
  // reopen a composer behind a popped route.
  WidgetsBinding.instance.addPostFrameCallback((_) {
    if (context.mounted && isExpanded.value) focusNode.requestFocus();
  });
  if (defaultTargetPlatform != TargetPlatform.android) return;
  androidImeFallbackTimer.value?.cancel();
  if (view.viewInsets.bottom > 0) {
    androidImeTransitionStarted.value = true;
    return;
  }
  androidImeTransitionStarted.value = false;
  androidImeFallbackTimer.value = Timer(const Duration(milliseconds: 250), () {
    if (context.mounted && isExpanded.value) {
      androidImeTransitionStarted.value = true;
    }
  });
}

Widget _composerSuggestionPanel({
  required List<Channel> channelSuggestions,
  required List<MentionCandidate> mentionSuggestions,
  required Map<String, UserProfile> userCache,
  required String? currentPubkey,
  required bool isDmChannel,
  required ValueChanged<Channel> onChannelSelect,
  required ValueChanged<MentionCandidate> onMentionSelect,
  required _ThreadAgentPins agentPins,
  required ValueChanged<MentionCandidate> onToggleAgentPin,
}) => channelSuggestions.isNotEmpty
    ? KeyedSubtree(
        key: const ValueKey('channel-suggestions'),
        child: _ChannelSuggestions(
          suggestions: channelSuggestions,
          onSelect: onChannelSelect,
        ),
      )
    : mentionSuggestions.isNotEmpty
    ? KeyedSubtree(
        key: const ValueKey('mention-suggestions'),
        child: _MentionSuggestions(
          suggestions: mentionSuggestions,
          userCache: userCache,
          currentPubkey: currentPubkey,
          isDmChannel: isDmChannel,
          onSelect: onMentionSelect,
          agentPins: agentPins.active ? agentPins : null,
          onTogglePin: onToggleAgentPin,
        ),
      )
    : const SizedBox.shrink(key: ValueKey('no-suggestions'));

Widget _composerAttachmentPanel({
  required _AttachmentSurface surface,
  required Widget suggestionPanel,
  required VoidCallback onBack,
  required VoidCallback onCamera,
  required VoidCallback onPhotos,
  required VoidCallback onVideo,
  required VoidCallback onVoiceNote,
  required VoidCallback onFiles,
  required Future<void> Function(XFile image) onCapture,
  required Future<List<XFile>> Function() onPickAllPhotos,
  required Future<void> Function(List<XFile> photos) onChoosePhotos,
  required Future<void> Function(List<XFile> photos) onChooseAllPhotos,
}) => _AttachmentSurfacePanel(
  key: ValueKey(
    surface == _AttachmentSurface.closed
        ? 'composer-suggestions'
        : 'attachment-surface',
  ),
  surface: surface,
  suggestionPanel: suggestionPanel,
  onBack: onBack,
  onCamera: onCamera,
  onPhotos: onPhotos,
  onVideo: onVideo,
  onVoiceNote: onVoiceNote,
  onFiles: onFiles,
  onCapture: onCapture,
  onPickAllPhotos: onPickAllPhotos,
  onChoosePhotos: onChoosePhotos,
  onChooseAllPhotos: onChooseAllPhotos,
);

const _pastedImageMimeTypes = <String>[
  'image/jpeg',
  'image/jpg',
  'image/png',
  'image/webp',
];

/// Cap on ranked mention suggestions shown — matches desktop's
/// `MENTION_SUGGESTION_LIMIT`.
const _mentionSuggestionLimit = 50;

/// Walk backward from [cursor] looking for [trigger] (e.g. `@` or `#`) at a
/// word boundary. Returns the index of the trigger character, or `null` if none
/// is found.
///
/// When [stopAtSpace] is `true` the walk stops at both spaces and newlines —
/// appropriate for `#channel` names which are kebab-case slugs without spaces.
/// When `false`, only newlines stop the walk, allowing multi-word queries like
/// `@Alice Smith` to match members with multi-word display names.
@visibleForTesting
int? findTrigger(
  String text,
  int cursor,
  String trigger, {
  bool stopAtSpace = true,
}) {
  for (var i = cursor - 1; i >= 0; i--) {
    final ch = text[i];
    if (ch == '\n') break;
    if (stopAtSpace && ch == ' ') break;
    if (ch == trigger) {
      if (i == 0 || text[i - 1] == ' ' || text[i - 1] == '\n') {
        return i;
      }
      break;
    }
  }
  return null;
}

/// Replace the range `[start, cursor)` with [replacement] and move the cursor
/// to the end of the replacement. Used by both mention and channel insertion.
@visibleForTesting
void spliceAndMoveCursor(
  TextEditingController controller,
  FocusNode focusNode, {
  required int start,
  required String replacement,
}) {
  final text = controller.text;
  final cursor =
      (controller.selection.isValid
              ? controller.selection.baseOffset
              : text.length)
          .clamp(start, text.length);

  final before = text.substring(0, start);
  final after = text.substring(cursor);
  controller.value = TextEditingValue(
    text: '$before$replacement$after',
    selection: TextSelection.collapsed(offset: start + replacement.length),
  );
  focusNode.requestFocus();
}

/// Insert [trigger] (e.g. `@` or `#`) at the cursor position, prefixed with
/// a space if needed for word separation. Used by `triggerMention` and
/// `triggerChannel`.
void _insertTriggerAtCursor(
  TextEditingController controller,
  FocusNode focusNode,
  String trigger,
) {
  final text = controller.text;
  final cursor = controller.selection.isValid
      ? controller.selection.baseOffset
      : text.length;
  final needsSpace =
      cursor > 0 && text[cursor - 1] != ' ' && text[cursor - 1] != '\n';
  final insert = needsSpace ? ' $trigger' : trigger;
  final before = text.substring(0, cursor);
  final after = text.substring(cursor);
  controller.value = TextEditingValue(
    text: '$before$insert$after',
    selection: TextSelection.collapsed(offset: cursor + insert.length),
  );
  focusNode.requestFocus();
}

bool hasMention(String text, String name) {
  final pattern = RegExp(
    '(?:^|\\s|[*_]{1,3}|\\|\\|)@${RegExp.escape(name)}(?=\\|\\||[\\s,;.!?:)\\]}*_]|\$)',
    caseSensitive: false,
  );
  return pattern.hasMatch(text);
}

/// User-facing text for a failed add or send.
String _composeSendErrorMessage(Object error) {
  if (error is AddMembersException) return error.message;
  return error.toString().replaceFirst('Exception: ', '');
}

/// Send a typing indicator over the WebSocket (fire-and-forget).
///
/// Desktop sends these as `["EVENT", signedEvent]` over the WebSocket — not
/// via HTTP. Ephemeral events like typing indicators are broadcast-only and
/// the relay doesn't persist them, so the HTTP `/api/events` endpoint may
/// silently discard them.
void _sendTypingIndicator(
  WidgetRef ref, {
  required String channelId,
  String? threadHeadId,
  String? rootId,
}) {
  try {
    final config = ref.read(relayConfigProvider);
    if (outgoingAuthorPubkey(config) == null) return;

    final tags = <List<String>>[
      ['h', channelId],
      if (threadHeadId != null && rootId != null && rootId != threadHeadId) ...[
        ['e', rootId, '', 'root'],
        ['e', threadHeadId, '', 'reply'],
      ] else if (threadHeadId != null)
        ['e', threadHeadId, '', 'reply'],
    ];

    // Signed on a legacy community, an unsigned draft on a token one.
    final event = buildOutgoingEvent(
      config,
      kind: EventKind.typingIndicator,
      content: '',
      tags: tags,
    );

    // Send directly over WebSocket — fire-and-forget, matching desktop.
    final session = ref.read(relaySessionProvider.notifier);
    session.sendRaw(['EVENT', event.toJson()]);
  } catch (_) {
    // Fire-and-forget — typing indicator failure is non-fatal.
  }
}

/// Reports a send that was cancelled because the active community changed.
///
/// The send path is fire-and-forget, so a `StateError` escaping it would be
/// silent. The composer's own error line cannot carry this message either: the
/// identity change that causes the failure also resets that state on the next
/// frame, so [messenger] must be resolved before the send's first `await`.
void _reportSendCancelledByCommunitySwitch(ScaffoldMessengerState? messenger) {
  messenger?.showSnackBar(
    const SnackBar(content: Text('Message not sent: the community changed')),
  );
}
