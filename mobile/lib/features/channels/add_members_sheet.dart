import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/identity_names/identity_names_provider.dart';
import '../../shared/l10n/l10n.dart';
import 'channel_management_provider.dart';

/// Searchable multi-select used to add people or agents to a channel.
class AddChannelMembersSheet extends HookConsumerWidget {
  const AddChannelMembersSheet({
    super.key,
    required this.channelId,
    required this.existingPubkeys,
    this.chat = false,
    this.startChat,
  });

  final String channelId;
  final Set<String> existingPubkeys;

  /// Adding to a chat of people: agents join as `bot`, wording says chat.
  final bool chat;

  /// When set, the picked people start a new chat through this instead of
  /// joining [channelId] (a 1:1's "Add people").
  final Future<void> Function(List<String> pubkeys)? startChat;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final queryController = useTextEditingController();
    final query = useState('');
    final debouncedQuery = useState('');
    final selectedUsers = useState<List<DirectoryUser>>([]);
    final isSubmitting = useState(false);
    final submitError = useState<String?>(null);

    useEffect(() {
      final timer = Timer(const Duration(milliseconds: 250), () {
        debouncedQuery.value = query.value.trim().toLowerCase();
      });
      return timer.cancel;
    }, [query.value]);

    final normalizedQuery = debouncedQuery.value;
    final directoryAsync = normalizedQuery.isEmpty
        ? ref.watch(relayDirectoryUsersProvider)
        : ref.watch(relayDirectorySearchProvider(normalizedQuery));
    final normalizedExisting = existingPubkeys
        .map((pubkey) => pubkey.toLowerCase())
        .toSet();
    final selectedPubkeys = selectedUsers.value
        .map((user) => user.pubkey.toLowerCase())
        .toSet();
    final successfulPubkeys = useState<Set<String>>(<String>{});
    final excludedPubkeys = {...normalizedExisting, ...successfulPubkeys.value};
    final availableUsers =
        directoryAsync.asData?.value
            .where(
              (user) => !excludedPubkeys.contains(user.pubkey.toLowerCase()),
            )
            .toList() ??
        const <DirectoryUser>[];
    // Compare every shown choice with the channel's members, so a candidate
    // who shares a member's name is told apart before being added. Members
    // are only compared: their names come from the roster the details page
    // already loaded, so only the shown choices' profiles are fetched.
    final choices = [...availableUsers, ...selectedUsers.value];
    final roster =
        ref.watch(channelMembersProvider(channelId)).asData?.value ??
        const <ChannelMember>[];
    final names = watchIdentityNames(
      ref,
      [...normalizedExisting, for (final user in choices) user.pubkey],
      agentPubkeys: {
        for (final member in roster)
          if (member.isBot) member.pubkey,
        for (final user in choices)
          if (user.isAgent) user.pubkey,
      },
      fallbackNames: {
        for (final member in roster) member.pubkey: ?member.displayName,
        for (final user in choices) user.pubkey: user.label,
      },
      shown: [for (final user in choices) user.pubkey],
    );
    String labelFor(DirectoryUser user) => names.labelFor(user.pubkey);

    void toggleUser(DirectoryUser user) {
      if (isSubmitting.value) return;
      final normalized = user.pubkey.toLowerCase();
      selectedUsers.value = selectedPubkeys.contains(normalized)
          ? [
              for (final selected in selectedUsers.value)
                if (selected.pubkey.toLowerCase() != normalized) selected,
            ]
          : [...selectedUsers.value, user];
      submitError.value = null;
    }

    Future<void> addSelectedMembers() async {
      if (selectedUsers.value.isEmpty || isSubmitting.value) return;
      final submittedUsers = List<DirectoryUser>.of(selectedUsers.value);
      isSubmitting.value = true;
      submitError.value = null;
      final messenger = ScaffoldMessenger.maybeOf(context);
      void announceInvited(List<String> invited) {
        if (invited.isEmpty) return;
        final names = [
          for (final user in submittedUsers)
            if (invited.contains(user.pubkey.toLowerCase())) labelFor(user),
        ];
        messenger?.showSnackBar(
          SnackBar(content: Text(invitationSentMessage(names))),
        );
      }

      try {
        if (startChat case final start?) {
          await start(submittedUsers.map((user) => user.pubkey).toList());
          if (context.mounted) Navigator.of(context).pop(true);
          return;
        }
        final actions = ref.read(channelActionsProvider);
        final agents = [
          for (final user in submittedUsers)
            if (chat && user.isAgent) user.pubkey,
        ];
        final people = [
          for (final user in submittedUsers)
            if (!(chat && user.isAgent)) user.pubkey,
        ];
        final invited = <String>[];
        final failures = <String, String>{};
        // A chat adds agents as bots; each batch reports its own refusals.
        for (final (pubkeys, role) in [(people, 'member'), (agents, 'bot')]) {
          if (pubkeys.isEmpty) continue;
          try {
            final result = await actions.addMembers(
              channelId: channelId,
              pubkeys: pubkeys,
              role: role,
            );
            invited.addAll(result.invited);
          } on AddMembersException catch (error) {
            invited.addAll(error.invited);
            failures.addAll(error.failures);
          }
        }
        if (failures.isNotEmpty) {
          throw AddMembersException(failures, invited: invited);
        }
        announceInvited(invited);
        if (context.mounted) Navigator.of(context).pop(true);
      } on AddMembersException catch (error) {
        announceInvited(error.invited);
        final failedPubkeys = error.failures.keys
            .map((pubkey) => pubkey.toLowerCase())
            .toSet();
        successfulPubkeys.value = {
          ...successfulPubkeys.value,
          for (final user in submittedUsers)
            if (!failedPubkeys.contains(user.pubkey.toLowerCase()))
              user.pubkey.toLowerCase(),
        };
        selectedUsers.value = [
          for (final user in submittedUsers)
            if (failedPubkeys.contains(user.pubkey.toLowerCase())) user,
        ];
        submitError.value = error.message;
      } catch (error) {
        submitError.value = error.toString();
      } finally {
        isSubmitting.value = false;
      }
    }

    return Padding(
      padding: EdgeInsets.fromLTRB(
        Grid.gutter,
        0,
        Grid.gutter,
        MediaQuery.viewInsetsOf(context).bottom,
      ),
      child: SafeArea(
        top: false,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            maxHeight: MediaQuery.sizeOf(context).height * 0.72,
          ),
          child: Column(
            children: [
              TextField(
                key: const ValueKey('add-channel-members-search'),
                controller: queryController,
                autofocus: true,
                autocorrect: false,
                enableSuggestions: false,
                enabled: !isSubmitting.value,
                onChanged: (value) => query.value = value,
                decoration: InputDecoration(
                  hintText: context.l10n.channelAddMembersSearchHint,
                  prefixIcon: const Icon(LucideIcons.search),
                ),
              ),
              if (selectedUsers.value.isNotEmpty) ...[
                const SizedBox(height: Grid.xxs),
                Align(
                  alignment: AlignmentDirectional.centerStart,
                  child: Wrap(
                    spacing: Grid.half,
                    runSpacing: Grid.half,
                    children: [
                      for (final user in selectedUsers.value)
                        InputChip(
                          key: ValueKey(
                            'add-channel-member-selected-${user.pubkey}',
                          ),
                          label: Text(labelFor(user)),
                          onDeleted: isSubmitting.value
                              ? null
                              : () => toggleUser(user),
                        ),
                    ],
                  ),
                ),
              ],
              const SizedBox(height: Grid.xxs),
              Expanded(
                child: directoryAsync.when(
                  data: (_) => availableUsers.isEmpty
                      ? Center(
                          child: Text(
                            normalizedQuery.isEmpty
                                ? (chat
                                      ? context.l10n.channelAddMembersAllInChat
                                      : context
                                            .l10n
                                            .channelAddMembersAllInGroup)
                                : context.l10n.channelAddMembersNoMatches,
                            textAlign: TextAlign.center,
                          ),
                        )
                      : ListView.builder(
                          key: const ValueKey('add-channel-members-results'),
                          itemCount: availableUsers.length,
                          itemBuilder: (context, index) {
                            final user = availableUsers[index];
                            final selected = selectedPubkeys.contains(
                              user.pubkey.toLowerCase(),
                            );
                            return Semantics(
                              key: ValueKey(
                                'add-channel-member-${user.pubkey}',
                              ),
                              button: true,
                              selected: selected,
                              label: selected
                                  ? context.l10n.channelAddMembersSelected(
                                      labelFor(user),
                                    )
                                  : labelFor(user),
                              child: ListTile(
                                contentPadding: EdgeInsets.zero,
                                leading: AvatarImage(
                                  imageUrl: user.avatarUrl,
                                  radius: 20,
                                  backgroundColor:
                                      context.colors.primaryContainer,
                                  fallback: Text(user.initial),
                                  isAgent: user.isAgent,
                                ),
                                title: Text(
                                  labelFor(user),
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                ),
                                subtitle: Text(
                                  user.secondaryLabel,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                ),
                                trailing: Icon(
                                  selected
                                      ? LucideIcons.circleCheck
                                      : LucideIcons.plus,
                                  color: selected
                                      ? context.colors.primary
                                      : context.colors.onSurfaceVariant,
                                ),
                                onTap: () => toggleUser(user),
                              ),
                            );
                          },
                        ),
                  loading: () => Center(
                    child: KuzzLoadingIndicator(
                      size: 44,
                      semanticLabel: context.l10n.channelAddMembersLoading,
                    ),
                  ),
                  error: (error, _) => Center(
                    child: Text(
                      context.l10n.channelAddMembersLoadError,
                      style: context.textTheme.bodyMedium?.copyWith(
                        color: context.colors.error,
                      ),
                      textAlign: TextAlign.center,
                    ),
                  ),
                ),
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
              const SizedBox(height: Grid.xxs),
              SizedBox(
                width: double.infinity,
                child: FilledButton(
                  key: const ValueKey('add-channel-members-submit'),
                  onPressed: selectedUsers.value.isEmpty || isSubmitting.value
                      ? null
                      : addSelectedMembers,
                  child: Text(
                    startChat != null
                        ? (isSubmitting.value
                              ? context.l10n.channelAddMembersStarting
                              : context.l10n.channelAddMembersStartChat)
                        : isSubmitting.value
                        ? context.l10n.channelAddMembersAdding
                        : chat
                        ? context.l10n.channelAddPeople
                        : context.l10n.channelAddMembersSubmit(
                            selectedUsers.value.length,
                          ),
                  ),
                ),
              ),
              const SizedBox(height: Grid.xxs),
            ],
          ),
        ),
      ),
    );
  }
}
