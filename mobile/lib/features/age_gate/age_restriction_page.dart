import 'package:flutter/material.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';

class AgeRestrictionPage extends StatelessWidget {
  const AgeRestrictionPage({super.key});

  @override
  Widget build(BuildContext context) {
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
                      Icons.lock_outline,
                      size: 48,
                      color: context.colors.primary,
                    ),
                  ),
                  const SizedBox(height: Grid.lg),
                  Text(
                    context.l10n.ageGateTitle,
                    textAlign: TextAlign.center,
                    style: context.textTheme.headlineSmall?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: Grid.sm),
                  Text(
                    context.l10n.ageGateBody,
                    textAlign: TextAlign.center,
                    style: context.textTheme.bodyLarge?.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
