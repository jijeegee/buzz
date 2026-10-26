import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/chats/chat_state_writer.dart';
import 'package:kuzz/shared/read_state/read_state_format.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _channel = '11111111-1111-4111-8111-111111111111';
const _other = '22222222-2222-4222-8222-222222222222';
final _root = 'c' * 64;

void main() {
  test('only channels and threads become mark_read items', () {
    expect(markReadItemForContext(_channel, 10)?.toJson(), {
      'channel': _channel,
      'at': 10,
    });
    expect(markReadItemForContext('$threadContextPrefix$_root', 10)?.toJson(), {
      'thread': _root,
      'at': 10,
    });
    expect(markReadItemForContext('msg:abc', 10), isNull);
    expect(markReadItemForContext('${threadContextPrefix}short', 10), isNull);
    expect(markReadItemForContext(_channel, 0), isNull);
  });

  test('reads batch into one request, newest position per context', () async {
    final sent = <Map<String, dynamic>>[];
    final writer = ChatStateWriter(
      send: (filter) async {
        sent.add(filter);
        return null;
      },
    );
    writer.markRead(_channel, 10);
    writer.markRead(_channel, 12);
    writer.markRead(_channel, 11);
    writer.markRead('$threadContextPrefix$_root', 5);
    await writer.flush();

    expect(sent, hasLength(1));
    expect(sent.single['mark_read'], 1);
    expect(sent.single['unread_mark'], false);
    expect(sent.single['items'], [
      {'channel': _channel, 'at': 12},
      {'thread': _root, 'at': 5},
    ]);
    expect(writer.isOwnWriteEcho(), isTrue);

    // Already sent: nothing new to say.
    writer.markRead(_channel, 12);
    await writer.flush();
    expect(sent, hasLength(1));
  });

  test('a resend goes out again at a position already sent', () async {
    final sent = <Object?>[];
    final writer = ChatStateWriter(
      send: (filter) async {
        sent.add(filter['items']);
        return null;
      },
    );
    writer.markRead(_channel, 12);
    await writer.flush();
    // A message dated 11 arrived after: the relay must turn 12 into a
    // position again for it to be read.
    writer.markRead(_channel, 12, resend: true);
    await writer.flush();
    expect(sent, [
      [
        {'channel': _channel, 'at': 12},
      ],
      [
        {'channel': _channel, 'at': 12},
      ],
    ]);
  });

  test('a refused item is dropped and never resent', () async {
    final sent = <List<Object?>>[];
    final writer = ChatStateWriter(
      send: (filter) async {
        final items = filter['items'] as List<Object?>;
        sent.add(items);
        if (items.length == 2) {
          throw RelayException(403, 'mark_read item 0: not a member');
        }
        return null;
      },
    );
    writer.markRead(_channel, 10);
    writer.markRead(_other, 10);
    await writer.flush();

    expect(sent, hasLength(2));
    expect(sent.last, [
      {'channel': _other, 'at': 10},
    ]);

    writer.markRead(_channel, 20);
    await writer.flush();
    expect(sent, hasLength(2));
  });

  test('items left after two refusals go in the next batch', () async {
    const third = '33333333-3333-4333-8333-333333333333';
    final sent = <List<Object?>>[];
    final writer = ChatStateWriter(
      send: (filter) async {
        final items = filter['items'] as List<Object?>;
        sent.add(items);
        if (items.length > 1) {
          throw RelayException(403, 'mark_read item 0: not a member');
        }
        return null;
      },
    );
    writer.markRead(_channel, 10);
    writer.markRead(_other, 10);
    writer.markRead(third, 10);
    await writer.flush();
    expect(sent, hasLength(2));

    await writer.flush();
    expect(sent.last, [
      {'channel': third, 'at': 10},
    ]);
    writer.stop();
  });

  test('chat_state_disabled stops the writer', () async {
    var calls = 0;
    final writer = ChatStateWriter(
      send: (filter) async {
        calls++;
        throw RelayException(404, '{"error":"chat_state_disabled"}');
      },
    );
    writer.markRead(_channel, 10);
    await writer.flush();
    expect(writer.stopped, isTrue);

    writer.markRead(_channel, 20);
    await writer.flush();
    expect(calls, 1);
  });

  test('a transient failure keeps the read queued for a retry', () async {
    var fail = true;
    final sent = <Map<String, dynamic>>[];
    final writer = ChatStateWriter(
      send: (filter) async {
        if (fail) throw RelayException(503, 'busy');
        sent.add(filter);
        return null;
      },
    );
    writer.markRead(_channel, 10);
    await writer.flush();
    expect(sent, isEmpty);

    fail = false;
    await writer.flush();
    expect(sent.single['items'], [
      {'channel': _channel, 'at': 10},
    ]);
    writer.stop();
  });
}
