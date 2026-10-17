import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/channel_management_provider.dart';
import '../profile/user_profile_sheet.dart';
import '../pulse/pulse_actions.dart';
import '../pulse/pulse_models.dart';
import '../pulse/pulse_provider.dart';
import 'person_row.dart';

/// The People tab, as desktop's: Friends (people I added, my follow list)
/// A–Z, narrowed by name. Typing a name also finds others so they can be
/// added. People only; agents live in the Agents tab. Tapping someone opens
/// their profile, which has Message.
class PeoplePage extends HookConsumerWidget {
  const PeoplePage({this.tabReselection, super.key});

  /// Notifies this page when its already-selected tab is tapped again.
  final ValueListenable<int>? tabReselection;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final me = ref.watch(myPubkeyProvider)?.toLowerCase() ?? '';
    final contactsAsync = ref.watch(contactListProvider(me));
    final agents = ref.watch(knownAgentPubkeysProvider);
    final filterController = useTextEditingController();
    final filter = useState('');
    final debounced = useState('');
    useEffect(() {
      final timer = Timer(const Duration(milliseconds: 250), () {
        debounced.value = filter.value.trim().toLowerCase();
      });
      return timer.cancel;
    }, [filter.value]);
    final busy = useState(<String>{});

    final friendIds = [
      for (final entry in contactsAsync.value ?? const <ContactEntry>[])
        if (entry.pubkey.toLowerCase() != me &&
            !agents.contains(entry.pubkey.toLowerCase()))
          entry.pubkey.toLowerCase(),
    ];
    useEffect(() {
      if (friendIds.isEmpty) return null;
      Future.microtask(
        () => ref.read(userCacheProvider.notifier).preload(friendIds),
      );
      return null;
    }, [friendIds.join(',')]);
    final profiles = ref.watch(userCacheProvider);
    String labelOf(String pubkey) {
      final name = profiles[pubkey]?.displayName?.trim();
      if (name != null && name.isNotEmpty) return name;
      return pubkey.length > 8 ? pubkey.substring(0, 8) : pubkey;
    }

    final needle = debounced.value;
    final friends =
        [
          for (final pubkey in friendIds)
            if (profiles[pubkey]?.isAgent != true &&
                (needle.isEmpty ||
                    labelOf(pubkey).toLowerCase().contains(needle)))
              pubkey,
        ]..sort(
          (a, b) =>
              labelOf(a).toLowerCase().compareTo(labelOf(b).toLowerCase()),
        );
    final friendSet = friendIds.toSet();
    final othersAsync = needle.isEmpty
        ? null
        : ref.watch(relayDirectorySearchProvider(needle));
    final others = [
      for (final user in othersAsync?.value ?? const <DirectoryUser>[])
        if (!user.isAgent &&
            !agents.contains(user.pubkey.toLowerCase()) &&
            user.pubkey.toLowerCase() != me &&
            !friendSet.contains(user.pubkey.toLowerCase()))
          user,
    ];

    Future<void> setFriend(String pubkey, bool add) async {
      busy.value = {...busy.value, pubkey};
      try {
        add ? await followUser(ref, pubkey) : await unfollowUser(ref, pubkey);
      } catch (error) {
        if (context.mounted) {
          ScaffoldMessenger.maybeOf(context)?.showSnackBar(
            SnackBar(
              content: Text(
                add ? 'Could not add friend' : 'Could not remove friend',
              ),
            ),
          );
        }
      } finally {
        busy.value = {...busy.value}..remove(pubkey);
      }
    }

    void showFriendActions(String pubkey) {
      showKuzzModalBottomSheet<void>(
        context: context,
        builder: (sheetContext) => SafeArea(
          child: ListTile(
            leading: const Icon(Icons.person_remove_outlined),
            title: const Text('Remove friend'),
            onTap: () {
              Navigator.of(sheetContext).pop();
              unawaited(setFriend(pubkey, false));
            },
          ),
        ),
      );
    }

    final headerTitleStyle = context.textTheme.titleMedium?.copyWith(
      fontSize: 22,
      fontWeight: FontWeight.w600,
      color: navigationPrimaryForeground(context),
    );
    final topSectionHeight = frostedAppBarHeight(
      context,
      titleStyle: headerTitleStyle,
      bottomHeight: Grid.xxs,
    );

    final loading = contactsAsync.isLoading && !contactsAsync.hasValue;
    return FrostedScaffold(
      backgroundColor: context.colors.surface,
      appBar: FrostedAppBar(
        automaticallyImplyLeading: false,
        horizontalInset: Grid.gutter,
        showBottomDivider: true,
        bottomDividerOpacity: 0.07,
        title: Text('People', style: headerTitleStyle),
        titleStyle: headerTitleStyle,
        bottomHeight: Grid.xxs,
        bottom: const SizedBox.expand(),
      ),
      body: ListView(
        key: const ValueKey('people-list'),
        padding: EdgeInsets.only(
          top: topSectionHeight,
          bottom: MediaQuery.paddingOf(context).bottom + Grid.xs,
        ),
        children: [
          PeopleFilterField(
            controller: filterController,
            hint: 'Find people',
            onChanged: (value) => filter.value = value,
          ),
          PeopleSectionTitle('Friends · ${friends.length}'),
          if (loading)
            const Padding(
              padding: EdgeInsets.all(Grid.sm),
              child: Center(child: KuzzLoadingIndicator()),
            )
          else if (friends.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Grid.gutter,
                vertical: Grid.xs,
              ),
              child: Text(
                needle.isEmpty
                    ? 'No friends yet. Type a name to find people and add them.'
                    : 'No friends match that name.',
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            )
          else
            for (final pubkey in friends)
              PersonRow(
                key: ValueKey('people-friend-$pubkey'),
                pubkey: pubkey,
                name: labelOf(pubkey),
                subtitle: 'Friend',
                onTap: () => showUserProfileSheet(context, pubkey),
                onLongPress: () => showFriendActions(pubkey),
              ),
          if (needle.isNotEmpty) ...[
            const PeopleSectionTitle('Others'),
            if (othersAsync?.isLoading == true && others.isEmpty)
              const Padding(
                padding: EdgeInsets.all(Grid.sm),
                child: Center(child: KuzzLoadingIndicator()),
              )
            else if (others.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: Grid.gutter,
                  vertical: Grid.xs,
                ),
                child: Text(
                  'No one else matches that name.',
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              )
            else
              for (final user in others)
                PersonRow(
                  key: ValueKey('people-other-${user.pubkey}'),
                  pubkey: user.pubkey.toLowerCase(),
                  name: user.label,
                  subtitle: 'Member',
                  onTap: () =>
                      showUserProfileSheet(context, user.pubkey.toLowerCase()),
                  trailing: OutlinedButton(
                    key: ValueKey('people-add-${user.pubkey}'),
                    onPressed: busy.value.contains(user.pubkey.toLowerCase())
                        ? null
                        : () => unawaited(
                            setFriend(user.pubkey.toLowerCase(), true),
                          ),
                    style: OutlinedButton.styleFrom(
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('Add'),
                  ),
                ),
          ],
        ],
      ),
    );
  }
}
