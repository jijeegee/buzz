import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chat_list_provider.dart';
import 'package:kuzz/features/chats/thread_pins_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

final _a = 'a' * 64;
final _b = 'b' * 64;

void main() {
  group('parseThreadPins (desktop thread-pins record)', () {
    test('reads the pinned registers of meta', () {
      expect(
        parseThreadPins({
          'version': 1,
          'ids': [_b],
          'meta': {
            'v': 1,
            'i': {
              _a: [3, 'dev', true],
              _b: [4, 'dev', false],
            },
          },
        }),
        {_a},
      );
    });

    test('a record without meta reads as no pins (desktop decodeDoc)', () {
      expect(
        parseThreadPins({
          'version': 1,
          'ids': [_a, _b],
        }),
        isEmpty,
      );
    });

    test('ids that are not 64-hex event ids are skipped', () {
      expect(
        parseThreadPins({
          'version': 1,
          'meta': {
            'v': 1,
            'i': {
              'not-an-id': [1, 'dev', true],
              ('A' * 64): [1, 'dev', true],
              _b: [1, 'dev', true],
            },
          },
        }),
        {'a' * 64, _b},
      );
    });

    test('anything else reads as no pins', () {
      expect(parseThreadPins(null), isEmpty);
      expect(
        parseThreadPins({
          'version': 2,
          'meta': {
            'v': 1,
            'i': {
              _a: [1, 'dev', true],
            },
          },
        }),
        isEmpty,
      );
      expect(
        parseThreadPins({
          'version': 1,
          'meta': {'v': 2, 'i': {}},
        }),
        isEmpty,
      );
    });

    test('keeps at most 200', () {
      final registers = {
        for (var i = 0; i < 250; i++)
          i.toRadixString(16).padLeft(64, '0'): [1, 'dev', true],
      };
      expect(
        parseThreadPins({
          'version': 1,
          'meta': {'v': 1, 'i': registers},
        }),
        hasLength(200),
      );
    });
  });

  group('chatListRetry', () {
    test('a timeout is tried once more', () {
      expect(chatListRetry(0, TimeoutException('slow')), isNotNull);
      expect(
        chatListRetry(
          0,
          RelayException(503, '{"error":"error: query timed out"}'),
        ),
        isNotNull,
      );
      expect(chatListRetry(1, TimeoutException('slow')), isNull);
    });

    test('no retry when the relay keeps no chat state', () {
      expect(chatListRetry(0, const ChatListUnsupportedException()), isNull);
    });

    test('a chat_list request waits 30 seconds as desktop', () {
      expect(chatListQueryTimeout, const Duration(seconds: 30));
    });
  });
}
