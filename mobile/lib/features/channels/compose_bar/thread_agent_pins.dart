part of '../compose_bar.dart';

const _autoMentionConfirmationDuration = Duration(seconds: 3);

/// Thread-reply automatic agent mentions, matching desktop's
/// `useThreadAgentAudience` + `useAgentAddressLockPicker`: the thread root's
/// agents and agents the user mentions in the thread are inserted as
/// `@Name` at the start of every reply until removed.
class _ThreadAgentPins {
  final bool active;
  final bool enabled;
  final List<MentionCandidate> pinnedAgents;
  final Set<String> pinnedPubkeys;
  final VoidCallback restore;
  final void Function(VoidCallback clear) clearAndRestore;
  final ValueChanged<MentionCandidate> onInlineAgentSelected;
  final ValueChanged<MentionCandidate> togglePin;
  final ValueChanged<String> remove;
  final ValueChanged<bool> setEnabled;

  /// "X will be mentioned automatically", shown above the composer briefly.
  final String? confirmationTitle;
  final VoidCallback turnOffConfirmation;

  const _ThreadAgentPins({
    required this.active,
    required this.enabled,
    required this.pinnedAgents,
    required this.pinnedPubkeys,
    required this.restore,
    required this.clearAndRestore,
    required this.onInlineAgentSelected,
    required this.togglePin,
    required this.remove,
    required this.setEnabled,
    required this.confirmationTitle,
    required this.turnOffConfirmation,
  });
}

_ThreadAgentPins _useThreadAgentPins({
  required BuildContext context,
  required WidgetRef ref,
  required String? scope,
  required List<List<String>> rootTags,
  required _MarkdownEditingController controller,
  required ObjectRef<Map<String, MentionCandidate>> mentionMap,
  required ObjectRef<Set<String>> implicitPubkeys,
  required ObjectRef<bool> isModifyingText,
  required MentionCandidate Function(String pubkey) agentCandidate,
}) {
  final enabled = ref.watch(keepMentionedAgentsPinnedProvider);
  final knownAgents = ref.watch(knownAgentPubkeysProvider);
  final pubkeys = ref.watch(
    threadAgentAudienceProvider.select((state) => state.pubkeysFor(scope)),
  );
  final audience = ref.read(threadAgentAudienceProvider.notifier);
  final pubkeysKey = pubkeys.join(',');
  final rootAgents = [
    for (final tag in rootTags)
      if (tag.length > 1 &&
          tag[0] == 'p' &&
          knownAgents.contains(tag[1].toLowerCase()))
        tag[1].toLowerCase(),
  ];
  final rootAgentsKey = rootAgents.join(',');

  // Agents removed in this composer stay out until explicitly re-pinned.
  final sessionUnpinned = useMemoized(() => <String>{}, [scope]);
  // Pinned agents whose mention is currently visible in the draft.
  final visible = useMemoized(() => <String>{}, [scope]);
  final suspendSync = useRef(false);
  useEffect(() {
    implicitPubkeys.value.clear();
    return null;
  }, [scope]);
  // Inline above the composer so it never covers the input; it belongs to
  // the scope that raised it and clears itself.
  final confirmation = useState<({String scope, String title})?>(null);
  useEffect(() {
    if (confirmation.value == null) return null;
    final timer = Timer(_autoMentionConfirmationDuration, () {
      if (context.mounted) confirmation.value = null;
    });
    return timer.cancel;
  }, [confirmation.value]);
  final latest = useRef(pubkeys)..value = pubkeys;

  String labelFor(String pubkey) {
    for (final entry in mentionMap.value.entries) {
      if (entry.value.pubkey.toLowerCase() == pubkey) return entry.key;
    }
    final candidate = agentCandidate(pubkey);
    final label = selectedMentionLabel(candidate.label, pubkey, {
      for (final entry in mentionMap.value.entries)
        entry.key: entry.value.pubkey,
    });
    mentionMap.value[label] = candidate;
    return label;
  }

  Set<String> presentPubkeys(String text) {
    final labels = {
      for (final entry in mentionMap.value.entries)
        entry.key: entry.value.pubkey.toLowerCase(),
    };
    return {
      for (final range in mentionOccurrences(text, labels.keys))
        labels[range.label]!,
    };
  }

  void replaceText(TextEditingValue value) {
    isModifyingText.value = true;
    try {
      controller.value = value;
    } finally {
      isModifyingText.value = false;
    }
  }

  void insertPrefix(List<String> targets) {
    if (targets.isEmpty) return;
    final prefix = '${targets.map((p) => '@${labelFor(p)}').join(' ')} ';
    final current = controller.value;
    final selection = current.selection;
    implicitPubkeys.value.addAll(targets);
    replaceText(
      TextEditingValue(
        text: '$prefix${current.text}',
        selection: !selection.isValid || current.text.isEmpty
            ? TextSelection.collapsed(
                offset: prefix.length + current.text.length,
              )
            : TextSelection(
                baseOffset: selection.baseOffset + prefix.length,
                extentOffset: selection.extentOffset + prefix.length,
              ),
      ),
    );
    visible.addAll(targets);
  }

  void restore() {
    if (scope == null) return;
    final present = presentPubkeys(controller.text);
    final missing = <String>[];
    for (final pubkey in latest.value) {
      if (present.contains(pubkey)) {
        visible.add(pubkey);
      } else if (!sessionUnpinned.contains(pubkey)) {
        missing.add(pubkey);
      }
    }
    insertPrefix(missing);
  }

  // Desktop seeds the thread from the root message's agent `p` tags. Hook
  // effects run during build, where providers must not be modified.
  useEffect(() {
    if (scope == null || !enabled || rootAgents.isEmpty) return null;
    var cancelled = false;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!cancelled) audience.initialize(scope, rootAgents);
    });
    return () => cancelled = true;
  }, [scope, enabled, rootAgentsKey]);

  final latestRestore = useRef(restore)..value = restore;
  useEffect(() {
    if (scope == null || pubkeys.isEmpty) return null;
    var cancelled = false;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!cancelled) latestRestore.value();
    });
    return () => cancelled = true;
  }, [scope, pubkeysKey]);

  // Deleting a pinned agent's mention stops automatically mentioning it.
  useEffect(() {
    if (scope == null) return null;
    void listener() {
      if (suspendSync.value) return;
      final present = presentPubkeys(controller.text);
      // Draft hydration replaces text during build; only user edits count.
      final building =
          SchedulerBinding.instance.schedulerPhase ==
          SchedulerPhase.persistentCallbacks;
      for (final pubkey in building ? const <String>[] : [...visible]) {
        if (!present.contains(pubkey) && latest.value.contains(pubkey)) {
          audience.exclude(scope, pubkey);
        }
      }
      implicitPubkeys.value.retainWhere(present.contains);
      visible
        ..clear()
        ..addAll(present);
    }

    controller.addListener(listener);
    return () => controller.removeListener(listener);
  }, [controller, scope]);

  void confirm(List<String> promoted) {
    if (promoted.isEmpty) return;
    final name = promoted.length == 1
        ? agentCandidate(promoted.single).displayName?.trim()
        : null;
    final title = name != null && name.isNotEmpty
        ? '$name will be mentioned automatically'
        : promoted.length == 1
        ? 'Agent will be mentioned automatically'
        : '${promoted.length} agents will be mentioned automatically';
    if (scope != null) confirmation.value = (scope: scope, title: title);
  }

  void stripLeadingMention(String pubkey) {
    final text = controller.text;
    final first = mentionOccurrences(text, mentionMap.value.keys).firstOrNull;
    if (first == null ||
        first.start != 0 ||
        mentionMap.value[first.label]?.pubkey.toLowerCase() != pubkey) {
      return;
    }
    final end = first.end < text.length && text[first.end] == ' '
        ? first.end + 1
        : first.end;
    final selection = controller.selection;
    int shift(int offset) =>
        offset <= first.start ? offset : math.max(0, offset - end);
    implicitPubkeys.value.remove(pubkey);
    suspendSync.value = true;
    try {
      replaceText(
        TextEditingValue(
          text: text.substring(end),
          selection: selection.isValid
              ? TextSelection(
                  baseOffset: shift(selection.baseOffset),
                  extentOffset: shift(selection.extentOffset),
                )
              : const TextSelection.collapsed(offset: 0),
        ),
      );
    } finally {
      suspendSync.value = false;
    }
  }

  void remove(String pubkey) {
    if (scope == null) return;
    final normalized = pubkey.toLowerCase();
    sessionUnpinned.add(normalized);
    visible.remove(normalized);
    audience.exclude(scope, normalized);
    stripLeadingMention(normalized);
  }

  void setEnabled(bool value) => unawaited(
    ref.read(keepMentionedAgentsPinnedProvider.notifier).setEnabled(value),
  );

  return _ThreadAgentPins(
    active: scope != null,
    enabled: enabled,
    pinnedAgents: [for (final pubkey in pubkeys) agentCandidate(pubkey)],
    pinnedPubkeys: pubkeys.toSet(),
    restore: restore,
    clearAndRestore: (clear) {
      suspendSync.value = true;
      implicitPubkeys.value.clear();
      try {
        clear();
        visible.clear();
      } finally {
        suspendSync.value = false;
      }
      restore();
    },
    onInlineAgentSelected: (candidate) {
      if (scope == null || !candidate.isAgent || !enabled) return;
      final pubkey = candidate.pubkey.toLowerCase();
      final wasUnpinned =
          !latest.value.contains(pubkey) && sessionUnpinned.contains(pubkey);
      visible.add(pubkey);
      confirm(
        audience.promote(scope, [pubkey], reinstateExcluded: !wasUnpinned),
      );
    },
    togglePin: (candidate) {
      if (scope == null || !candidate.isAgent) return;
      final pubkey = candidate.pubkey.toLowerCase();
      if (latest.value.contains(pubkey)) {
        remove(pubkey);
        return;
      }
      sessionUnpinned.remove(pubkey);
      if (!presentPubkeys(controller.text).contains(pubkey)) {
        mentionMap.value.putIfAbsent(
          selectedMentionLabel(candidate.label, pubkey, {
            for (final entry in mentionMap.value.entries)
              entry.key: entry.value.pubkey,
          }),
          () => candidate,
        );
        insertPrefix([pubkey]);
      }
      visible.add(pubkey);
      confirm(audience.promote(scope, [pubkey], reinstateExcluded: true));
      setEnabled(true);
    },
    remove: remove,
    setEnabled: setEnabled,
    confirmationTitle: confirmation.value?.scope == scope
        ? confirmation.value?.title
        : null,
    turnOffConfirmation: () {
      confirmation.value = null;
      setEnabled(false);
    },
  );
}

/// Desktop's auto-pin confirmation popover, as a row above the composer.
class _AutoMentionConfirmation extends StatelessWidget {
  final String title;
  final VoidCallback onTurnOff;

  const _AutoMentionConfirmation({
    required this.title,
    required this.onTurnOff,
  });

  @override
  Widget build(BuildContext context) {
    return Semantics(
      liveRegion: true,
      child: Padding(
        key: const ValueKey('composer-auto-pin-confirmation'),
        padding: const EdgeInsets.symmetric(horizontal: Grid.xs),
        child: Row(
          children: [
            Expanded(
              child: Text(
                title,
                style: context.textTheme.labelMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            TextButton(
              style: TextButton.styleFrom(
                visualDensity: VisualDensity.compact,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
              ),
              onPressed: onTurnOff,
              child: const Text('Turn off'),
            ),
          ],
        ),
      ),
    );
  }
}

/// Pinned agents shown above a thread reply; tapping one stops automatically
/// mentioning it in this thread (desktop's composer address avatars).
class _ThreadAgentPinChips extends StatelessWidget {
  final List<MentionCandidate> agents;
  final Map<String, UserProfile> userCache;
  final ValueChanged<String> onRemove;

  const _ThreadAgentPinChips({
    required this.agents,
    required this.userCache,
    required this.onRemove,
  });

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      key: const ValueKey('composer-address-locks'),
      height: 32,
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: Grid.xs),
        children: [
          Padding(
            padding: const EdgeInsets.only(right: Grid.xxs),
            child: Icon(
              LucideIcons.atSign,
              size: 14,
              color: context.colors.primary,
            ),
          ),
          for (final agent in agents)
            Padding(
              padding: const EdgeInsets.only(right: Grid.xxs),
              child: Semantics(
                button: true,
                label:
                    "Don't automatically mention ${agent.label} in this thread",
                excludeSemantics: true,
                child: InputChip(
                  key: ValueKey('composer-address-lock-${agent.pubkey}'),
                  visualDensity: VisualDensity.compact,
                  materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  avatar: AvatarImage(
                    imageUrl:
                        agent.avatarUrl ?? userCache[agent.pubkey]?.avatarUrl,
                    radius: 9,
                    backgroundColor: context.colors.primaryContainer,
                    fallback: Text(
                      agent.initial,
                      style: context.textTheme.labelSmall,
                    ),
                    isAgent: true,
                  ),
                  label: Text(agent.label),
                  onDeleted: () => onRemove(agent.pubkey),
                  deleteIcon: const Icon(LucideIcons.x, size: 14),
                  onPressed: () => onRemove(agent.pubkey),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
