import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/deeplink/deep_link.dart';

void main() {
  const code = 'AbCdEfGhIjKlMnOpQrStUv_-12';

  group('parseGroupLinkInput', () {
    test('reads web links with a code or an address', () {
      expect(
        parseGroupLinkInput('https://kuzz.example.com/g/$code'),
        const GroupInviteCodeTarget(code),
      );
      expect(
        parseGroupLinkInput('https://kuzz.example.com:8443/g/@Book_Club'),
        const GroupHandleTarget('book_club'),
      );
      expect(
        parseGroupLinkInput('http://localhost:3000/g/$code/'),
        const GroupInviteCodeTarget(code),
      );
    });

    test('reads a web link copied without its scheme', () {
      expect(
        parseGroupLinkInput('kuzz.example.com/g/@book_club'),
        const GroupHandleTarget('book_club'),
      );
      expect(
        parseGroupLinkInput('  kuzz.example.com/g/$code  '),
        const GroupInviteCodeTarget(code),
      );
    });

    test('reads app links', () {
      expect(
        parseGroupLinkInput('kuzz://join/$code'),
        const GroupInviteCodeTarget(code),
      );
      expect(
        parseGroupLinkInput('kuzz://join/@book_club'),
        const GroupHandleTarget('book_club'),
      );
      expect(
        parseGroupLinkInput('kuzz-demo-preview-1://join/@abc'),
        const GroupHandleTarget('abc'),
      );
    });

    test('reads a bare code or @address', () {
      expect(parseGroupLinkInput(code), const GroupInviteCodeTarget(code));
      expect(
        parseGroupLinkInput('@Book_Club'),
        const GroupHandleTarget('book_club'),
      );
    });

    test('rejects text that is not a group link', () {
      for (final input in [
        '',
        '   ',
        'hello',
        '@ab',
        '@${'a' * 33}',
        '@book-club',
        'short_code',
        'https://kuzz.example.com/invite/$code',
        'https://kuzz.example.com/g/$code/extra',
        'https://kuzz.example.com/g/',
        'https://user@kuzz.example.com/g/$code',
        'https://kuzz.example.com/g/$code#x',
        'kuzz://join?relay=wss%3A%2F%2Frelay.example.com&code=abc123',
        'kuzz://join/$code?x=1',
        'kuzz://join/a/b',
        'kuzz://channel/$code',
        'other://join/$code',
        'ftp://kuzz.example.com/g/$code',
        'path/g/$code/more/than',
        'a/b/g/$code',
      ]) {
        expect(parseGroupLinkInput(input), isNull, reason: input);
      }
    });
  });

  group('parseKuzzDeepLink', () {
    test('returns group join links', () {
      expect(
        parseKuzzDeepLink(Uri.parse('kuzz://join/$code')),
        const GroupJoinDeepLink(GroupInviteCodeTarget(code)),
      );
      expect(
        parseKuzzDeepLink(Uri.parse('kuzz://join/@book_club')),
        const GroupJoinDeepLink(GroupHandleTarget('book_club')),
      );
    });

    test('still reads the community invite form', () {
      expect(
        parseKuzzDeepLink(
          Uri.parse(
            'kuzz://join?relay=wss%3A%2F%2Frelay.example.com&code=abc123',
          ),
        ),
        isA<InviteDeepLink>().having((link) => link.code, 'code', 'abc123'),
      );
    });

    test('ignores a malformed group join link', () {
      expect(
        parseKuzzDeepLink(Uri.parse('kuzz://join/@x')),
        isNot(isA<GroupJoinDeepLink>()),
      );
    });
  });
}
