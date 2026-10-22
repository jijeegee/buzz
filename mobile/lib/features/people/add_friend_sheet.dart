import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/channel.dart';
import '../channels/channel_detail_page.dart';
import '../channels/channel_management_provider.dart';
import 'blocked_users_provider.dart';
import 'friends_provider.dart';
import 'people_agents.dart';
import 'people_errors.dart';
import 'person_row.dart';

/// "Add friend": type someone's exact Kuzz ID or email, find that one
/// person, add them. No partial search, so nobody's email can be guessed.
/// After adding, "Message" closes the sheet and opens our conversation.
Future<void> showAddFriendSheet(BuildContext context) async {
  final channel = await showKuzzModalBottomSheet<Channel>(
    context: context,
    isScrollControlled: true,
    title: context.l10n.friendsAddFriend,
    builder: (_) => const AddFriendSheet(),
  );
  if (channel == null || !context.mounted) return;
  await Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => ChannelDetailPage(channel: channel),
    ),
  );
}

class AddFriendSheet extends HookConsumerWidget {
  const AddFriendSheet({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = useTextEditingController();
    final searching = useState(false);
    final adding = useState(false);
    // Who was just added here: shown as "Added" with a Message button.
    final added = useState<String?>(null);
    final opening = useState(false);
    final found = useState<PersonSummary?>(null);
    final message = useState<String?>(null);
    final generation = useRef(0);
    final origin = ref.watch(accountOriginProvider);
    final me = ref.watch(currentPubkeyProvider)?.toLowerCase();
    final friends = ref.watch(friendIdsProvider).value ?? const <String>{};
    final blocked = ref.watch(blockedPubkeysProvider);
    final agents = ref.watch(peopleAgentPubkeysProvider);
    final l10n = context.l10n;

    Future<void> find() async {
      final query = controller.text.trim();
      if (query.isEmpty || origin == null) return;
      final mine = ++generation.value;
      searching.value = true;
      found.value = null;
      message.value = null;
      String? failure;
      PersonSummary? person;
      final api = ref.read(accountApiProvider(origin));
      try {
        person = await api.lookupPerson(query);
      } catch (error) {
        failure = error is AccountApiException && error.statusCode == 429
            ? l10n.friendsTooManySearches
            : friendlyError(error, fallback: l10n.friendsAddSearchError);
      }
      // A newer search owns the result.
      if (!context.mounted || mine != generation.value) return;
      searching.value = false;
      if (failure != null) {
        message.value = failure;
      } else if (person == null ||
          person.pubkey == me ||
          blocked.contains(person.pubkey) ||
          agents.contains(person.pubkey)) {
        message.value = l10n.friendsAddNotFound;
      } else {
        found.value = person;
      }
    }

    Future<void> add(PersonSummary person) async {
      if (adding.value) return;
      adding.value = true;
      try {
        await ref.read(friendsControllerProvider).add(person.pubkey);
      } catch (error) {
        if (context.mounted) {
          adding.value = false;
          message.value = friendlyError(error, fallback: l10n.friendsAddError);
        }
        return;
      }
      if (context.mounted) {
        adding.value = false;
        added.value = person.pubkey;
      }
    }

    Future<void> openMessage(PersonSummary person) async {
      if (opening.value) return;
      opening.value = true;
      try {
        final channel = await ref
            .read(channelActionsProvider)
            .openDm(pubkeys: [person.pubkey]);
        if (context.mounted) Navigator.of(context).pop(channel);
      } catch (error) {
        if (!context.mounted) return;
        opening.value = false;
        message.value = friendlyError(
          error,
          fallback: l10n.friendsOpenChatError,
        );
      }
    }

    final person = found.value;
    final isFriend = person != null && friends.contains(person.pubkey);
    final justAdded = person != null && added.value == person.pubkey;

    return SafeArea(
      top: false,
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          Grid.xs + MediaQuery.viewInsetsOf(context).bottom,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            TextField(
              key: const Key('add-friend-query'),
              controller: controller,
              autofocus: true,
              autocorrect: false,
              enableSuggestions: false,
              keyboardType: TextInputType.emailAddress,
              textInputAction: TextInputAction.search,
              onChanged: (_) {
                if (found.value != null || message.value != null) {
                  found.value = null;
                  message.value = null;
                }
              },
              onSubmitted: (_) => unawaited(find()),
              decoration: InputDecoration(
                labelText: l10n.friendsAddIdLabel,
                helperText: l10n.friendsAddIdHelper,
              ),
            ),
            const SizedBox(height: Grid.xs),
            ValueListenableBuilder<TextEditingValue>(
              valueListenable: controller,
              builder: (context, value, _) => FilledButton(
                key: const Key('add-friend-find'),
                onPressed:
                    searching.value ||
                        value.text.trim().isEmpty ||
                        origin == null
                    ? null
                    : () => unawaited(find()),
                child: searching.value
                    ? KuzzLoadingIndicator(
                        size: 20,
                        semanticLabel: l10n.friendsSearching,
                      )
                    : Text(l10n.friendsAddFind),
              ),
            ),
            if (message.value case final text?) ...[
              const SizedBox(height: Grid.xs),
              Semantics(
                liveRegion: true,
                child: Text(
                  text,
                  key: const Key('add-friend-message'),
                  textAlign: TextAlign.center,
                  style: context.textTheme.bodyMedium?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              ),
            ],
            if (person != null) ...[
              const SizedBox(height: Grid.xs),
              ListTile(
                key: const Key('add-friend-result'),
                contentPadding: EdgeInsets.zero,
                leading: PersonAvatar(
                  name: personName(null, person),
                  avatarUrl: person.avatarUrl,
                ),
                title: Text(personName(null, person)),
                subtitle: person.username == null
                    ? null
                    : Text('@${person.username}'),
                trailing: justAdded
                    ? Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Semantics(
                            liveRegion: true,
                            child: Text(
                              l10n.friendsAdded,
                              key: const Key('add-friend-added'),
                            ),
                          ),
                          const SizedBox(width: Grid.xxs),
                          FilledButton.tonal(
                            key: const Key('add-friend-open-dm'),
                            onPressed: opening.value
                                ? null
                                : () => unawaited(openMessage(person)),
                            child: Text(l10n.friendsMessage),
                          ),
                        ],
                      )
                    : isFriend
                    ? Text(l10n.friendsIsFriend)
                    : FilledButton.tonal(
                        key: const Key('add-friend-add'),
                        onPressed: adding.value
                            ? null
                            : () => unawaited(add(person)),
                        child: Text(l10n.friendsAdd),
                      ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
