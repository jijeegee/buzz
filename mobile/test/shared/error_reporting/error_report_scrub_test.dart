import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/error_reporting/error_report_scrub.dart';
import 'package:sentry_flutter/sentry_flutter.dart';

void main() {
  group('scrubReportText', () {
    void expectGone(String scrubbed, List<String> secrets) {
      for (final secret in secrets) {
        expect(scrubbed, isNot(contains(secret)), reason: scrubbed);
      }
    }

    test('removes quoted text, which can be message text', () {
      final scrubbed = scrubReportText(
        'FormatException: Unexpected character "hello my secret plan"',
      );
      expectGone(scrubbed, ['secret plan']);
      expect(scrubbed, startsWith('FormatException: Unexpected character'));
    });

    test('removes keys, tokens and emails', () {
      final scrubbed = scrubReportText(
        [
          'nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq',
          '8cf9a7e7deebdc7d1fe172b9af4c6796b224e43e8f03e80dbd357f6c61fdeff9',
          'someone@example.com',
          'token=abc.def.ghi',
          'eyJhbGciOiJIUzI1NiJ9eyJzdWIiOiIxMjM0NTY3ODkwIn0abcdefgh',
        ].join(' '),
      );
      expectGone(scrubbed, [
        'nsec1',
        '8cf9a7e7',
        'someone@example.com',
        'abc.def.ghi',
        'eyJhbGci',
      ]);
    });

    test('removes IPs, user folders and auth headers', () {
      final scrubbed = scrubReportText(
        [
          'connect 192.168.0.12:3000 refused; 2001:db8::1 down;',
          'open /Users/Jiho Kim/Library/x failed;',
          'Authorization: Nostr eyJhbGciOi;',
          'blob abc+def/ghiJKLMNOPQRSTUVWXYZ==; said “my message”',
        ].join(' '),
      );
      expectGone(scrubbed, [
        '192.168',
        '2001:db8',
        'Jiho',
        'eyJhbGciOi',
        'ghiJKLMNOP',
        'my message',
      ]);
      expect(scrubbed, contains('/Users/[user]/Library'));
      expect(
        scrubReportText(r'open C:\Users\Jiho Kim\AppData\x failed'),
        r'open C:\Users\[user]\AppData\x failed',
      );
    });

    test('removes open and escaped quotes', () {
      final escaped = scrubReportText(r'bad "a \"b secret\" c" and 「비밀」 end');
      expectGone(escaped, ['b secret', '비밀']);
      expect(escaped, endsWith(' end'));
      final open = scrubReportText(
        'Unexpected end of input {"content":"hello my secret',
      );
      expectGone(open, ['hello my secret']);
    });

    test('drops URL queries and keeps hosts', () {
      expect(
        scrubReportText(
          'GET https://gateway.kuzz.app/api/x?auth=zzz#frag failed',
        ),
        'GET https://gateway.kuzz.app/api/x failed',
      );
    });

    test('keeps code locations and versions readable', () {
      const text =
          'Null check at package:kuzz/app.dart:12:34 in RelayClient.add 0.6.24';
      expect(scrubReportText(text), text);
    });

    test('caps length and stays fast on huge text', () {
      final watch = Stopwatch()..start();
      final scrubbed = scrubReportText('a.' * 100000);
      expect(watch.elapsedMilliseconds, lessThan(500));
      expect(scrubbed.length, lessThanOrEqualTo(maxReportTextChars + 1));
    });
  });

  group('scrubReportEvent', () {
    test('drops identity and context and scrubs exception text', () {
      final event = SentryEvent(
        user: SentryUser(email: 'someone@example.com'),
        request: SentryRequest(url: 'https://x.test/?a=1'),
        serverName: 'JIHO-PHONE',
        breadcrumbs: [Breadcrumb(message: 'print: hello')],
        // ignore: deprecated_member_use
        extra: {'body': 'hello'},
        tags: {'pubkey': 'abc'},
        message: SentryMessage('send failed for "my message"'),
        exceptions: [
          SentryException(type: 'StateError', value: 'bad `my message`'),
        ],
        contexts: Contexts(device: SentryDevice(name: "Jiho's iPhone"))
          ..['flutter_error_details'] = {
            'information': 'Image provider: https://x.test/media/a.jpg?t=1',
          },
        debugMeta: DebugMeta(
          images: [
            DebugImage(
              type: 'elf',
              codeFile: '/data/app/~~x/app.kuzz.mobile-1/lib/arm64/libapp.so',
            ),
          ],
        ),
      );

      final scrubbed = scrubReportEvent(event);
      expect(scrubbed.user, isNull);
      expect(scrubbed.request, isNull);
      expect(scrubbed.serverName, isNull);
      expect(scrubbed.breadcrumbs, isNull);
      // ignore: deprecated_member_use
      expect(scrubbed.extra, isNull);
      expect(scrubbed.tags, isNull);
      expect(scrubbed.contexts.device?.name, isNull);
      expect(scrubbed.contexts['flutter_error_details'], isNull);
      expect(scrubbed.message?.formatted, 'send failed for [quoted]');
      expect(scrubbed.exceptions!.single.value, 'bad [quoted]');
      expect(scrubbed.debugMeta!.images.single.codeFile, 'libapp.so');
    });
  });

  test('createReportGate admits the same error once and caps a run', () {
    SentryEvent error(String value) => SentryEvent(
      exceptions: [SentryException(type: 'Error', value: value)],
    );
    final admit = createReportGate();
    expect(admit(error('a')), isTrue);
    expect(admit(error('a')), isFalse);
    for (var n = 1; n < maxReportsPerRun; n++) {
      expect(admit(error('$n')), isTrue);
    }
    expect(admit(error('one too many')), isFalse);
  });
}
