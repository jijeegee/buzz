import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/widgets/dot_morph.dart';

void main() {
  test('every huddle step names a shape the scene has', () {
    for (final (key, _, _) in DotMorphLoader.huddleSteps) {
      expect(DotScene.huddle.shapes, contains(key));
    }
  });

  test('shape points stay inside their scene', () {
    for (final scene in [DotScene.refresh, DotScene.huddle]) {
      for (final MapEntry(:key, value: points) in scene.shapes.entries) {
        expect(points, hasLength(scene.count * 2));
        if (key == dustShape) continue;
        for (var i = 0; i < points.length; i += 2) {
          expect(points[i], inInclusiveRange(0, scene.width));
          expect(points[i + 1], inInclusiveRange(0, scene.height));
        }
      }
    }
  });

  test('a morph lands exactly on the target shape', () {
    final morph = DotMorph(DotScene.refresh, 'k');
    addTearDown(morph.dispose);
    morph.go('p', const Duration(seconds: 1));
    morph.tick(const Duration(milliseconds: 500));
    expect(morph.settled, isFalse);
    morph.tick(const Duration(milliseconds: 500));
    expect(morph.settled, isTrue);
    expect(morph.positions, orderedEquals(DotScene.refresh.shape('p')));
  });

  test('a pull blends dust into the K', () {
    final morph = DotMorph(DotScene.refresh, dustShape);
    addTearDown(morph.dispose);
    morph.blend(dustShape, 'k', 0);
    expect(morph.positions, orderedEquals(DotScene.refresh.shape(dustShape)));
    morph.blend(dustShape, 'k', 1);
    expect(morph.positions, orderedEquals(DotScene.refresh.shape('k')));
  });
}
