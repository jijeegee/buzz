import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';

import 'goal_tree.dart';

/// A goal title and optional note from [editGoal].
typedef GoalDraft = ({String title, String? note});

/// Runs a goal edit, reporting a failure in a snack bar.
Future<void> runGoalAction(
  BuildContext context,
  Future<void> Function() action,
) async {
  try {
    await action();
  } catch (error) {
    if (!context.mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(error.toString().replaceFirst('Exception: ', ''))),
    );
  }
}

/// Asks for a goal title and note; null when cancelled.
Future<GoalDraft?> editGoal(
  BuildContext context, {
  required String title,
  String initialTitle = '',
  String initialNote = '',
  String saveLabel = 'Save',
}) => showDialog<GoalDraft>(
  context: context,
  builder: (_) => _GoalEditorDialog(
    heading: title,
    initialTitle: initialTitle,
    initialNote: initialNote,
    saveLabel: saveLabel,
  ),
);

class _GoalEditorDialog extends HookWidget {
  const _GoalEditorDialog({
    required this.heading,
    required this.initialTitle,
    required this.initialNote,
    required this.saveLabel,
  });

  final String heading;
  final String initialTitle;
  final String initialNote;
  final String saveLabel;

  @override
  Widget build(BuildContext context) {
    final titleController = useTextEditingController(text: initialTitle);
    final noteController = useTextEditingController(text: initialNote);
    useListenable(titleController);
    final error = goalTitleError(titleController.text);
    return AlertDialog(
      title: Text(heading),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            key: const ValueKey('goal-editor-title'),
            controller: titleController,
            autofocus: true,
            maxLines: 1,
            decoration: InputDecoration(
              labelText: 'Goal (one sentence)',
              errorText: titleController.text.isEmpty ? null : error,
            ),
          ),
          TextField(
            key: const ValueKey('goal-editor-note'),
            controller: noteController,
            minLines: 2,
            maxLines: 5,
            decoration: const InputDecoration(
              labelText: 'Key information (optional)',
            ),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const ValueKey('goal-editor-save'),
          onPressed: error != null
              ? null
              : () => Navigator.pop<GoalDraft>(context, (
                  title: titleController.text.trim(),
                  note: noteController.text.trim().isEmpty
                      ? null
                      : noteController.text,
                )),
          child: Text(saveLabel),
        ),
      ],
    );
  }
}
