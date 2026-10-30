/**
 * The newest-window requests of rooms being opened (the window a room opens
 * on), so launch work can let the room on screen go first
 * (docs/message-sync.md#launch-order): the first preload wave waits while
 * one is out, and agent indicator replay starts after the first settles.
 */
let inFlight = 0;
let firstSettled = false;
let waiters: (() => void)[] = [];
/** Bumped by a community switch; settles of older requests are ignored. */
let generation = 0;

/** Count `request` as an open window until it settles. */
export function trackOpenWindow<T>(request: Promise<T>): Promise<T> {
  const requestGeneration = generation;
  inFlight += 1;
  const settle = () => {
    if (requestGeneration !== generation) return;
    inFlight = Math.max(0, inFlight - 1);
    if (firstSettled) return;
    firstSettled = true;
    const settled = waiters;
    waiters = [];
    for (const resolve of settled) resolve();
  };
  return request.finally(settle);
}

/** Open-window requests still out. */
export function openWindowsInFlight(): number {
  return inFlight;
}

/** Resolves once a room's open window has been answered (or failed). */
export function firstOpenWindowSettled(): Promise<void> {
  if (firstSettled) return Promise.resolve();
  return new Promise((resolve) => waiters.push(resolve));
}

/**
 * A community switch starts over: requests of the old one no longer count,
 * and waiters of the old one are released.
 */
export function resetOpenWindows(): void {
  generation += 1;
  inFlight = 0;
  firstSettled = false;
  const settled = waiters;
  waiters = [];
  for (const resolve of settled) resolve();
}
