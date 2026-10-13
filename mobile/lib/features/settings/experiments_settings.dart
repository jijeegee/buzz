import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/theme/grid.dart';
import '../../shared/widgets/app_list_card.dart';
import 'read_aloud_settings.dart';

/// Builds the rows contributed by one experiment.
typedef ExperimentRowsBuilder =
    List<Widget> Function(BuildContext context, WidgetRef ref);

/// Mobile experiments, in display order. Add a builder here to surface a new
/// experiment in Settings → Experiments.
const List<ExperimentRowsBuilder> mobileExperiments = [readAloudExperimentRows];

/// Settings card for device-local experimental features.
class ExperimentsSettings extends ConsumerWidget {
  const ExperimentsSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final rows = [
      for (final experiment in mobileExperiments) ...experiment(context, ref),
    ];
    if (rows.isEmpty) return const SizedBox.shrink();
    return AppListCard(
      key: const ValueKey('settings-experiments'),
      label: 'Experiments',
      description:
          'Features that are still being refined. They may change or be '
          'removed.',
      verticalPadding: Grid.twelve,
      children: rows,
    );
  }
}
