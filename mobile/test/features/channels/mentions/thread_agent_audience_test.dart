import 'package:buzz/features/channels/mentions/thread_agent_audience.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  final owner = 'a' * 64;
  final agent = 'b' * 64;
  final other = 'c' * 64;

  late ProviderContainer container;
  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    container = ProviderContainer(
      overrides: [savedPrefsProvider.overrideWithValue(prefs)],
    );
    addTearDown(container.dispose);
  });

  ThreadAgentAudience audience() =>
      container.read(threadAgentAudienceProvider.notifier);
  List<String> pubkeys(String scope) =>
      container.read(threadAgentAudienceProvider).pubkeysFor(scope);

  test('scope exists only for thread composers with a valid owner', () {
    expect(
      threadAgentAudienceScope(
        ownerPubkey: owner.toUpperCase(),
        channelId: 'channel',
        threadHeadId: 'head',
      ),
      '$owner:channel:thread:head',
    );
    expect(
      threadAgentAudienceScope(
        ownerPubkey: owner,
        channelId: 'channel',
        threadHeadId: null,
      ),
      isNull,
    );
    expect(
      threadAgentAudienceScope(
        ownerPubkey: owner,
        channelId: 'channel',
        threadHeadId: null,
        channelMain: true,
      ),
      '$owner:channel:channel',
    );
    expect(
      threadAgentAudienceScope(
        ownerPubkey: 'nope',
        channelId: 'channel',
        threadHeadId: 'head',
      ),
      isNull,
    );
  });

  test('preference defaults off like desktop and persists', () async {
    expect(container.read(keepMentionedAgentsPinnedProvider), isFalse);
    await container
        .read(keepMentionedAgentsPinnedProvider.notifier)
        .setEnabled(true);
    expect(container.read(keepMentionedAgentsPinnedProvider), isTrue);
    final prefs = container.read(savedPrefsProvider);
    expect(prefs.getBool(KeepMentionedAgentsPinnedPreference.key), isTrue);
  });

  test('excluded agents are not re-added by the thread root', () {
    const scope = 'scope';
    audience().initialize(scope, [agent.toUpperCase(), 'invalid']);
    expect(pubkeys(scope), [agent]);
    audience().exclude(scope, agent);
    audience().initialize(scope, [agent]);
    expect(pubkeys(scope), isEmpty);
    expect(
      audience().promote(scope, [agent], reinstateExcluded: false),
      isEmpty,
    );
    expect(audience().promote(scope, [agent, other], reinstateExcluded: true), [
      agent,
      other,
    ]);
    expect(pubkeys(scope), [agent, other]);
  });

  test('turning the preference off forgets every thread audience', () async {
    final preference = container.read(
      keepMentionedAgentsPinnedProvider.notifier,
    );
    await preference.setEnabled(true);
    audience().initialize('one', [agent]);
    audience().exclude('two', other);
    await preference.setEnabled(false);
    final state = container.read(threadAgentAudienceProvider);
    expect(state.audiences, isEmpty);
    expect(state.excluded, isEmpty);
  });

  test('keeps at most the most recent scopes', () {
    for (var i = 0; i <= ThreadAgentAudience.maxScopes; i++) {
      audience().initialize('scope-$i', [agent]);
    }
    final state = container.read(threadAgentAudienceProvider);
    expect(state.audiences.length, ThreadAgentAudience.maxScopes);
    expect(state.audiences.containsKey('scope-0'), isFalse);
  });
}
