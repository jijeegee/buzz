/**
 * Paint first, confirm after (docs/message-sync.md#painting-from-the-store):
 * a room or thread opened for the first time reads its stored window and
 * asks the relay at the same time. Shared cases:
 * test-fixtures/message-store-paint-first.json.
 */

const RELAY_ENDED = Symbol("relay ended");

/**
 * The stored window to draw first, or `null` to draw the relay's answer:
 * stored rows are drawn when the store answers before the relay, or when
 * the relay fails first. A failed or empty store read is `null`.
 * `relayFailed` says the relay request already failed, so nothing can
 * follow the stored rows.
 */
export async function storeBeforeRelay<S>(
  store: Promise<S | null>,
  relay: Promise<unknown>,
): Promise<{ stored: S; relayFailed: boolean } | null> {
  const storeRead: Promise<{ value: S | null }> = store.then(
    (value) => ({ value }),
    () => ({ value: null }),
  );
  let relayFailed = false;
  const relayEnded: Promise<typeof RELAY_ENDED> = relay.then(
    () => RELAY_ENDED,
    () => {
      relayFailed = true;
      return RELAY_ENDED;
    },
  );
  const first = await Promise.race([storeRead, relayEnded]);
  if (first === RELAY_ENDED) {
    if (!relayFailed) return null;
    const { value } = await storeRead;
    return value === null ? null : { stored: value, relayFailed: true };
  }
  return first.value === null
    ? null
    : { stored: first.value, relayFailed: false };
}
