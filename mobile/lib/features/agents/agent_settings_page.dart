import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/load_error_view.dart';
import 'agent_settings.dart';
import 'agent_settings_provider.dart';
import 'my_agent_status_card.dart';

part 'agent_settings_page/agent_settings_form.dart';

/// Edit one of the owner's agents from the phone: its own name,
/// description, instructions and model (kind:30177). The agent's computer
/// picks up the change. Agents the phone cannot edit are shown read-only.
class AgentSettingsPage extends ConsumerWidget {
  const AgentSettingsPage({
    super.key,
    required this.agentPubkey,
    required this.fallbackName,
  });

  final String agentPubkey;

  /// Shown until (or unless) the agent's own record names it.
  final String fallbackName;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final args = AgentSettingsArgs(
      agentPubkey: agentPubkey,
      fallbackName: fallbackName,
    );
    final state = ref.watch(agentSettingsProvider(args));
    return switch (state) {
      AsyncData(value: final AgentSettingsEditable loaded) =>
        _AgentSettingsForm(key: ValueKey(loaded.event.id), loaded: loaded),
      AsyncData(value: AgentSettingsReadOnly(:final name, :final message)) =>
        _ReadOnly(agentPubkey: agentPubkey, name: name, message: message),
      AsyncError() => Scaffold(
        appBar: AppBar(title: const Text('Agent settings')),
        body: LoadErrorView(
          message: 'Couldn’t load this agent’s settings.',
          onRetry: () => ref.invalidate(agentSettingsProvider(args)),
        ),
      ),
      _ => Scaffold(
        appBar: AppBar(title: const Text('Agent settings')),
        body: const Center(child: KuzzLoadingIndicator()),
      ),
    };
  }
}

class _ReadOnly extends StatelessWidget {
  const _ReadOnly({
    required this.agentPubkey,
    required this.name,
    required this.message,
  });

  final String agentPubkey;
  final String name;
  final String message;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Agent settings')),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.symmetric(vertical: Grid.xs),
          children: [
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: Grid.sm),
              child: MyAgentStatusCard(agentPubkey: agentPubkey, name: name),
            ),
            AppListCard(
              key: const Key('agent-settings-read-only'),
              description: message,
              children: [
                AppListRowRaw(
                  leading: const SizedBox.shrink(),
                  title: Text(name, style: context.textTheme.bodyLarge),
                  subtitle: Text(
                    'Name',
                    style: context.textTheme.bodySmall?.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
