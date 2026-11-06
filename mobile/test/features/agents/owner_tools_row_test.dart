import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/agents/owner_tools_row.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/theme/theme.dart';

const _origin = 'https://relay.test';
final _agent = 'a' * 64;

class _FakeToolsApi extends Fake implements AccountApi {
  _FakeToolsApi(this.level);

  AgentToolLevel? level;
  AccountApiException? refuse;
  final sent = <({String level, String respondClass})>[];

  @override
  Future<Map<String, AgentToolLevel>> toolLevels(List<String> pubkeys) async {
    final known = level;
    return known == null ? {} : {known.pubkey: known};
  }

  @override
  Future<String> setToolLevel(
    String pubkey, {
    required String level,
    required String respondClass,
    bool isDefault = false,
  }) async {
    sent.add((level: level, respondClass: respondClass));
    if (refuse case final refusal?) throw refusal;
    this.level = AgentToolLevel(
      pubkey: pubkey,
      level: level,
      respondClass: respondClass,
    );
    return 'set';
  }
}

AgentToolLevel _level(String? level, String? respondClass) =>
    AgentToolLevel(pubkey: _agent, level: level, respondClass: respondClass);

void main() {
  group('toolLevelSaveWrite', () {
    test('keeps the level; must succeed only while on', () {
      expect(
        toolLevelSaveWrite(
          _level('full', 'owner-only'),
          respondClass: 'allowlist',
        ),
        const ToolLevelWrite(level: 'full', mustSucceed: true),
      );
      expect(
        toolLevelSaveWrite(
          _level('limited', 'owner-only'),
          respondClass: 'allowlist',
        ),
        const ToolLevelWrite(level: 'limited', mustSucceed: false),
      );
    });

    test('anyone sends off, even when the level could not be read', () {
      const off = ToolLevelWrite(level: 'limited', mustSucceed: false);
      expect(
        toolLevelSaveWrite(
          _level('full', 'owner-only'),
          respondClass: 'anyone',
        ),
        off,
      );
      expect(toolLevelSaveWrite(null, respondClass: 'anyone'), off);
    });

    test('unknown or never set sends nothing', () {
      expect(toolLevelSaveWrite(null, respondClass: 'owner-only'), isNull);
      expect(
        toolLevelSaveWrite(_level(null, null), respondClass: 'owner-only'),
        isNull,
      );
    });

    test('a failed call stops the save only while on', () async {
      final api = _FakeToolsApi(null)
        ..refuse = const AccountApiException('down', statusCode: 503);
      await expectLater(
        writeToolLevelBeforeSave(
          api,
          _agent,
          current: _level('full', 'owner-only'),
          respondClass: 'owner-only',
        ),
        throwsA(isA<AccountApiException>()),
      );
      await writeToolLevelBeforeSave(
        api,
        _agent,
        current: _level('limited', 'owner-only'),
        respondClass: 'owner-only',
      );
      expect(api.sent, hasLength(2));
    });

    test('a stale class does not stop the save', () async {
      final api = _FakeToolsApi(null)
        ..refuse = const AccountApiException(
          'stale',
          statusCode: 409,
          code: 'respond_class_stale',
        );
      await writeToolLevelBeforeSave(
        api,
        _agent,
        current: _level('full', 'owner-only'),
        respondClass: 'allowlist',
      );
      expect(api.sent.single, (level: 'full', respondClass: 'allowlist'));
    });
  });

  group('OwnerToolsRow', () {
    Future<_FakeToolsApi> pump(
      WidgetTester tester,
      AgentToolLevel? level, {
      String? respondClass,
    }) async {
      final api = _FakeToolsApi(level);
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            accountOriginProvider.overrideWithValue(_origin),
            accountApiProvider.overrideWith((ref, _) => api),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: Scaffold(
              body: OwnerToolsRow(
                agentPubkey: _agent,
                respondClass: respondClass,
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      return api;
    }

    Switch toggle(WidgetTester tester) => tester.widget<Switch>(
      find.descendant(
        of: find.byKey(const Key('owner-tools-switch')),
        matching: find.byType(Switch),
      ),
    );

    testWidgets('turns owner tools on with the stored class', (tester) async {
      final api = await pump(tester, _level('limited', 'owner-only'));
      expect(toggle(tester).value, isFalse);

      await tester.tap(find.byKey(const Key('owner-tools-switch')));
      await tester.pumpAndSettle();

      expect(api.sent.single, (level: 'full', respondClass: 'owner-only'));
      expect(toggle(tester).value, isTrue);
    });

    testWidgets('is off and locked while anyone can talk to it', (
      tester,
    ) async {
      await pump(tester, _level('full', 'anyone'));
      expect(toggle(tester).value, isFalse);
      expect(toggle(tester).onChanged, isNull);
    });

    testWidgets('asks before turning on for an allowlist', (tester) async {
      final api = await pump(
        tester,
        _level('limited', 'allowlist'),
        respondClass: 'allowlist',
      );
      await tester.tap(find.byKey(const Key('owner-tools-switch')));
      await tester.pumpAndSettle();
      expect(api.sent, isEmpty);

      await tester.tap(find.byKey(const Key('devices-confirm')));
      await tester.pumpAndSettle();
      expect(api.sent.single.level, 'full');
    });

    testWidgets('a stale class says to change it on the computer', (
      tester,
    ) async {
      final api = await pump(tester, _level(null, null));
      api.refuse = const AccountApiException(
        'stale',
        statusCode: 409,
        code: 'respond_class_stale',
      );
      await tester.tap(find.byKey(const Key('owner-tools-switch')));
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('owner-tools-error')), findsOneWidget);
      expect(
        find.text('Change this on the computer that runs this agent.'),
        findsOneWidget,
      );
    });

    testWidgets('shows nothing for an agent the relay does not list', (
      tester,
    ) async {
      await pump(tester, null);
      expect(find.byKey(const Key('owner-tools-row')), findsNothing);
    });
  });
}
