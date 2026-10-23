import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../l10n/l10n.dart';
import '../theme/theme.dart';
import 'app_update.dart';

/// Wraps the whole app: below the minimum version only the update screen is
/// shown; an optional update shows a bar at the bottom until the user picks
/// "Update" or "Later".
class AppUpdateGate extends ConsumerWidget {
  const AppUpdateGate({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final update = ref.watch(appUpdateProvider);
    if (update.blocking) return _RequiredUpdatePage(update: update);
    if (!update.offering) return child;
    return Stack(
      children: [
        child,
        Positioned(
          left: 0,
          right: 0,
          bottom: 0,
          child: _UpdateAvailableBar(update: update),
        ),
      ],
    );
  }
}

class _RequiredUpdatePage extends ConsumerWidget {
  const _RequiredUpdatePage({required this.update});

  final AppUpdateState update;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final notifier = ref.read(appUpdateProvider.notifier);
    return Scaffold(
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: Padding(
              padding: const EdgeInsets.all(Grid.xl),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ExcludeSemantics(
                    child: Icon(
                      Icons.system_update,
                      size: 48,
                      color: context.colors.primary,
                    ),
                  ),
                  const SizedBox(height: Grid.lg),
                  Text(
                    context.l10n.appUpdateRequiredTitle,
                    textAlign: TextAlign.center,
                    style: context.textTheme.headlineSmall?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: Grid.sm),
                  Text(
                    context.l10n.appUpdateRequiredBody,
                    textAlign: TextAlign.center,
                    style: context.textTheme.bodyLarge,
                  ),
                  const SizedBox(height: Grid.xl),
                  _UpdateButton(
                    update: update,
                    onUpdate: notifier.install,
                    onRetry: notifier.check,
                  ),
                  if (update.phase == AppUpdatePhase.failed) ...[
                    const SizedBox(height: Grid.sm),
                    Text(
                      context.l10n.appUpdateFailed,
                      textAlign: TextAlign.center,
                      style: context.textTheme.bodySmall?.copyWith(
                        color: context.colors.error,
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _UpdateAvailableBar extends ConsumerWidget {
  const _UpdateAvailableBar({required this.update});

  final AppUpdateState update;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final notifier = ref.read(appUpdateProvider.notifier);
    final busy =
        update.phase == AppUpdatePhase.downloading ||
        update.phase == AppUpdatePhase.installing;
    return Material(
      elevation: 8,
      color: context.colors.surfaceContainerHigh,
      child: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(
            Grid.lg,
            Grid.sm,
            Grid.sm,
            Grid.sm,
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  update.phase == AppUpdatePhase.failed
                      ? context.l10n.appUpdateFailed
                      : context.l10n.appUpdateAvailable(
                          update.release!.version,
                        ),
                  style: context.textTheme.bodyMedium,
                ),
              ),
              if (!busy)
                TextButton(
                  onPressed: notifier.dismiss,
                  child: Text(context.l10n.appUpdateLater),
                ),
              _UpdateButton(update: update, onUpdate: notifier.install),
            ],
          ),
        ),
      ),
    );
  }
}

class _UpdateButton extends StatelessWidget {
  const _UpdateButton({
    required this.update,
    required this.onUpdate,
    this.onRetry,
  });

  final AppUpdateState update;
  final Future<void> Function() onUpdate;
  final Future<void> Function()? onRetry;

  @override
  Widget build(BuildContext context) {
    switch (update.phase) {
      case AppUpdatePhase.downloading:
      case AppUpdatePhase.installing:
        final progress = update.progress;
        return FilledButton.icon(
          onPressed: null,
          icon: SizedBox.square(
            dimension: 16,
            child: CircularProgressIndicator(strokeWidth: 2, value: progress),
          ),
          label: Text(
            progress == null
                ? context.l10n.appUpdateDownloading
                : context.l10n.appUpdateDownloadingPercent(
                    (progress * 100).round(),
                  ),
          ),
        );
      case AppUpdatePhase.idle:
      case AppUpdatePhase.failed:
        // Forced before the relay named a build: only a re-check can help.
        if (update.release == null) {
          return FilledButton(
            onPressed: onRetry,
            child: Text(context.l10n.appUpdateRetry),
          );
        }
        return FilledButton(
          onPressed: onUpdate,
          child: Text(context.l10n.appUpdateInstall),
        );
    }
  }
}
