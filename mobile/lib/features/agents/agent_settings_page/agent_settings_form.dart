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
    final lockedName = useTextEditingController(text: loaded.agentName);
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
      final l10n = context.l10n;
      try {
        await saveAgentSettings(
          ref.read(agentSettingsRelayProvider),
          loaded,
          draft,
        );
        final savedName = loaded.nameEditable
            ? draft.name.trim()
            : loaded.agentName.trim();
        messenger.showSnackBar(
          SnackBar(content: Text(l10n.agentsSettingsSaved(savedName))),
        );
        navigator.pop();
      } on AgentSettingsException catch (failure) {
        if (context.mounted) error.value = failure.message;
      } catch (failure) {
        if (context.mounted) {
          error.value = l10n.agentsSettingsSaveError('$failure');
        }
      } finally {
        if (context.mounted) saving.value = false;
      }
    }

    InputDecoration decoration(String label, {String? hint}) =>
        InputDecoration(labelText: label, hintText: hint);

    return Scaffold(
      appBar: AppBar(
        title: Text(context.l10n.agentsSettingsTitle),
        actions: [
          TextButton(
            key: const Key('agent-settings-save'),
            onPressed: changed && !saving.value
                ? () => unawaited(save())
                : null,
            child: Text(
              saving.value
                  ? context.l10n.agentsSaving
                  : context.l10n.agentsSave,
            ),
          ),
        ],
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            MyAgentStatusCard(
              agentPubkey: loaded.agentPubkey,
              name: loaded.agentName,
            ),
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
            if (loaded.nameEditable)
              TextField(
                key: const Key('agent-settings-name'),
                controller: name,
                enabled: !saving.value,
                textInputAction: TextInputAction.next,
                decoration: decoration(context.l10n.agentsFieldName),
              )
            else
              TextField(
                key: const Key('agent-settings-name-locked'),
                controller: lockedName,
                enabled: false,
                decoration: decoration(
                  context.l10n.agentsFieldName,
                ).copyWith(helperText: agentSettingsNameLockedMessage),
              ),
            const SizedBox(height: Grid.xs),
            TextField(
              key: const Key('agent-settings-description'),
              controller: description,
              enabled: !saving.value,
              minLines: 1,
              maxLines: 3,
              keyboardType: TextInputType.text,
              textInputAction: TextInputAction.next,
              decoration: decoration(
                context.l10n.agentsFieldDescription,
                hint: context.l10n.agentsFieldDescriptionHint,
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
                context.l10n.agentsFieldInstructions,
                hint: context.l10n.agentsFieldInstructionsHint,
              ).copyWith(alignLabelWithHint: true),
            ),
            const SizedBox(height: Grid.xs),
            TextField(
              key: const Key('agent-settings-model'),
              controller: model,
              enabled: !saving.value,
              autocorrect: false,
              decoration: decoration(
                context.l10n.agentsFieldModel,
                hint: context.l10n.agentsFieldModelHint,
              ),
            ),
            const SizedBox(height: Grid.xs),
            Text(
              context.l10n.agentsSettingsApplyNote,
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
