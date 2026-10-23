import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';

final _usernamePattern = RegExp(r'^[a-z0-9_]{3,32}$');

/// The global account profile of a token relay (`PATCH /auth/profile`):
/// display name, username and photo, shared by every community on it.
class AccountPage extends ConsumerWidget {
  const AccountPage({super.key, required this.origin});

  /// Canonical origin of the token relay.
  final String origin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(accountProfileProvider(origin));
    // A reload after saving keeps the form (and its edits) mounted.
    final loaded = profile.value;
    return Scaffold(
      appBar: AppBar(title: Text(context.l10n.settingsAccountTitle)),
      body: SafeArea(
        child: switch (profile) {
          _ when loaded != null => _AccountForm(
            key: ValueKey(loaded.principalId),
            origin: origin,
            profile: loaded,
          ),
          AsyncError(:final error) => _LoadFailure(
            message: '$error',
            onRetry: () => ref.invalidate(accountProfileProvider(origin)),
          ),
          _ => Center(
            child: KuzzLoadingIndicator(
              size: 32,
              semanticLabel: context.l10n.settingsAccountLoading,
            ),
          ),
        },
      ),
    );
  }
}

class _LoadFailure extends StatelessWidget {
  const _LoadFailure({required this.message, required this.onRetry});

  final String message;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(Grid.sm),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Semantics(
              liveRegion: true,
              child: Text(
                context.l10n.settingsAccountLoadFailed(message),
                textAlign: TextAlign.center,
                style: context.textTheme.bodyMedium,
              ),
            ),
            const SizedBox(height: Grid.xs),
            FilledButton(
              key: const Key('account-load-retry'),
              onPressed: onRetry,
              child: Text(context.l10n.commonTryAgain),
            ),
          ],
        ),
      ),
    );
  }
}

class _AccountForm extends HookConsumerWidget {
  const _AccountForm({super.key, required this.origin, required this.profile});

  final String origin;
  final AccountProfile profile;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final name = useTextEditingController(text: profile.displayName);
    final username = useTextEditingController(text: profile.username ?? '');
    final avatarUrl = useState<String?>(profile.avatarUrl);
    final uploading = useState(false);
    final saving = useState(false);
    final error = useState<String?>(null);
    final busy = uploading.value || saving.value;

    Future<void> changePhoto() async {
      error.value = null;
      uploading.value = true;
      try {
        final blob = await ref
            .read(mediaUploadServiceProvider)
            .pickAndUploadImage();
        if (blob != null && context.mounted) avatarUrl.value = blob.url;
      } catch (failure) {
        if (context.mounted) {
          error.value = context.l10n.settingsAccountPhotoUploadFailed(
            '$failure',
          );
        }
      } finally {
        if (context.mounted) uploading.value = false;
      }
    }

    /// One `PATCH /auth/profile` with every field: a single atomic write.
    Future<void> save() async {
      final displayName = name.text.trim();
      final handle = username.text.trim();
      if (displayName.isEmpty || displayName.characters.length > 64) {
        error.value = context.l10n.settingsAccountDisplayNameInvalid;
        return;
      }
      if (handle.isNotEmpty && !_usernamePattern.hasMatch(handle)) {
        error.value = context.l10n.settingsAccountUsernameInvalid;
        return;
      }
      error.value = null;
      saving.value = true;
      // Outlives this page if it is closed while the save is in flight.
      final container = ProviderScope.containerOf(context, listen: false);
      final AccountProfile saved;
      try {
        saved = await ref
            .read(accountApiProvider(origin))
            .updateProfile(
              displayName: displayName,
              avatarUrl: avatarUrl.value,
              username: handle.isEmpty ? null : handle,
            );
      } on AccountApiException catch (failure) {
        if (context.mounted) {
          saving.value = false;
          error.value = failure.code == 'username_taken'
              ? context.l10n.settingsAccountUsernameTaken
              : context.l10n.settingsAccountSaveFailed(failure.message);
        }
        return;
      }
      // Names and avatars elsewhere read the user cache: show the saved
      // profile there now instead of waiting for the next relay fetch.
      final key = saved.principalId.toLowerCase();
      final previous = container.read(userCacheProvider)[key];
      container
          .read(userCacheProvider.notifier)
          .put(
            UserProfile(
              pubkey: key,
              displayName: saved.displayName,
              avatarUrl: saved.avatarUrl,
              about: previous?.about,
              nip05Handle: previous?.nip05Handle,
              ownerPubkey: previous?.ownerPubkey,
            ),
          );
      container.invalidate(accountProfileProvider(origin));
      if (!context.mounted) return;
      saving.value = false;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(context.l10n.settingsAccountProfileSaved)),
      );
    }

    return ListView(
      padding: const EdgeInsets.all(Grid.sm),
      children: [
        Center(
          child: Semantics(
            image: true,
            label: avatarUrl.value == null
                ? context.l10n.settingsAccountNoPhoto
                : context.l10n.settingsAccountPhoto,
            child: ExcludeSemantics(
              child: AvatarImage(
                imageUrl: avatarUrl.value,
                radius: 40,
                fallback: Icon(
                  LucideIcons.user,
                  size: 32,
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ),
          ),
        ),
        const SizedBox(height: Grid.xxs),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            TextButton(
              key: const Key('account-avatar-change'),
              onPressed: busy ? null : () => unawaited(changePhoto()),
              child: Text(context.l10n.settingsAccountChangePhoto),
            ),
            if (avatarUrl.value != null)
              TextButton(
                key: const Key('account-avatar-remove'),
                onPressed: busy ? null : () => avatarUrl.value = null,
                child: Text(context.l10n.settingsAccountRemovePhoto),
              ),
          ],
        ),
        const SizedBox(height: Grid.xs),
        TextField(
          key: const Key('account-display-name'),
          controller: name,
          enabled: !busy,
          maxLength: 64,
          textInputAction: TextInputAction.next,
          decoration: InputDecoration(
            labelText: context.l10n.settingsAccountDisplayNameLabel,
          ),
        ),
        const SizedBox(height: Grid.xxs),
        TextField(
          key: const Key('account-username'),
          controller: username,
          enabled: !busy,
          maxLength: 32,
          autocorrect: false,
          textInputAction: TextInputAction.done,
          decoration: InputDecoration(
            labelText: context.l10n.settingsAccountUsernameLabel,
            prefixText: '@',
            helperText: context.l10n.settingsAccountUsernameHelper,
          ),
        ),
        if (error.value != null) ...[
          const SizedBox(height: Grid.xxs),
          Semantics(
            liveRegion: true,
            child: Text(
              error.value!,
              key: const Key('account-error'),
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
        ],
        const SizedBox(height: Grid.sm),
        FilledButton(
          key: const Key('account-save'),
          onPressed: busy ? null : () => unawaited(save()),
          child: saving.value
              ? KuzzLoadingIndicator(
                  size: 20,
                  semanticLabel: context.l10n.commonSaving,
                )
              : Text(context.l10n.commonSave),
        ),
        const SizedBox(height: Grid.xs),
        Text(
          context.l10n.settingsAccountSharedNote,
          style: context.textTheme.bodySmall?.copyWith(
            color: context.colors.onSurfaceVariant,
          ),
        ),
      ],
    );
  }
}
