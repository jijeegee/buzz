import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/bee_refresh_indicator.dart';
import '../../shared/widgets/conversation_row_parts.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/kuzz_search_field.dart';
import '../channels/channel_management_provider.dart';
import '../pulse/pulse_provider.dart';
import 'add_friend_sheet.dart';
import 'blocked_users_provider.dart';
import 'friends_provider.dart';
import 'people_agents.dart';
import 'people_favorites.dart';
import 'person_page.dart';
import 'person_row.dart';

/// How long typing must pause before "In your groups" asks the server.
const peopleSearchDebounce = Duration(milliseconds: 300);

/// People who share a channel with me and match [query] (server side). Empty
/// without an account or for an empty query. Never retried: a failure shows
/// as such and the next keystroke asks again.
final sharedPeopleProvider = FutureProvider.autoDispose
    .family<List<PersonSummary>, String>((ref, query) async {
      final q = query.trim();
      final origin = ref.watch(accountOriginProvider);
      if (q.isEmpty || origin == null) return const [];
      return ref.watch(accountApiProvider(origin)).sharedPeople(q);
    }, retry: (_, _) => null);

bool _matches(String query, UserProfile? profile, String name) {
  final q = query.toLowerCase();
  if (name.toLowerCase().contains(q)) return true;
  final handle = profile?.nip05Handle?.toLowerCase();
  return handle != null && handle.contains(q);
}

/// The People tab: favorites and friends, a search over both plus the
/// people I share channels with, and "Add friend" by exact ID or email.
class PeoplePage extends HookConsumerWidget {
  const PeoplePage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = useTextEditingController();
    final focusNode = useFocusNode();
    final query = useState('');
    final debouncedQuery = useState('');
    final isEditing = useState(false);

    useEffect(() {
      final q = query.value.trim();
      if (q.isEmpty) {
        debouncedQuery.value = '';
        return null;
      }
      final timer = Timer(peopleSearchDebounce, () {
        debouncedQuery.value = q;
      });
      return timer.cancel;
    }, [query.value]);

    useEffect(() {
      void onFocus() => isEditing.value = focusNode.hasFocus;
      focusNode.addListener(onFocus);
      return () => focusNode.removeListener(onFocus);
    }, [focusNode]);

    final me = ref.watch(currentPubkeyProvider)?.toLowerCase();
    final blocked = ref.watch(blockedPubkeysProvider);
    final agents = ref.watch(peopleAgentPubkeysProvider);
    final favorites = ref.watch(peopleFavoritesProvider);
    final friendsAsync = ref.watch(friendIdsProvider);
    final cache = ref.watch(userCacheProvider);

    bool visible(String pk) {
      if (pk == me || blocked.contains(pk) || agents.contains(pk)) {
        return false;
      }
      return !(cache[pk]?.isAgent ?? false);
    }

    final friendIds = (friendsAsync.value ?? const <String>{})
        .where(visible)
        .toList();
    final favoriteIds = favorites.where(visible).toList();

    useEffect(
      () {
        final ids = {...friendIds, ...favoriteIds};
        if (ids.isNotEmpty) {
          ref.read(userCacheProvider.notifier).preload(ids.toList());
        }
        return null;
      },
      [
        Object.hashAllUnordered(friendIds),
        Object.hashAllUnordered(favoriteIds),
      ],
    );

    String nameOf(String pk) => personName(cache[pk]);
    int byName(String a, String b) =>
        nameOf(a).toLowerCase().compareTo(nameOf(b).toLowerCase());
    friendIds.sort(byName);
    favoriteIds.sort(byName);

    void openPerson(String pk, [PersonSummary? summary]) {
      unawaited(
        Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => PersonPage(pubkey: pk, summary: summary),
          ),
        ),
      );
    }

    final headerTitleStyle = context.textTheme.titleMedium?.copyWith(
      fontSize: 22,
      fontWeight: FontWeight.w600,
      color: navigationPrimaryForeground(context),
    );
    final topHeight = frostedAppBarHeight(
      context,
      titleStyle: headerTitleStyle,
      bottomHeight: Grid.xxs,
    );

    // Pull to refresh: friends and blocks changed on another device.
    Future<void> refresh() async {
      final key = me;
      await Future.wait([
        if (key != null && key.isNotEmpty)
          ref
              .refresh(contactListProvider(key).future)
              .then<void>((_) {}, onError: (_) {}),
        ref.read(blockedUsersProvider.notifier).refresh(),
      ]);
    }

    final searching = query.value.trim().isNotEmpty;
    final children = <Widget>[
      Padding(
        padding: const EdgeInsets.fromLTRB(
          Grid.gutter,
          Grid.xxs,
          Grid.gutter,
          Grid.xxs,
        ),
        child: SizedBox(
          height: kuzzSearchIdleFieldHeight,
          child: KuzzSearchField(
            fieldKey: const Key('people-search'),
            controller: controller,
            focusNode: focusNode,
            hintText: 'Search friends',
            iconColor: navigationPrimaryForeground(context),
            inputColor: navigationPrimaryForeground(context),
            placeholderColor: navigationSecondaryForeground(context),
            surfaceColor: navigationSearchSurface(context),
            isEditing: isEditing.value,
            reduceMotion: MediaQuery.disableAnimationsOf(context),
            motionDuration: const Duration(milliseconds: 160),
            autocorrect: false,
            enableSuggestions: false,
            onTap: focusNode.requestFocus,
            onChanged: (value) => query.value = value,
            onSubmitted: (_) {},
          ),
        ),
      ),
    ];

    if (searching) {
      final q = query.value.trim();
      final local =
          {
              ...favoriteIds,
              ...friendIds,
            }.where((pk) => _matches(q, cache[pk], nameOf(pk))).toList()
            ..sort(byName);
      children
        ..add(_SectionHeader(title: 'Friends'))
        ..addAll(
          local.isEmpty
              ? [const _EmptyLine(text: 'No friends match')]
              : [
                  for (final pk in local)
                    PersonRow(
                      key: ValueKey('people-friend-$pk'),
                      pubkey: pk,
                      isFavorite: favorites.contains(pk),
                      onTap: () => openPerson(pk),
                    ),
                ],
        )
        ..add(
          _SharedResults(
            query: debouncedQuery.value,
            pending: debouncedQuery.value != q,
            exclude: {...local},
            visible: visible,
            favorites: favorites,
            onOpen: openPerson,
          ),
        );
    } else {
      if (favoriteIds.isNotEmpty) {
        children
          ..add(const _SectionHeader(title: 'Favorites'))
          ..addAll([
            for (final pk in favoriteIds)
              PersonRow(
                key: ValueKey('people-favorite-$pk'),
                pubkey: pk,
                isFavorite: true,
                onTap: () => openPerson(pk),
              ),
          ]);
      }
      children.add(
        _SectionHeader(
          title: friendsAsync.hasValue
              ? 'Friends · ${friendIds.length}'
              : 'Friends',
        ),
      );
      if (friendsAsync.isLoading && !friendsAsync.hasValue) {
        children.add(
          const Padding(
            padding: EdgeInsets.all(Grid.sm),
            child: Center(
              child: KuzzLoadingIndicator(
                size: 24,
                semanticLabel: 'Loading friends',
              ),
            ),
          ),
        );
      } else if (friendsAsync.hasError && !friendsAsync.hasValue) {
        children.add(
          _EmptyLine(
            text: 'Couldn’t load your friends',
            action: TextButton(
              onPressed: () => ref.invalidate(contactListProvider),
              child: const Text('Retry'),
            ),
          ),
        );
      } else if (friendIds.isEmpty) {
        children.add(
          const _EmptyLine(
            key: Key('people-friends-empty'),
            text: 'No friends yet. Tap Add friend to find someone.',
          ),
        );
      } else {
        children.addAll([
          for (final pk in friendIds)
            PersonRow(
              key: ValueKey('people-friend-$pk'),
              pubkey: pk,
              isFavorite: favorites.contains(pk),
              onTap: () => openPerson(pk),
            ),
        ]);
      }
    }

    return FrostedScaffold(
      backgroundColor: conversationListBackground(context),
      appBar: FrostedAppBar(
        automaticallyImplyLeading: false,
        horizontalInset: Grid.gutter,
        showBottomDivider: true,
        bottomDividerOpacity: 0.07,
        title: Text('Friends', style: headerTitleStyle),
        titleStyle: headerTitleStyle,
        actions: [
          IconButton(
            key: const Key('people-add-friend'),
            tooltip: 'Add friend',
            color: navigationPrimaryForeground(context),
            icon: const Icon(LucideIcons.userPlus),
            onPressed: () => unawaited(showAddFriendSheet(context)),
          ),
        ],
        bottomHeight: Grid.xxs,
        bottom: const SizedBox.expand(),
      ),
      body: SafeArea(
        top: false,
        bottom: false,
        child: BeeRefreshIndicator(
          edgeOffset: topHeight,
          onRefresh: refresh,
          child: ListView(
            key: const Key('people-list'),
            physics: const AlwaysScrollableScrollPhysics(),
            keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
            padding: EdgeInsets.only(top: topHeight, bottom: Grid.lg),
            children: children,
          ),
        ),
      ),
    );
  }
}

/// "In your groups": server matches not already shown as friends.
class _SharedResults extends ConsumerWidget {
  const _SharedResults({
    required this.query,
    required this.pending,
    required this.exclude,
    required this.visible,
    required this.favorites,
    required this.onOpen,
  });

  final String query;
  final bool pending;
  final Set<String> exclude;
  final bool Function(String) visible;
  final Set<String> favorites;
  final void Function(String, [PersonSummary?]) onOpen;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (ref.watch(accountOriginProvider) == null) {
      return const SizedBox.shrink();
    }
    final header = const _SectionHeader(title: 'In your groups');
    if (pending || query.isEmpty) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [header, const _Searching()],
      );
    }
    final result = ref.watch(sharedPeopleProvider(query));
    final List<Widget> body;
    if (result.isLoading && !result.hasValue) {
      body = [const _Searching()];
    } else if (result.hasError && !result.hasValue) {
      final error = result.error;
      body = [
        _EmptyLine(
          key: const Key('people-shared-error'),
          text: error is AccountApiException && error.statusCode == 429
              ? 'Too many searches. Wait a minute and try again.'
              : 'Couldn’t search your groups',
          action: TextButton(
            onPressed: () => ref.invalidate(sharedPeopleProvider(query)),
            child: const Text('Retry'),
          ),
        ),
      ];
    } else {
      final people = [
        for (final person in result.value ?? const <PersonSummary>[])
          if (!exclude.contains(person.pubkey) && visible(person.pubkey))
            person,
      ];
      body = people.isEmpty
          ? [const _EmptyLine(text: 'No one else matches')]
          : [
              for (final person in people)
                PersonRow(
                  key: ValueKey('people-shared-${person.pubkey}'),
                  pubkey: person.pubkey,
                  summary: person,
                  isFavorite: favorites.contains(person.pubkey),
                  subtitle: person.username == null
                      ? null
                      : '@${person.username}',
                  onTap: () => onOpen(person.pubkey, person),
                ),
            ];
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [header, ...body],
    );
  }
}

class _SectionHeader extends StatelessWidget {
  const _SectionHeader({required this.title});

  final String title;

  @override
  Widget build(BuildContext context) => Semantics(
    header: true,
    child: Padding(
      padding: const EdgeInsets.fromLTRB(
        Grid.gutter,
        Grid.xs,
        Grid.gutter,
        Grid.half,
      ),
      child: Text(
        title,
        style: context.textTheme.labelLarge?.copyWith(
          color: navigationSectionForeground(context),
          fontWeight: FontWeight.w600,
        ),
      ),
    ),
  );
}

class _EmptyLine extends StatelessWidget {
  const _EmptyLine({super.key, required this.text, this.action});

  final String text;
  final Widget? action;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(
      horizontal: Grid.gutter,
      vertical: Grid.xxs,
    ),
    child: Row(
      children: [
        Expanded(
          child: Text(
            text,
            style: context.textTheme.bodyMedium?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
          ),
        ),
        ?action,
      ],
    ),
  );
}

class _Searching extends StatelessWidget {
  const _Searching();

  @override
  Widget build(BuildContext context) => const Padding(
    padding: EdgeInsets.all(Grid.xs),
    child: Center(
      child: KuzzLoadingIndicator(size: 20, semanticLabel: 'Searching'),
    ),
  );
}
