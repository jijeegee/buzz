import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/mentions/mention_candidates_provider.dart';
import 'agent_settings_provider.dart';
import 'external_agent_access.dart';
import 'external_agent_profile_sheet.dart';
import 'my_agent_actions.dart';
import 'owner_tools_row.dart';

/// A paired agent from Devices: its chat, and who it answers.
Future<void> showExternalAgentSheet(
  BuildContext page,
  WidgetRef pageRef, {
  required String agentPubkey,
  required String name,
}) => showKuzzModalBottomSheet<void>(
  context: page,
  title: name,
  isScrollControlled: true,
  useSafeArea: true,
  builder: (sheet) => _ExternalAgentSheet(
    agentPubkey: agentPubkey,
    name: name,
    onMessage: () {
      Navigator.of(sheet).pop();
      unawaited(openMyAgentDm(page, pageRef, agentPubkey, name));
    },
  ),
);

class _ExternalAgentSheet extends ConsumerWidget {
  const _ExternalAgentSheet({
    required this.agentPubkey,
    required this.name,
    required this.onMessage,
  });

  final String agentPubkey;
  final String name;
  final VoidCallback onMessage;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final access = ref.watch(externalAgentAccessProvider(agentPubkey));
    return SingleChildScrollView(
      padding: EdgeInsets.fromLTRB(
        Grid.xs,
        0,
        Grid.xs,
        Grid.xs + MediaQuery.viewInsetsOf(context).bottom,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          OutlinedButton.icon(
            key: const Key('agent-access-message'),
            onPressed: onMessage,
            icon: const Icon(LucideIcons.messageSquare, size: 18),
            label: Text(context.l10n.agentsMessage),
          ),
          const SizedBox(height: Grid.xxs),
          OutlinedButton.icon(
            key: const Key('agent-profile-edit'),
            onPressed: () => unawaited(
              showExternalAgentProfileSheet(
                context,
                agentPubkey: agentPubkey,
                name: name,
              ),
            ),
            icon: const Icon(LucideIcons.pencil, size: 18),
            label: Text(context.l10n.agentProfileEdit),
          ),
          const SizedBox(height: Grid.xs),
          Text(
            context.l10n.agentAccessBody(name),
            style: context.textTheme.bodyMedium?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: Grid.xxs),
          access.when(
            data: (loaded) => Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              mainAxisSize: MainAxisSize.min,
              children: [
                _AccessForm(
                  agentPubkey: agentPubkey,
                  name: name,
                  initial: loaded,
                ),
                OwnerToolsRow(
                  agentPubkey: agentPubkey,
                  respondClass: loaded.mode.wire,
                ),
              ],
            ),
            loading: () => const Padding(
              padding: EdgeInsets.all(Grid.xs),
              child: Center(child: KuzzLoadingIndicator()),
            ),
            error: (_, _) => Text(
              context.l10n.agentAccessLoadFailed,
              key: const Key('agent-access-load-failed'),
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _AccessForm extends HookConsumerWidget {
  const _AccessForm({
    required this.agentPubkey,
    required this.name,
    required this.initial,
  });

  final String agentPubkey;
  final String name;
  final ExternalAgentAccess initial;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final mode = useState(initial.mode);
    final people = useState(initial.allowlist);
    final query = useState('');
    final saving = useState(false);
    final error = useState<String?>(null);
    final profiles = ref.watch(userCacheProvider);

    useEffect(() {
      unawaited(
        Future<void>.microtask(
          () => ref.read(userCacheProvider.notifier).preload(initial.allowlist),
        ),
      );
      return null;
    }, const []);

    /// Never a key: an account without a name has none to show.
    String personName(String pubkey) {
      final name = profiles[pubkey.toLowerCase()]?.displayName?.trim();
      return name == null || name.isEmpty
          ? context.l10n.agentAccessUnnamed
          : name;
    }

    final me = ref.watch(myPubkeyProvider)?.toLowerCase();
    final results = query.value.trim().isEmpty
        ? null
        : ref.watch(mentionUserSearchProvider(query.value)).asData?.value.where(
            (user) {
              final key = user.pubkey.toLowerCase();
              return key != me && !people.value.contains(key);
            },
          ).toList();

    Future<void> save() async {
      final owner = ref.read(externalAccessWriterProvider);
      if (owner == null) {
        error.value = context.l10n.agentAccessUnavailable;
        return;
      }
      final navigator = Navigator.of(context);
      saving.value = true;
      error.value = null;
      try {
        // Owner tools first: with the level on, a failed call fails the save.
        final origin = ref.read(accountOriginProvider);
        if (origin != null) {
          await writeToolLevelBeforeSave(
            ref.read(accountApiProvider(origin)),
            agentPubkey,
            current: ref
                .read(agentToolLevelProvider(agentPubkey))
                .asData
                ?.value,
            respondClass: mode.value.wire,
          );
        }
        await saveExternalAgentAccess(
          ref.read(agentSettingsRelayProvider),
          owner: owner,
          agentPubkey: agentPubkey,
          name: name,
          mode: mode.value,
          allowlist: people.value,
        );
        ref.invalidate(externalAgentAccessProvider(agentPubkey));
        ref.invalidate(agentToolLevelProvider(agentPubkey));
        navigator.pop();
      } on AccountApiException catch (failure) {
        if (context.mounted) {
          error.value = toolLevelErrorMessage(context, failure);
          saving.value = false;
        }
      } catch (failure) {
        if (context.mounted) {
          debugPrint('[ExternalAgentAccess] save failed: $failure');
          error.value = context.l10n.agentAccessSaveFailed;
          saving.value = false;
        }
      }
    }

    Widget option(AgentAccessMode value, String label) => ListTile(
      key: Key('agent-access-mode-${value.wire}'),
      contentPadding: EdgeInsets.zero,
      leading: Icon(
        mode.value == value ? LucideIcons.circleCheck : LucideIcons.circle,
      ),
      title: Text(label),
      onTap: () => mode.value = value,
    );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        option(AgentAccessMode.ownerOnly, context.l10n.agentAccessOnlyMe),
        option(AgentAccessMode.allowlist, context.l10n.agentAccessSelected),
        option(AgentAccessMode.anyone, context.l10n.agentAccessAnyone),
        if (mode.value == AgentAccessMode.allowlist) ...[
          Text(
            context.l10n.agentAccessAlwaysYou,
            style: context.textTheme.bodySmall?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
          ),
          for (final person in people.value)
            ListTile(
              key: Key('agent-access-person-$person'),
              contentPadding: EdgeInsets.zero,
              dense: true,
              title: Text(personName(person)),
              trailing: IconButton(
                tooltip: context.l10n.agentAccessRemovePerson(
                  personName(person),
                ),
                icon: const Icon(LucideIcons.x, size: 18),
                onPressed: () => people.value = [
                  for (final other in people.value)
                    if (other != person) other,
                ],
              ),
            ),
          TextField(
            key: const Key('agent-access-search'),
            decoration: InputDecoration(
              hintText: context.l10n.agentAccessSearch,
            ),
            onChanged: (value) => query.value = value,
          ),
          if (results != null && results.isEmpty)
            Padding(
              padding: const EdgeInsets.only(top: Grid.xxs),
              child: Text(context.l10n.agentAccessNoMatches),
            ),
          for (final user in results ?? const [])
            ListTile(
              key: Key('agent-access-add-${user.pubkey.toLowerCase()}'),
              contentPadding: EdgeInsets.zero,
              dense: true,
              title: Text(personName(user.pubkey)),
              trailing: const Icon(LucideIcons.plus, size: 18),
              onTap: () {
                ref.read(userCacheProvider.notifier).put(user);
                people.value = [...people.value, user.pubkey.toLowerCase()];
              },
            ),
        ],
        if (error.value != null)
          Padding(
            padding: const EdgeInsets.only(top: Grid.xxs),
            child: Text(
              error.value!,
              key: const Key('agent-access-error'),
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
        const SizedBox(height: Grid.xs),
        FilledButton(
          key: const Key('agent-access-save'),
          onPressed:
              saving.value ||
                  (mode.value == AgentAccessMode.allowlist &&
                      people.value.isEmpty)
              ? null
              : () => unawaited(save()),
          child: Text(context.l10n.agentAccessSave),
        ),
      ],
    );
  }
}
