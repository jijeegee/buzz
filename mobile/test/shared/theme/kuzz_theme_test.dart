import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:kuzz/shared/widgets/frosted_app_bar.dart';

void main() {
  group('Kuzz theme catalog entries', () {
    test('both halves are in the catalog', () {
      expect(findTheme(kuzzThemeName), isNotNull);
      expect(findTheme(kuzzDarkThemeName), isNotNull);
    });

    test('borrow the GitHub palettes', () {
      final kuzz = findTheme(kuzzThemeName)!;
      final github = findTheme('github-light')!;
      expect(kuzz.bg, github.bg);
      expect(kuzz.fg, github.fg);
      expect(kuzz.comment, github.comment);

      final kuzzDark = findTheme(kuzzDarkThemeName)!;
      final githubDark = findTheme('github-dark')!;
      expect(kuzzDark.bg, githubDark.bg);
      expect(kuzzDark.fg, githubDark.fg);
      expect(kuzzDark.comment, githubDark.comment);
    });

    test('are a light/dark pair', () {
      expect(findTheme(kuzzThemeName)!.isDark, isFalse);
      expect(findTheme(kuzzDarkThemeName)!.isDark, isTrue);
      expect(themePairFor(kuzzThemeName), kuzzDarkThemeName);
      expect(themePairFor(kuzzDarkThemeName), kuzzThemeName);
    });

    test('appear as a single System-mode option labelled "Kuzz"', () {
      final paired = themeGroups().paired.map((t) => t.name);
      expect(paired, contains(kuzzThemeName));
      expect(paired, isNot(contains(kuzzDarkThemeName)));
      expect(pairedThemeLabel(kuzzThemeName), 'Kuzz');
      expect(themeSelectionLabel(kuzzThemeName, ThemeMode.system), 'Kuzz');
      expect(themeSelectionLabel(kuzzDarkThemeName, ThemeMode.system), 'Kuzz');
    });

    test('forces neutral rendering without changing the stored accent', () {
      const storedAccent = '#ef4444';

      expect(
        effectiveAccentIndex(kuzzThemeName, storedAccent),
        neutralAccentIndex,
      );
      expect(
        effectiveAccentIndex(kuzzDarkThemeName, storedAccent),
        neutralAccentIndex,
      );
      expect(
        effectiveAccentIndex('github-light', storedAccent),
        accentIndexForWireValue(storedAccent),
      );
      expect(storedAccent, '#ef4444');
    });

    test('resolve across brightnesses like any other pair', () {
      final resolved = resolveSchemes(kuzzThemeName, ThemeMode.system);
      expect(resolved.forcedMode, isNull);
      expect(resolved.light.brightness, Brightness.light);
      expect(resolved.dark.brightness, Brightness.dark);
      expect(resolved.lightTheme?.name, kuzzThemeName);
      expect(resolved.darkTheme?.name, kuzzDarkThemeName);

      expect(
        effectiveTheme(kuzzThemeName, ThemeMode.dark)?.name,
        kuzzDarkThemeName,
      );
      expect(
        effectiveTheme(kuzzDarkThemeName, ThemeMode.light)?.name,
        kuzzThemeName,
      );
    });

    test(
      'fallbacks expose the effective Kuzz theme for gradient selection',
      () {
        final coerced = resolveSchemes('nord', ThemeMode.light);
        expect(coerced.lightTheme?.name, kuzzThemeName);
        expect(
          kuzzTopSectionGradient(
            coerced.lightTheme!.name,
            coerced.light.brightness,
          ),
          isNotNull,
        );

        final unknown = resolveSchemes('not-a-theme', ThemeMode.light);
        expect(unknown.lightTheme?.name, kuzzThemeName);
        expect(
          kuzzTopSectionGradient(
            unknown.lightTheme!.name,
            unknown.light.brightness,
          ),
          isNotNull,
        );
      },
    );
  });

  group('kuzzTopSectionGradient', () {
    test('is null for non-Kuzz themes', () {
      expect(kuzzTopSectionGradient('github-light', Brightness.light), isNull);
      expect(kuzzTopSectionGradient('nord', Brightness.dark), isNull);
    });

    test('paints top to bottom for both halves of the pair', () {
      for (final name in [kuzzThemeName, kuzzDarkThemeName]) {
        final gradient = kuzzTopSectionGradient(name, Brightness.light);
        expect(gradient, isNotNull, reason: '$name should be gradient-backed');
        expect(gradient!.begin, Alignment.topCenter);
        expect(gradient.end, Alignment.bottomCenter);
        expect(gradient.colors, hasLength(2));
      }
    });

    test('brightness selects the stops, not the theme name', () {
      // Both halves enable the gradient, so System mode keeps it on across an
      // OS switch — the applied brightness alone decides which stops are used.
      final light = kuzzTopSectionGradient(kuzzThemeName, Brightness.light)!;
      final dark = kuzzTopSectionGradient(kuzzThemeName, Brightness.dark)!;

      expect(light.colors, isNot(dark.colors));
      expect(
        kuzzTopSectionGradient(kuzzDarkThemeName, Brightness.dark)!.colors,
        dark.colors,
      );
      expect(
        kuzzTopSectionGradient(kuzzDarkThemeName, Brightness.light)!.colors,
        light.colors,
      );
    });

    test('is opaque so the color replaces the frosted fill', () {
      for (final brightness in Brightness.values) {
        final gradient = kuzzTopSectionGradient(kuzzThemeName, brightness)!;
        for (final color in gradient.colors) {
          expect(color.a, 1.0);
        }
      }
    });
  });

  group('theme threading', () {
    BoxDecoration barDecoration(WidgetTester tester) {
      final container = tester
          .widgetList<Container>(
            find.descendant(
              of: find.byType(FrostedAppBar),
              matching: find.byType(Container),
            ),
          )
          .first;
      return container.decoration! as BoxDecoration;
    }

    Widget harness(ThemeData theme) => MaterialApp(
      theme: theme,
      home: Builder(
        builder: (context) => Stack(
          children: [
            FrostedAppBar(
              gradient: context.appColors.topSectionGradient,
              title: const Text('Home'),
            ),
          ],
        ),
      ),
    );

    testWidgets('AppTheme carries the gradient to the top section', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          AppTheme.light(
            topSectionGradient: kuzzTopSectionGradient(
              kuzzThemeName,
              Brightness.light,
            ),
          ),
        ),
      );

      final decoration = barDecoration(tester);
      expect(decoration.gradient, isNotNull);
      // A BoxDecoration cannot paint a color and a gradient at once.
      expect(decoration.color, isNull);
    });

    testWidgets('non-Kuzz themes keep the frosted surface fill', (
      tester,
    ) async {
      await tester.pumpWidget(harness(AppTheme.light()));

      final decoration = barDecoration(tester);
      expect(decoration.gradient, isNull);
      expect(decoration.color, isNotNull);
    });

    testWidgets('Kuzz section labels use 80% neutral foreground', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          AppTheme.light(
            topSectionGradient: kuzzTopSectionGradient(
              kuzzThemeName,
              Brightness.light,
            ),
          ),
        ),
      );

      final context = tester.element(find.text('Home'));
      expect(
        navigationSectionForeground(context),
        Colors.black.withValues(alpha: 0.8),
      );
    });

    testWidgets('navigation roles inherit non-Kuzz theme tokens', (
      tester,
    ) async {
      const primaryForeground = Color(0xFF123456);
      const secondaryForeground = Color(0xFF789ABC);
      const searchSurface = Color(0xFFDEF012);
      final theme = ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.purple).copyWith(
          onSurface: primaryForeground,
          onSurfaceVariant: secondaryForeground,
          surfaceContainerHighest: searchSurface,
        ),
      );

      await tester.pumpWidget(
        MaterialApp(
          theme: theme,
          home: const Scaffold(body: SizedBox()),
        ),
      );

      final context = tester.element(find.byType(SizedBox));
      expect(navigationPrimaryForeground(context), primaryForeground);
      expect(navigationSecondaryForeground(context), secondaryForeground);
      expect(navigationSectionForeground(context), secondaryForeground);
      expect(navigationSearchSurface(context), searchSurface);
      expect(
        navigationDivider(context, 0.15),
        primaryForeground.withValues(alpha: 0.15),
      );
    });
  });

  group('isKuzzTheme', () {
    test('matches only the Kuzz pair', () {
      expect(isKuzzTheme(kuzzThemeName), isTrue);
      expect(isKuzzTheme(kuzzDarkThemeName), isTrue);
      expect(isKuzzTheme('github-light'), isFalse);
      expect(isKuzzTheme(''), isFalse);
    });
  });
}
