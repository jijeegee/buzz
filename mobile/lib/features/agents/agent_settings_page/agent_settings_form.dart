part of '../agent_settings_page.dart';

class _AgentSettingsForm extends HookConsumerWidget {
  const _AgentSettingsForm({super.key, required this.loaded});

  final AgentSettingsEditable loaded;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final initial = loaded.draft;
    final name = useTextEditingController(text: initial.name);
    final description = useTextEditingController(text: initial.description);
    final instructions = useTextEditingController(text: initial.instructions);
    final model = useTextEditingController(text: initial.model);
    final fields = useMemoized(
      () => Listenable.merge([name, description, instructions, model]),
      [name, description, instructions, model],
    );
    useListenable(fields);
    final saving = useState(false);
    final error = useState<String?>(null);

    final draft = AgentSettingsDraft(
      name: name.text,
      description: description.text,
      instructions: instructions.text,
      model: model.text,
    );
    final changed = draft != initial;

    Future<void> save() async {
      if (saving.value) return;
      saving.value = true;
      error.value = null;
      final messenger = ScaffoldMessenger.of(context);
      final navigator = Navigator.of(context);
      try {
        await saveAgentSettings(
          ref.read(agentSettingsRelayProvider),
          loaded,
          draft,
        );
        messenger.showSnackBar(
          SnackBar(
            content: Text(
              'Saved. ${draft.name.trim()} picks up the change on its '
              'computer.',
            ),
          ),
        );
        navigator.pop();
      } on AgentSettingsException catch (failure) {
        if (context.mounted) error.value = failure.message;
      } catch (failure) {
        if (context.mounted) error.value = 'Couldn’t save: $failure';
      } finally {
        if (context.mounted) saving.value = false;
      }
    }

    InputDecoration decoration(String label, {String? hint}) =>
        InputDecoration(labelText: label, hintText: hint);

    return Scaffold(
      appBar: AppBar(
        title: const Text('Agent settings'),
        actions: [
          TextButton(
            key: const Key('agent-settings-save'),
            onPressed: changed && !saving.value
                ? () => unawaited(save())
                : null,
            child: Text(saving.value ? 'Saving…' : 'Save'),
          ),
        ],
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            if (error.value case final message?)
              Padding(
                padding: const EdgeInsets.only(bottom: Grid.xs),
                child: Text(
                  message,
                  key: const Key('agent-settings-error'),
                  style: context.textTheme.bodyMedium?.copyWith(
                    color: context.colors.error,
                  ),
                ),
              ),
            TextField(
              key: const Key('agent-settings-name'),
              controller: name,
              enabled: !saving.value,
              textInputAction: TextInputAction.next,
              decoration: decoration('Name'),
            ),
            const SizedBox(height: Grid.xs),
            TextField(
              key: const Key('agent-settings-description'),
              controller: description,
              enabled: !saving.value,
              minLines: 1,
              maxLines: 3,
              decoration: decoration(
                'Description',
                hint: 'A short public description',
              ),
            ),
            const SizedBox(height: Grid.xs),
            TextField(
              key: const Key('agent-settings-instructions'),
              controller: instructions,
              enabled: !saving.value,
              minLines: 6,
              maxLines: 16,
              keyboardType: TextInputType.multiline,
              decoration: decoration(
                'Instructions',
                hint: 'How this agent should work',
              ).copyWith(alignLabelWithHint: true),
            ),
            const SizedBox(height: Grid.xs),
            TextField(
              key: const Key('agent-settings-model'),
              controller: model,
              enabled: !saving.value,
              autocorrect: false,
              decoration: decoration('Model', hint: 'Default'),
            ),
            const SizedBox(height: Grid.xs),
            Text(
              'Your agent’s computer applies the change. A running agent '
              'restarts when it is idle.',
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
