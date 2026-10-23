import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';

/// Builds the rows contributed by one experiment.
typedef ExperimentRowsBuilder =
    List<Widget> Function(BuildContext context, WidgetRef ref);

/// Mobile experiments, in display order. Add a builder here to surface a new
/// experiment under Settings → Experiments.
const List<ExperimentRowsBuilder> mobileExperiments = [];

/// The Experiments row in Settings. Experiments are switched on their own
/// screen, not on the Settings page; the row hides when there are none.
class ExperimentsSettings extends ConsumerWidget {
  const ExperimentsSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (mobileExperiments.isEmpty) return const SizedBox.shrink();
    return AppListCard(
      key: const ValueKey('settings-experiments'),
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const ValueKey('settings-experiments-row'),
          icon: LucideIcons.flaskConical,
          title: context.l10n.settingsExperimentsTitle,
          trailing: Icon(
            LucideIcons.chevronRight,
            size: 18,
            color: context.colors.onSurfaceVariant,
          ),
          onTap: () => Navigator.of(context).push(
            MaterialPageRoute<void>(builder: (_) => const ExperimentsPage()),
          ),
        ),
      ],
    );
  }
}

/// Settings → Experiments: features that are still being refined.
class ExperimentsPage extends ConsumerWidget {
  const ExperimentsPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final rows = [
      for (final experiment in mobileExperiments) ...experiment(context, ref),
    ];
    return FrostedScaffold(
      useUtilitySurfaceTheme: true,
      appBar: FrostedAppBar(title: Text(context.l10n.settingsExperimentsTitle)),
      body: ListView(
        padding: EdgeInsets.only(
          top: frostedAppBarHeight(context),
          bottom: Grid.xs,
        ),
        children: [
          AppListCard(
            key: const ValueKey('experiments-list'),
            description: context.l10n.settingsExperimentsDescription,
            verticalPadding: Grid.twelve,
            children: rows,
          ),
        ],
      ),
    );
  }
}
