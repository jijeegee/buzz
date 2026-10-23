part of '../compose_bar.dart';

class _PhotoGalleryPicker extends StatelessWidget {
  final VoidCallback onBack;
  final Future<List<XFile>> Function() onPickAllPhotos;
  final Future<void> Function(List<XFile> photos) onChoosePhotos;
  final Future<void> Function(List<XFile> photos) onChooseAllPhotos;

  const _PhotoGalleryPicker({
    required this.onBack,
    required this.onPickAllPhotos,
    required this.onChoosePhotos,
    required this.onChooseAllPhotos,
  });

  @override
  Widget build(BuildContext context) {
    final fallback = _RecentPhotoGalleryPicker(
      onBack: onBack,
      onPickAllPhotos: onPickAllPhotos,
      onChoosePhotos: onChoosePhotos,
      onChooseAllPhotos: onChooseAllPhotos,
    );
    if (defaultTargetPlatform != TargetPlatform.iOS) return fallback;
    return _IOSInlinePhotoPicker(
      onBack: onBack,
      onPickAllPhotos: onPickAllPhotos,
      onChoosePhotos: onChoosePhotos,
      onChooseAllPhotos: onChooseAllPhotos,
      fallback: fallback,
    );
  }
}

class _RecentPhotoGalleryPicker extends HookConsumerWidget {
  final VoidCallback onBack;
  final Future<List<XFile>> Function() onPickAllPhotos;
  final Future<void> Function(List<XFile> photos) onChoosePhotos;
  final Future<void> Function(List<XFile> photos) onChooseAllPhotos;

  const _RecentPhotoGalleryPicker({
    required this.onBack,
    required this.onPickAllPhotos,
    required this.onChoosePhotos,
    required this.onChooseAllPhotos,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final recentPhotos = useMemoized(
      ref.read(photoLibraryProvider).loadRecentPhotos,
    );
    final recentSnapshot = useFuture(recentPhotos);
    final selection = useState<List<RecentPhoto>>([]);
    final isResolving = useState(false);
    final actionError = useState<String?>(null);
    final reducedMotion = MediaQuery.disableAnimationsOf(context);

    void togglePhoto(RecentPhoto photo) {
      if (isResolving.value) return;
      final current = selection.value;
      final existingIndex = current.indexWhere((item) => item.id == photo.id);
      selection.value = existingIndex < 0
          ? [...current, photo]
          : [
              ...current.take(existingIndex),
              ...current.skip(existingIndex + 1),
            ];
    }

    Future<void> choosePhotos() async {
      if (isResolving.value) return;
      isResolving.value = true;
      actionError.value = null;
      try {
        final photos = selection.value.isEmpty
            ? await onPickAllPhotos()
            : await ref
                  .read(photoLibraryProvider)
                  .resolveSelectedPhotos(selection.value);
        if (photos.isNotEmpty && context.mounted) {
          await (selection.value.isEmpty ? onChooseAllPhotos : onChoosePhotos)(
            photos,
          );
        }
      } catch (_) {
        if (context.mounted) {
          actionError.value = selection.value.isEmpty
              ? context.l10n.composePhotoLibraryOpenError
              : context.l10n.composePhotosPrepareError;
        }
      } finally {
        if (context.mounted) isResolving.value = false;
      }
    }

    final selectedCount = selection.value.length;
    final actionLabel = selectedCount == 0
        ? context.l10n.composeAllPhotos
        : context.l10n.composeAddPhotos(selectedCount);

    Widget buildGalleryBody() {
      if (recentSnapshot.connectionState != ConnectionState.done) {
        return Center(
          child: KuzzLoadingIndicator(
            size: 44,
            semanticLabel: context.l10n.composeRecentPhotosLoading,
          ),
        );
      }
      if (recentSnapshot.hasError) {
        return _PhotoGalleryMessage(
          icon: LucideIcons.images,
          title: context.l10n.composeRecentPhotosUnavailable,
          message: context.l10n.composeRecentPhotosUnavailableBody,
        );
      }

      final photos = recentSnapshot.data ?? const <RecentPhoto>[];
      if (photos.isEmpty) {
        return _PhotoGalleryMessage(
          icon: LucideIcons.images,
          title: context.l10n.composeRecentPhotosEmpty,
          message: context.l10n.composeRecentPhotosEmptyBody,
        );
      }
      return GridView.builder(
        key: const ValueKey('recent-photo-grid'),
        padding: EdgeInsets.zero,
        gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
          crossAxisCount: 4,
          crossAxisSpacing: Grid.quarter,
          mainAxisSpacing: Grid.quarter,
        ),
        itemCount: photos.length,
        itemBuilder: (context, index) {
          final photo = photos[index];
          final selectionIndex = selection.value.indexWhere(
            (item) => item.id == photo.id,
          );
          return _RecentPhotoTile(
            photo: photo,
            selectionIndex: selectionIndex,
            reducedMotion: reducedMotion,
            onTap: () => _runComposerAction(() => togglePhoto(photo)),
          );
        },
      );
    }

    return Padding(
      key: const ValueKey('photo-gallery-picker'),
      padding: const EdgeInsets.all(Grid.xxs),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          SizedBox(
            height: 40,
            child: Row(
              children: [
                if (Theme.of(context).platform == TargetPlatform.iOS)
                  IosGlassNavigationButton(
                    key: const ValueKey('photo-gallery-back'),
                    icon: IosGlassNavigationIcon.back,
                    semanticLabel: context.l10n.composeBackToAttachments,
                    onPressed: isResolving.value
                        ? null
                        : () => _runComposerAction(onBack),
                    width: 40,
                    height: 40,
                  )
                else
                  IconButton(
                    key: const ValueKey('photo-gallery-back'),
                    onPressed: isResolving.value
                        ? null
                        : () => _runComposerAction(onBack),
                    tooltip: context.l10n.composeBackToAttachments,
                    visualDensity: VisualDensity.compact,
                    icon: const Icon(LucideIcons.arrowLeft, size: 20),
                  ),
                const SizedBox(width: Grid.quarter),
                Expanded(
                  child: Text(
                    context.l10n.composeRecentPhotos,
                    style: context.textTheme.titleSmall?.copyWith(
                      color: context.colors.onSurface,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                if (selectedCount > 0)
                  Padding(
                    padding: const EdgeInsets.only(right: Grid.half),
                    child: Text(
                      context.l10n.composeGallerySelectedCount(selectedCount),
                      style: context.textTheme.labelMedium?.copyWith(
                        color: context.colors.onSurfaceVariant,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(height: Grid.half),
          Expanded(
            child: Column(
              children: [
                Expanded(child: buildGalleryBody()),
                if (actionError.value case final error?) ...[
                  const SizedBox(height: Grid.half),
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxHeight: 72),
                    child: SingleChildScrollView(
                      child: Text(
                        error,
                        key: const ValueKey('photo-gallery-error'),
                        style: context.textTheme.bodySmall?.copyWith(
                          color: context.colors.error,
                        ),
                        textAlign: TextAlign.center,
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(height: Grid.xxs),
          SizedBox(
            width: double.infinity,
            child: selectedCount == 0
                ? OutlinedButton.icon(
                    key: const ValueKey('photo-gallery-action'),
                    onPressed: isResolving.value
                        ? null
                        : () => _runComposerAction(
                            () => unawaited(choosePhotos()),
                          ),
                    icon: isResolving.value
                        ? KuzzLoadingIndicator(
                            size: 22,
                            color: context.colors.primary,
                            semanticLabel: context.l10n.composeOpeningAllPhotos,
                          )
                        : const Icon(LucideIcons.images, size: 18),
                    label: Text(actionLabel),
                  )
                : FilledButton.icon(
                    key: const ValueKey('photo-gallery-action'),
                    onPressed: isResolving.value
                        ? null
                        : () => _runComposerAction(
                            () => unawaited(choosePhotos()),
                          ),
                    icon: isResolving.value
                        ? KuzzLoadingIndicator(
                            size: 22,
                            color: Colors.white,
                            semanticLabel: context.l10n.composePhotosPreparing,
                          )
                        : const Icon(LucideIcons.plus, size: 18),
                    label: Text(actionLabel),
                  ),
          ),
        ],
      ),
    );
  }
}

class _RecentPhotoTile extends StatelessWidget {
  final RecentPhoto photo;
  final int selectionIndex;
  final bool reducedMotion;
  final VoidCallback onTap;

  const _RecentPhotoTile({
    required this.photo,
    required this.selectionIndex,
    required this.reducedMotion,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final isSelected = selectionIndex >= 0;
    return Semantics(
      button: true,
      selected: isSelected,
      label: isSelected
          ? context.l10n.composePhotoSelected(selectionIndex + 1)
          : context.l10n.composeSelectPhoto,
      child: GestureDetector(
        key: ValueKey('recent-photo-${photo.id}'),
        behavior: HitTestBehavior.opaque,
        onTap: onTap,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(Radii.sm),
              child: Image.memory(
                photo.thumbnailBytes,
                fit: BoxFit.cover,
                gaplessPlayback: true,
              ),
            ),
            AnimatedContainer(
              duration: reducedMotion
                  ? Duration.zero
                  : const Duration(milliseconds: 140),
              curve: Curves.easeOutCubic,
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(Radii.sm),
                border: Border.all(
                  color: isSelected
                      ? context.colors.primary
                      : Colors.transparent,
                  width: isSelected ? 3 : 0,
                ),
                color: isSelected
                    ? Colors.black.withValues(alpha: 0.08)
                    : Colors.transparent,
              ),
            ),
            PositionedDirectional(
              top: Grid.half,
              end: Grid.half,
              child: AnimatedScale(
                scale: isSelected ? 1 : 0.8,
                duration: reducedMotion
                    ? Duration.zero
                    : const Duration(milliseconds: 140),
                curve: Curves.easeOutCubic,
                child: AnimatedOpacity(
                  opacity: isSelected ? 1 : 0,
                  duration: reducedMotion
                      ? Duration.zero
                      : const Duration(milliseconds: 100),
                  child: Container(
                    key: ValueKey('photo-selection-index-${photo.id}'),
                    width: 24,
                    height: 24,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: context.colors.primary,
                      shape: BoxShape.circle,
                      border: Border.all(color: Colors.white, width: 1.5),
                    ),
                    child: Text(
                      '${selectionIndex + 1}',
                      style: context.textTheme.labelSmall?.copyWith(
                        color: context.colors.onPrimary,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _PhotoGalleryMessage extends StatelessWidget {
  final IconData icon;
  final String title;
  final String message;

  const _PhotoGalleryMessage({
    required this.icon,
    required this.title,
    required this.message,
  });

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(Grid.sm),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 32, color: context.colors.onSurfaceVariant),
            const SizedBox(height: Grid.xxs),
            Text(
              title,
              style: context.textTheme.titleSmall?.copyWith(
                color: context.colors.onSurface,
              ),
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: Grid.quarter),
            Text(
              message,
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
              textAlign: TextAlign.center,
            ),
          ],
        ),
      ),
    );
  }
}
