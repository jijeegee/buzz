import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/groups/groups_api.dart';

const _url =
    'https://relay.example/media/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg';

Map<String, dynamic> _group({Object? picture = _url, bool hasKey = true}) => {
  'id': 'group-1',
  'name': 'design',
  'description': 'Design team',
  'channelType': 'stream',
  'memberCount': 3,
  'isMember': false,
  'visibility': 'open',
  if (hasKey) 'picture': picture,
};

void main() {
  group('groups API picture', () {
    test('group JSON carries the photo to the summary and channel', () {
      final summary = GroupSummary.fromJson(_group());
      expect(summary.picture, _url);
      expect(summary.toChannel().picture, _url);
    });

    test('null, empty or missing picture means no photo', () {
      expect(GroupSummary.fromJson(_group(picture: null)).picture, isNull);
      expect(GroupSummary.fromJson(_group(picture: '')).picture, isNull);
      expect(GroupSummary.fromJson(_group(hasKey: false)).picture, isNull);
      expect(GroupSummary.fromJson(_group(picture: 7)).picture, isNull);
    });

    test('an invitation channel object carries the photo', () {
      final invitation = GroupInvitation.fromJson({
        'id': 'inv-1',
        'channel': {..._group(), 'visibility': null},
        'inviter': 'ABC',
        'createdAt': '2026-10-20T00:00:00Z',
      });
      expect(invitation.group.picture, _url);
      expect(invitation.group.toChannel().picture, _url);
      expect(invitation.group.isPrivate, isTrue);
    });
  });
}
