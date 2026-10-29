import 'dart:async';

import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_state/read_state_provider.dart';

/// How long a room or thread waits, out of sight, for the read state before
/// it opens without it (at the newest row).
const readStateOpenWait = Duration(seconds: 1);

/// The account whose wait for the read state already ran out this session: a
/// read state stuck loading costs that account one wait, not one per room.
final readStateWaitSpentProvider =
    NotifierProvider<ReadStateWaitSpent, String?>(ReadStateWaitSpent.new);

class ReadStateWaitSpent extends Notifier<String?> {
  @override
  String? build() => null;

  void spend(String? pubkey) => state = pubkey ?? '';
}

/// Whether a page can decide where it opens: at once when it does not
/// [waits] for the read state (a link decides), once the read state
/// [isReady], or after [readStateOpenWait]. Once true it stays true, so the
/// position is decided once per visit.
bool useReadStateDecided({required bool waits, required bool isReady}) {
  final container = ProviderScope.containerOf(useContext(), listen: false);
  final pubkey = container.read(readStateProvider).pubkey;
  final spentFor = container.read(readStateWaitSpentProvider);
  final spent = spentFor != null && spentFor == (pubkey ?? '');
  final expired = useState(false);
  final decided = useRef(false);
  final waiting = waits && !isReady && !spent;
  useEffect(() {
    if (!waiting) return null;
    final timer = Timer(readStateOpenWait, () {
      container.read(readStateWaitSpentProvider.notifier).spend(pubkey);
      expired.value = true;
    });
    return timer.cancel;
  }, [waiting]);
  if (!waiting || expired.value) decided.value = true;
  return decided.value;
}
