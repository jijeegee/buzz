import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/utils/string_utils.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../people/person_row.dart';
import '../profile/user_profile_sheet.dart';
import '../settings/agents_settings.dart';

/// The Agents tab: my agents, then the community's other agents, and the
/// agent settings. Agents run on a computer; this tab lists them and opens
/// their profile (with Message).
class AgentsPage extends HookConsumerWidget {
  const AgentsPage({this.tabReselection, super.key});

  /// Notifies this page when its already-selected tab is tapped again.
  final ValueListenable<int>? tabReselection;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final me = ref.watch(myPubkeyProvider)?.toLowerCase() ?? '';
    final directoryAsync = ref.watch(agentDirectoryProvider);
    final owners = ref.watch(agentOwnersProvider).value ?? const {};
    final filterController = useTextEditingController();
    final filter = useState('');
    final directory = directoryAsync.value ?? const <AgentDirectoryEntry>[];
    final pubkeys = [for (final agent in directory) agent.pubkey];
    useEffect(() {
      final wanted = {...pubkeys, ...owners.values.map((o) => o.toLowerCase())};
      if (wanted.isEmpty) return null;
      Future.microtask(
        () => ref.read(userCacheProvider.notifier).preload(wanted.toList()),
      );
      return null;
    }, [pubkeys.join(','), owners.length]);
    final profiles = ref.watch(userCacheProvider);
    String nameOf(AgentDirectoryEntry agent) {
      final name = profiles[agent.pubkey]?.displayName?.trim();
      if (name != null && name.isNotEmpty) return name;
      final display = agent.displayName?.trim();
      if (display != null && display.isNotEmpty) return display;
      return shortPubkey(agent.pubkey);
    }

    String ownerOf(AgentDirectoryEntry agent) =>
        (owners[agent.pubkey] ?? profiles[agent.pubkey]?.ownerPubkey ?? '')
            .toLowerCase();

    final needle = filter.value.trim().toLowerCase();
    final visible =
        [
          for (final agent in directory)
            if (needle.isEmpty || nameOf(agent).toLowerCase().contains(needle))
              agent,
        ]..sort(
          (a, b) => nameOf(a).toLowerCase().compareTo(nameOf(b).toLowerCase()),
        );
    final mine = [
      for (final agent in visible)
        if (me.isNotEmpty && ownerOf(agent) == me) agent,
    ];
    final others = [
      for (final agent in visible)
        if (!(me.isNotEmpty && ownerOf(agent) == me)) agent,
    ];

    String ownerLabel(AgentDirectoryEntry agent) {
      final owner = ownerOf(agent);
      if (owner.isEmpty) return 'Agent';
      final name = profiles[owner]?.displayName?.trim();
      return 'Agent · ${name == null || name.isEmpty ? shortPubkey(owner) : name}';
    }

    Widget agentRow(AgentDirectoryEntry agent, String subtitle) => PersonRow(
      key: ValueKey('agents-row-${agent.pubkey}'),
      pubkey: agent.pubkey,
      name: nameOf(agent),
      subtitle: subtitle,
      onTap: () => showUserProfileSheet(context, agent.pubkey),
    );

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
    Widget note(String text) => Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Grid.gutter,
        vertical: Grid.xs,
      ),
      child: Text(
        text,
        style: context.textTheme.bodySmall?.copyWith(
          color: context.colors.onSurfaceVariant,
        ),
      ),
    );

    return FrostedScaffold(
      backgroundColor: context.colors.surface,
      appBar: FrostedAppBar(
        automaticallyImplyLeading: false,
        horizontalInset: Grid.gutter,
        showBottomDivider: true,
        bottomDividerOpacity: 0.07,
        title: Text('Agents', style: headerTitleStyle),
        titleStyle: headerTitleStyle,
        bottomHeight: Grid.xxs,
        bottom: const SizedBox.expand(),
      ),
      body: ListView(
        key: const ValueKey('agents-list'),
        padding: EdgeInsets.only(
          top: topSectionHeight,
          bottom: MediaQuery.paddingOf(context).bottom + Grid.xs,
        ),
        children: [
          PeopleFilterField(
            controller: filterController,
            hint: 'Find agents',
            onChanged: (value) => filter.value = value,
          ),
          if (directoryAsync.isLoading && directory.isEmpty)
            const Padding(
              padding: EdgeInsets.all(Grid.sm),
              child: Center(child: KuzzLoadingIndicator()),
            )
          else ...[
            const PeopleSectionTitle('My agents'),
            if (mine.isEmpty)
              note(
                needle.isEmpty
                    ? 'No agents yet. Create one in the desktop app.'
                    : 'None of your agents match that name.',
              )
            else
              for (final agent in mine) agentRow(agent, 'Agent'),
            if (others.isNotEmpty) ...[
              const PeopleSectionTitle('Other agents'),
              for (final agent in others) agentRow(agent, ownerLabel(agent)),
            ],
          ],
          const PeopleSectionTitle('Settings'),
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: Grid.gutter),
            child: AgentsSettings(),
          ),
        ],
      ),
    );
  }
}
