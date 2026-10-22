part of '../channels_page.dart';

const int _defaultCreateChannelTtlSeconds = 7 * 24 * 60 * 60;

class _CreateChannelMenuOption<T> {
  final Key? key;
  final String label;
  final T value;

  const _CreateChannelMenuOption({
    this.key,
    required this.label,
    required this.value,
  });
}

const _createChannelTtlOptions = [
  _CreateChannelMenuOption(label: '30 minutes', value: 30 * 60),
  _CreateChannelMenuOption(label: '1 hour', value: 60 * 60),
  _CreateChannelMenuOption(label: '6 hours', value: 6 * 60 * 60),
  _CreateChannelMenuOption(label: '12 hours', value: 12 * 60 * 60),
  _CreateChannelMenuOption(label: '1 day', value: 24 * 60 * 60),
  _CreateChannelMenuOption(label: '3 days', value: 3 * 24 * 60 * 60),
  _CreateChannelMenuOption(
    label: '7 days',
    value: _defaultCreateChannelTtlSeconds,
  ),
  _CreateChannelMenuOption(label: '14 days', value: 14 * 24 * 60 * 60),
  _CreateChannelMenuOption(label: '30 days', value: 30 * 24 * 60 * 60),
];

class _CreateChannelSheet extends HookConsumerWidget {
  final String channelType;

  const _CreateChannelSheet({required this.channelType});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final nameController = useTextEditingController();
    final descriptionController = useTextEditingController();
    final visibility = useState('open');
    final temporary = useState(false);
    final ttlSeconds = useState(_defaultCreateChannelTtlSeconds);
    final isSubmitting = useState(false);
    final errorMessage = useState<String?>(null);
    useListenable(nameController);

    final kindLabel = channelType == 'forum' ? 'forum' : 'group';
    final canSubmit =
        nameController.text.trim().isNotEmpty && !isSubmitting.value;

    Future<void> submit() async {
      final name = nameController.text.trim();
      if (name.isEmpty || isSubmitting.value) {
        return;
      }

      isSubmitting.value = true;
      errorMessage.value = null;
      try {
        final created = await ref
            .read(channelActionsProvider)
            .createChannel(
              name: name,
              channelType: channelType,
              visibility: visibility.value,
              description: descriptionController.text.trim(),
              ttlSeconds: temporary.value ? ttlSeconds.value : null,
            );
        if (context.mounted) {
          Navigator.of(context).pop(created);
        }
      } catch (error) {
        errorMessage.value = error.toString();
      } finally {
        isSubmitting.value = false;
      }
    }

    return Padding(
      padding: EdgeInsets.fromLTRB(
        Grid.gutter,
        0,
        Grid.gutter,
        MediaQuery.viewInsetsOf(context).bottom + Grid.xs,
      ),
      child: SafeArea(
        top: false,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _CreateChannelFieldLabel(label: 'Name'),
              const SizedBox(height: Grid.xxs),
              _CreateChannelFieldShell(
                child: TextField(
                  key: const Key('create-channel-name'),
                  controller: nameController,
                  enabled: !isSubmitting.value,
                  autofocus: true,
                  autocorrect: false,
                  enableSuggestions: false,
                  textCapitalization: TextCapitalization.none,
                  decoration: InputDecoration(
                    hintText: channelType == 'forum'
                        ? 'design-discussions'
                        : 'release-notes',
                    border: InputBorder.none,
                    enabledBorder: InputBorder.none,
                    focusedBorder: InputBorder.none,
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: Grid.twelve,
                      vertical: Grid.xs,
                    ),
                  ),
                  textInputAction: TextInputAction.done,
                  onSubmitted: (_) {
                    if (canSubmit) {
                      unawaited(submit());
                    }
                  },
                ),
              ),
              const SizedBox(height: Grid.xs),
              const _CreateChannelFieldLabel(
                label: 'Description',
                optional: true,
              ),
              const SizedBox(height: Grid.xxs),
              _CreateChannelFieldShell(
                child: TextField(
                  key: const Key('create-channel-description'),
                  controller: descriptionController,
                  enabled: !isSubmitting.value,
                  decoration: InputDecoration(
                    hintText: 'What this $kindLabel is for',
                    border: InputBorder.none,
                    enabledBorder: InputBorder.none,
                    focusedBorder: InputBorder.none,
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: Grid.twelve,
                      vertical: Grid.xs,
                    ),
                  ),
                  minLines: 2,
                  maxLines: 3,
                  textInputAction: TextInputAction.done,
                  onSubmitted: (_) {
                    if (canSubmit) {
                      unawaited(submit());
                    }
                  },
                ),
              ),
              const SizedBox(height: Grid.sm),
              _CreateChannelRadioGroup<bool>(
                enabled: !isSubmitting.value,
                label: 'Group type',
                onSelected: (value) => temporary.value = value,
                options: const [
                  _CreateChannelMenuOption(
                    key: Key('create-channel-type-ongoing'),
                    label: 'Ongoing',
                    value: false,
                  ),
                  _CreateChannelMenuOption(
                    key: Key('create-channel-type-temporary'),
                    label: 'Temporary',
                    value: true,
                  ),
                ],
                value: temporary.value,
              ),
              if (temporary.value) ...[
                const SizedBox(height: Grid.xs),
                _CreateChannelSettingMenu<int>(
                  controlKey: const Key('create-channel-ttl'),
                  enabled: !isSubmitting.value,
                  label: 'Expires after',
                  onSelected: (value) => ttlSeconds.value = value,
                  options: _createChannelTtlOptions,
                  value: ttlSeconds.value,
                  valueLabel: _createChannelTtlOptions
                      .firstWhere((option) => option.value == ttlSeconds.value)
                      .label,
                ),
              ],
              const SizedBox(height: Grid.sm),
              _CreateChannelRadioGroup<String>(
                enabled: !isSubmitting.value,
                label: 'Visibility',
                onSelected: (value) => visibility.value = value,
                options: const [
                  _CreateChannelMenuOption(
                    key: Key('create-channel-visibility-public'),
                    label: 'Public',
                    value: 'open',
                  ),
                  _CreateChannelMenuOption(
                    key: Key('create-channel-visibility-private'),
                    label: 'Private',
                    value: 'private',
                  ),
                ],
                value: visibility.value,
              ),
              if (errorMessage.value case final error?) ...[
                const SizedBox(height: Grid.xs),
                Text(
                  error,
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.error,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _CreateChannelFieldLabel extends StatelessWidget {
  final String label;
  final bool optional;

  const _CreateChannelFieldLabel({required this.label, this.optional = false});

  @override
  Widget build(BuildContext context) {
    return Text.rich(
      TextSpan(
        children: [
          TextSpan(text: label),
          if (optional)
            TextSpan(
              text: '  Optional',
              style: context.textTheme.labelSmall?.copyWith(
                color: context.colors.onSurfaceVariant.withValues(alpha: 0.6),
                fontWeight: FontWeight.w400,
              ),
            ),
        ],
      ),
      style: context.textTheme.labelLarge?.copyWith(
        color: context.colors.onSurface,
        fontWeight: FontWeight.w600,
      ),
    );
  }
}

class _CreateChannelFieldShell extends StatelessWidget {
  final Widget child;

  const _CreateChannelFieldShell({required this.child});

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: context.colors.primaryContainer.withValues(alpha: 0.55),
        border: Border.all(color: context.colors.outlineVariant),
        borderRadius: BorderRadius.circular(Radii.lg),
      ),
      child: child,
    );
  }
}

class _CreateChannelRadioGroup<T> extends StatelessWidget {
  final bool enabled;
  final String label;
  final ValueChanged<T> onSelected;
  final List<_CreateChannelMenuOption<T>> options;
  final T value;

  const _CreateChannelRadioGroup({
    required this.enabled,
    required this.label,
    required this.onSelected,
    required this.options,
    required this.value,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _CreateChannelFieldLabel(label: label),
        const SizedBox(height: Grid.xxs),
        DecoratedBox(
          decoration: BoxDecoration(
            color: context.colors.surface,
            border: Border.all(color: context.colors.outlineVariant),
            borderRadius: BorderRadius.circular(Radii.lg),
          ),
          child: RadioGroup<T>(
            groupValue: value,
            onChanged: (nextValue) {
              if (enabled && nextValue != null) {
                onSelected(nextValue);
              }
            },
            child: Column(
              children: [
                for (final (index, option) in options.indexed) ...[
                  RadioListTile<T>(
                    key: option.key,
                    value: option.value,
                    enabled: enabled,
                    selected: option.value == value,
                    controlAffinity: ListTileControlAffinity.leading,
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: Grid.xs,
                      vertical: Grid.half,
                    ),
                    visualDensity: VisualDensity.standard,
                    activeColor: context.colors.primary,
                    title: Text(
                      option.label,
                      style: context.textTheme.bodyMedium?.copyWith(
                        color: enabled
                            ? context.colors.onSurface
                            : context.colors.onSurface.withValues(alpha: 0.38),
                        fontWeight: option.value == value
                            ? FontWeight.w600
                            : FontWeight.w400,
                      ),
                    ),
                  ),
                  if (index < options.length - 1)
                    Divider(
                      height: 1,
                      indent: Grid.xs,
                      endIndent: Grid.xs,
                      color: context.colors.outlineVariant,
                    ),
                ],
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _CreateChannelSettingMenu<T> extends StatelessWidget {
  final Key controlKey;
  final bool enabled;
  final String label;
  final ValueChanged<T> onSelected;
  final List<_CreateChannelMenuOption<T>> options;
  final T value;
  final IconData? valueIcon;
  final String valueLabel;

  const _CreateChannelSettingMenu({
    required this.controlKey,
    required this.enabled,
    required this.label,
    required this.onSelected,
    required this.options,
    required this.value,
    this.valueIcon,
    required this.valueLabel,
  });

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: context.colors.surface,
        border: Border.all(color: context.colors.outlineVariant),
        borderRadius: BorderRadius.circular(Radii.lg),
      ),
      child: PopupMenuButton<T>(
        enabled: enabled,
        initialValue: value,
        onSelected: onSelected,
        tooltip: '$label: $valueLabel',
        itemBuilder: (context) => [
          for (final option in options)
            CheckedPopupMenuItem<T>(
              checked: option.value == value,
              value: option.value,
              child: Text(option.label),
            ),
        ],
        child: Padding(
          key: controlKey,
          padding: const EdgeInsets.symmetric(
            horizontal: Grid.twelve,
            vertical: Grid.xs,
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  label,
                  style: context.textTheme.labelLarge?.copyWith(
                    color: context.colors.onSurface,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              if (valueIcon case final icon?) ...[
                Icon(icon, size: 16, color: context.colors.onSurfaceVariant),
                const SizedBox(width: Grid.half),
              ],
              Text(
                valueLabel,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurface,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(width: Grid.half),
              Icon(
                LucideIcons.chevronDown,
                size: 16,
                color: context.colors.onSurfaceVariant,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _NewDirectMessageSheet extends HookConsumerWidget {
  final String? currentPubkey;

  const _NewDirectMessageSheet({required this.currentPubkey});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final queryController = useTextEditingController();
    final queryFocusNode = useFocusNode();
    final query = useState('');
    final debouncedQuery = useState('');
    final selectedUsers = useState<List<DirectoryUser>>([]);
    final chatNameController = useTextEditingController();
    final showAllSelected = useState(false);
    final isSubmitting = useState(false);
    final submitError = useState<String?>(null);
    final previewDirectoryActive = ref.watch(dmDirectoryPreviewEnabledProvider);

    useEffect(() {
      final timer = Timer(const Duration(milliseconds: 250), () {
        debouncedQuery.value = query.value.trim().toLowerCase();
      });
      return timer.cancel;
    }, [query.value]);

    final normalizedQuery = previewDirectoryActive
        ? query.value.trim().toLowerCase()
        : debouncedQuery.value;
    final isSearchTransitionPending =
        !previewDirectoryActive &&
        query.value.trim().toLowerCase() != normalizedQuery;
    final directoryAsync = previewDirectoryActive
        ? AsyncValue.data(dmDirectoryPreviewUsers)
        : normalizedQuery.isEmpty
        ? ref.watch(relayDirectoryUsersProvider)
        : ref.watch(relayDirectorySearchProvider(normalizedQuery));

    final selectedPubkeys = selectedUsers.value
        .map((user) => user.pubkey.toLowerCase())
        .toSet();
    final blockedPubkeys = ref.watch(blockedPubkeysProvider);
    final knownAgents = ref.watch(peopleAgentPubkeysProvider);
    final friendIds = previewDirectoryActive
        ? const <String>{}
        : ref.watch(friendIdsProvider).value ?? const <String>{};
    final cache = ref.watch(userCacheProvider);
    final me = currentPubkey?.toLowerCase();

    useEffect(() {
      if (friendIds.isNotEmpty) {
        unawaited(
          ref.read(userCacheProvider.notifier).preload(friendIds.toList()),
        );
      }
      return null;
    }, [Object.hashAllUnordered(friendIds)]);

    bool eligible(String pubkey) {
      final pk = pubkey.toLowerCase();
      return !selectedPubkeys.contains(pk) &&
          !blockedPubkeys.contains(pk) &&
          pk != me;
    }

    bool matches(DirectoryUser user) {
      if (normalizedQuery.isEmpty) return true;
      return user.label.toLowerCase().contains(normalizedQuery) ||
          (user.nip05Handle?.toLowerCase().contains(normalizedQuery) ?? false);
    }

    final directoryUsers =
        directoryAsync.asData?.value ?? const <DirectoryUser>[];
    final directoryByPubkey = {
      for (final user in directoryUsers) user.pubkey.toLowerCase(): user,
    };
    bool isAgent(DirectoryUser user) =>
        user.isAgent || knownAgents.contains(user.pubkey.toLowerCase());

    // Only friends, people I share a channel with, and agents can be found
    // here; the whole member directory is never listed.
    final List<DirectoryUser> friendResults;
    final List<DirectoryUser> agentResults;
    if (previewDirectoryActive) {
      friendResults = [
        for (final user in directoryUsers)
          if (eligible(user.pubkey) && matches(user)) user,
      ];
      agentResults = const [];
    } else {
      friendResults =
          [
            for (final pk in friendIds)
              if (eligible(pk) &&
                  !knownAgents.contains(pk) &&
                  !(directoryByPubkey[pk]?.isAgent ?? false) &&
                  !(cache[pk]?.isAgent ?? false))
                directoryByPubkey[pk] ??
                    DirectoryUser(
                      pubkey: pk,
                      displayName: cache[pk]?.displayName,
                      avatarUrl: cache[pk]?.avatarUrl,
                      nip05Handle: cache[pk]?.nip05Handle,
                    ),
          ].where(matches).toList()..sort(
            (a, b) => a.label.toLowerCase().compareTo(b.label.toLowerCase()),
          );
      agentResults = [
        for (final user in directoryUsers)
          if (isAgent(user) && eligible(user.pubkey) && matches(user))
            DirectoryUser(
              pubkey: user.pubkey,
              displayName: user.displayName,
              avatarUrl: user.avatarUrl,
              nip05Handle: user.nip05Handle,
              isAgent: true,
            ),
      ];
    }
    final shown = {
      for (final user in [...friendResults, ...agentResults])
        user.pubkey.toLowerCase(),
    };
    final sharedAsync = previewDirectoryActive || normalizedQuery.isEmpty
        ? const AsyncValue<List<PersonSummary>>.data([])
        : ref.watch(sharedPeopleProvider(normalizedQuery));
    final sharedResults = [
      for (final person in sharedAsync.value ?? const <PersonSummary>[])
        if (eligible(person.pubkey) &&
            !shown.contains(person.pubkey) &&
            !knownAgents.contains(person.pubkey))
          DirectoryUser(
            pubkey: person.pubkey,
            displayName: person.displayName,
            avatarUrl: person.avatarUrl,
            nip05Handle: person.username == null ? null : '@${person.username}',
          ),
    ];
    final sharedPending =
        normalizedQuery.isNotEmpty &&
        sharedAsync.isLoading &&
        !sharedAsync.hasValue;
    final sharedFailed =
        normalizedQuery.isNotEmpty &&
        sharedAsync.hasError &&
        !sharedAsync.hasValue;
    final availableResults = [
      ...friendResults,
      ...sharedResults,
      ...agentResults,
    ];
    // Eligible recipients shown or chosen here are the comparison context.
    final recipients = [...availableResults, ...selectedUsers.value];
    final names = watchIdentityNames(
      ref,
      [for (final user in recipients) user.pubkey],
      agentPubkeys: {
        for (final user in recipients)
          if (user.isAgent) user.pubkey,
      },
      fallbackNames: {for (final user in recipients) user.pubkey: user.label},
    );
    final canSubmit = !isSubmitting.value && selectedUsers.value.isNotEmpty;
    // Many picked people fold into "+N" until tapped.
    const visibleSelectedLimit = 6;
    final collapseSelected =
        !showAllSelected.value &&
        selectedUsers.value.length > visibleSelectedLimit;
    final visibleSelected = collapseSelected
        ? selectedUsers.value.take(visibleSelectedLimit - 1).toList()
        : selectedUsers.value;
    final hiddenSelected = selectedUsers.value.length - visibleSelected.length;

    Future<void> submit() async {
      if (selectedUsers.value.isEmpty || isSubmitting.value) {
        return;
      }
      if (previewDirectoryActive) {
        submitError.value = 'Preview only — mock people cannot be messaged.';
        return;
      }

      isSubmitting.value = true;
      submitError.value = null;
      final pubkeys = selectedUsers.value.map((user) => user.pubkey).toList();
      try {
        // One person is a 1:1 DM; two or more start a chat of people.
        if (pubkeys.length == 1) {
          final channel = await ref
              .read(channelActionsProvider)
              .openDm(pubkeys: pubkeys);
          if (context.mounted) Navigator.of(context).pop(channel);
          return;
        }
        final messenger = ScaffoldMessenger.maybeOf(context);
        final result = await ref
            .read(channelActionsProvider)
            .createGroupChat(pubkeys: pubkeys, name: chatNameController.text);
        if (!context.mounted) return;
        Navigator.of(context).pop(result.channel);
        if (result.notice case final notice?) {
          messenger?.showSnackBar(SnackBar(content: Text(notice)));
        }
      } catch (error) {
        submitError.value = friendlyError(
          error,
          fallback: 'Couldn’t start the conversation. Try again.',
        );
      } finally {
        isSubmitting.value = false;
      }
    }

    Widget tileFor(DirectoryUser user) {
      final subtitle = user.isAgent
          ? user.secondaryLabel
          : (user.nip05Handle?.trim() ?? '');
      return ListTile(
        key: Key('new-dm-person-${user.pubkey}'),
        contentPadding: const EdgeInsets.symmetric(horizontal: Grid.half),
        leading: AvatarImage(
          imageUrl: user.avatarUrl,
          radius: 20,
          backgroundColor: context.colors.primaryContainer,
          fallback: Text(
            user.initial,
            style: context.textTheme.labelLarge?.copyWith(
              color: context.colors.onPrimaryContainer,
              fontWeight: FontWeight.w600,
            ),
          ),
          isAgent: user.isAgent,
        ),
        title: Text(
          names.labelFor(user.pubkey),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: subtitle.isEmpty || subtitle == user.label
            ? null
            : Text(subtitle, maxLines: 1, overflow: TextOverflow.ellipsis),
        trailing: Icon(
          LucideIcons.plus,
          size: 18,
          color: context.colors.onSurfaceVariant,
        ),
        onTap: isSubmitting.value
            ? null
            : () {
                selectedUsers.value = [...selectedUsers.value, user];
                queryController.clear();
                query.value = '';
                debouncedQuery.value = '';
                submitError.value = null;
                queryFocusNode.requestFocus();
              },
      );
    }

    return Padding(
      padding: EdgeInsets.fromLTRB(
        Grid.gutter,
        0,
        Grid.gutter,
        MediaQuery.viewInsetsOf(context).bottom + Grid.xs,
      ),
      child: SafeArea(
        top: false,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              GestureDetector(
                behavior: HitTestBehavior.translucent,
                onTap: isSubmitting.value ? null : queryFocusNode.requestFocus,
                child: SizedBox(
                  width: double.infinity,
                  child: ConstrainedBox(
                    key: const Key('new-dm-recipient-field'),
                    constraints: const BoxConstraints(minHeight: Grid.xl),
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        color: context.colors.surface,
                        border: Border.all(
                          color: context.colors.outlineVariant,
                        ),
                        borderRadius: BorderRadius.circular(Radii.lg),
                      ),
                      child: Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: Grid.twelve,
                          vertical: Grid.xxs,
                        ),
                        child: LayoutBuilder(
                          builder: (context, constraints) {
                            final hasSelectedUsers =
                                selectedUsers.value.isNotEmpty;
                            final searchFieldWidth = hasSelectedUsers
                                ? 96.0
                                : (constraints.maxWidth - 32).clamp(
                                    96.0,
                                    constraints.maxWidth,
                                  );

                            return Wrap(
                              key: const Key('new-dm-recipient-wrap'),
                              spacing: 6,
                              runSpacing: 6,
                              crossAxisAlignment: WrapCrossAlignment.center,
                              children: [
                                Padding(
                                  padding: const EdgeInsets.symmetric(
                                    vertical: Grid.half,
                                  ),
                                  child: Text(
                                    'To:',
                                    style: context.textTheme.bodyLarge
                                        ?.copyWith(fontWeight: FontWeight.w600),
                                  ),
                                ),
                                for (final user in visibleSelected)
                                  _SelectedDmRecipientChip(
                                    user: user,
                                    label: names.labelFor(user.pubkey),
                                    enabled: !isSubmitting.value,
                                    onDeleted: () {
                                      selectedUsers.value = [
                                        for (final candidate
                                            in selectedUsers.value)
                                          if (candidate.pubkey != user.pubkey)
                                            candidate,
                                      ];
                                      queryFocusNode.requestFocus();
                                    },
                                  ),
                                if (hiddenSelected > 0)
                                  ActionChip(
                                    key: const Key('new-dm-selected-more'),
                                    label: Text('+$hiddenSelected'),
                                    onPressed: () =>
                                        showAllSelected.value = true,
                                  ),
                                SizedBox(
                                  width: searchFieldWidth,
                                  child: TextField(
                                    key: const Key('new-dm-search'),
                                    controller: queryController,
                                    focusNode: queryFocusNode,
                                    autofocus: true,
                                    autocorrect: false,
                                    enableSuggestions: false,
                                    enabled: !isSubmitting.value,
                                    onChanged: (value) => query.value = value,
                                    onSubmitted: (_) {
                                      if (canSubmit) {
                                        unawaited(submit());
                                      }
                                    },
                                    decoration: InputDecoration(
                                      hintText: hasSelectedUsers
                                          ? null
                                          : 'Search for a person',
                                      border: InputBorder.none,
                                      enabledBorder: InputBorder.none,
                                      focusedBorder: InputBorder.none,
                                      isDense: true,
                                      contentPadding:
                                          const EdgeInsets.symmetric(
                                            vertical: Grid.half,
                                          ),
                                      suffixIcon: isSubmitting.value
                                          ? const Padding(
                                              padding: EdgeInsets.all(
                                                Grid.half,
                                              ),
                                              child: SizedBox.square(
                                                dimension: 16,
                                                child: KuzzLoadingIndicator(
                                                  size: 16,
                                                  semanticLabel:
                                                      'Creating conversation',
                                                ),
                                              ),
                                            )
                                          : null,
                                      suffixIconConstraints:
                                          const BoxConstraints(
                                            minHeight: 32,
                                            minWidth: 32,
                                          ),
                                    ),
                                    textInputAction: TextInputAction.done,
                                  ),
                                ),
                              ],
                            );
                          },
                        ),
                      ),
                    ),
                  ),
                ),
              ),
              if (selectedUsers.value.length > 1) ...[
                const SizedBox(height: Grid.xs),
                TextField(
                  key: const Key('new-chat-name'),
                  controller: chatNameController,
                  enabled: !isSubmitting.value,
                  maxLength: channelNameMaxLength,
                  textInputAction: TextInputAction.done,
                  onSubmitted: (_) {
                    if (canSubmit) unawaited(submit());
                  },
                  decoration: const InputDecoration(
                    labelText: 'Chat name',
                    hintText: 'Optional',
                    counterText: '',
                  ),
                ),
              ],
              const SizedBox(height: Grid.xs),
              Builder(
                key: const Key('new-dm-results'),
                builder: (context) {
                  if (selectedUsers.value.length >= groupChatCreateLimit) {
                    return const SizedBox(
                      height: 96,
                      child: Center(
                        child: Text(
                          'A chat can start with up to '
                          '$groupChatCreateLimit people.',
                        ),
                      ),
                    );
                  }
                  if (directoryAsync.isLoading &&
                          directoryAsync.asData == null ||
                      isSearchTransitionPending) {
                    return const SizedBox(
                      height: 280,
                      child: Center(
                        child: KuzzLoadingIndicator(
                          size: 44,
                          semanticLabel: 'Loading people',
                        ),
                      ),
                    );
                  }
                  if (directoryAsync.hasError) {
                    return SizedBox(
                      height: 280,
                      child: Center(
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            const Text('Couldn’t load people. Try again.'),
                            const SizedBox(height: Grid.half),
                            TextButton(
                              onPressed: () {
                                if (normalizedQuery.isEmpty) {
                                  ref.invalidate(relayDirectoryUsersProvider);
                                } else {
                                  ref.invalidate(
                                    relayDirectorySearchProvider(
                                      normalizedQuery,
                                    ),
                                  );
                                }
                              },
                              child: const Text('Retry'),
                            ),
                          ],
                        ),
                      ),
                    );
                  }
                  if (availableResults.isEmpty &&
                      !sharedPending &&
                      !sharedFailed) {
                    return SizedBox(
                      height: 280,
                      child: Center(
                        child: Text(
                          normalizedQuery.isEmpty
                              ? 'Add friends in the Friends tab to message them here.'
                              : 'No matching people.',
                          textAlign: TextAlign.center,
                        ),
                      ),
                    );
                  }
                  Widget header(String key, String title) => Padding(
                    key: Key('new-dm-section-$key'),
                    padding: const EdgeInsets.fromLTRB(
                      Grid.half,
                      Grid.xs,
                      Grid.half,
                      Grid.half,
                    ),
                    child: Text(
                      title,
                      style: context.textTheme.labelLarge?.copyWith(
                        color: context.colors.onSurfaceVariant,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  );
                  Widget note(String key, String text) => Padding(
                    key: Key('new-dm-$key'),
                    padding: const EdgeInsets.symmetric(
                      horizontal: Grid.half,
                      vertical: Grid.xxs,
                    ),
                    child: Text(
                      text,
                      style: context.textTheme.bodyMedium?.copyWith(
                        color: context.colors.onSurfaceVariant,
                      ),
                    ),
                  );
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      if (friendResults.isNotEmpty) ...[
                        if (!previewDirectoryActive)
                          header('friends', 'Friends'),
                        for (final user in friendResults) tileFor(user),
                      ],
                      if (sharedResults.isNotEmpty ||
                          sharedPending ||
                          sharedFailed) ...[
                        header('shared', 'In your groups'),
                        if (sharedPending)
                          note('shared-loading', 'Searching…')
                        else if (sharedFailed)
                          note('shared-error', 'Couldn’t search your groups')
                        else
                          for (final user in sharedResults) tileFor(user),
                      ],
                      if (agentResults.isNotEmpty) ...[
                        header('agents', 'Agents'),
                        for (final user in agentResults) tileFor(user),
                      ],
                    ],
                  );
                },
              ),
              if (submitError.value case final error?) ...[
                const SizedBox(height: Grid.xxs),
                Text(
                  error,
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.error,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _SelectedDmRecipientChip extends StatelessWidget {
  final DirectoryUser user;
  final String label;
  final bool enabled;
  final VoidCallback onDeleted;

  const _SelectedDmRecipientChip({
    required this.user,
    required this.label,
    required this.enabled,
    required this.onDeleted,
  });

  @override
  Widget build(BuildContext context) {
    final foreground = context.colors.onSurfaceVariant;

    return ConstrainedBox(
      key: Key('new-dm-selected-${user.pubkey}'),
      constraints: const BoxConstraints(maxWidth: 224),
      child: SizedBox(
        height: 40,
        child: Material(
          color: context.colors.surfaceContainerHighest,
          shape: const StadiumBorder(),
          clipBehavior: Clip.antiAlias,
          child: Semantics(
            button: true,
            enabled: enabled,
            excludeSemantics: true,
            label: 'Remove $label',
            child: InkWell(
              customBorder: const StadiumBorder(),
              onTap: enabled ? onDeleted : null,
              child: Padding(
                padding: const EdgeInsets.only(
                  left: Grid.half,
                  right: Grid.twelve,
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    AvatarImage(
                      imageUrl: user.avatarUrl,
                      radius: 16,
                      backgroundColor: context.colors.primaryContainer,
                      fallback: Text(
                        user.initial,
                        style: context.textTheme.labelMedium?.copyWith(
                          color: context.colors.onPrimaryContainer,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      isAgent: user.isAgent,
                    ),
                    const SizedBox(width: Grid.xxs),
                    Flexible(
                      child: Text(
                        label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: context.textTheme.bodyLarge?.copyWith(
                          color: foreground,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
