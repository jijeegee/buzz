import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/direct_thread_page.dart';
import 'package:kuzz/shared/relay/relay.dart';

NostrEvent _reply({required String parent, required String root}) => NostrEvent(
  id: 'linked',
  pubkey: 'b' * 64,
  createdAt: 1000,
  kind: EventKind.streamMessageV2,
  tags: [
    const ['h', 'room'],
    ['e', root, '', 'root'],
    ['e', parent, '', 'reply'],
  ],
  content: 'linked reply',
  sig: '',
);

void main() {
  test('a reply not known yet keeps the link id as head', () {
    expect(
      linkedThreadHead(threadId: 'p', linked: null, parentMissing: false),
      (headId: 'p', rootId: null),
    );
  });

  test('a direct reply opens its root', () {
    expect(
      linkedThreadHead(
        threadId: 'r',
        linked: _reply(parent: 'r', root: 'r'),
        parentMissing: false,
      ),
      (headId: 'r', rootId: null),
    );
  });

  test('a nested reply keeps its parent and names the root', () {
    expect(
      linkedThreadHead(
        threadId: 'p',
        linked: _reply(parent: 'p', root: 'r'),
        parentMissing: false,
      ),
      (headId: 'p', rootId: 'r'),
    );
  });

  test('a hidden parent gives way to the root', () {
    expect(
      linkedThreadHead(
        threadId: 'p',
        linked: _reply(parent: 'p', root: 'r'),
        parentMissing: true,
      ),
      (headId: 'r', rootId: null),
    );
  });

  test('a reply under another parent leaves the link alone', () {
    expect(
      linkedThreadHead(
        threadId: 'r',
        linked: _reply(parent: 'p', root: 'r'),
        parentMissing: true,
      ),
      (headId: 'r', rootId: null),
    );
  });
}
