/** One tile of a chat picture, in percent of the picture's box. */
export type ChatGridTile = { left: number; top: number; size: number };

/**
 * Tile positions for `count` people: 0-1 = one full circle, 2 = a diagonal
 * pair, 3 = one on top and two below, 4+ = a 2x2 grid (first four people).
 */
export function chatGridTiles(count: number): ChatGridTile[] {
  if (count <= 1) return [{ left: 0, top: 0, size: 100 }];
  if (count === 2) {
    return [
      { left: 0, top: 0, size: 62 },
      { left: 38, top: 38, size: 62 },
    ];
  }
  if (count === 3) {
    return [
      { left: 25, top: 0, size: 50 },
      { left: 0, top: 50, size: 50 },
      { left: 50, top: 50, size: 50 },
    ];
  }
  return [
    { left: 0, top: 0, size: 50 },
    { left: 50, top: 0, size: 50 },
    { left: 0, top: 50, size: 50 },
    { left: 50, top: 50, size: 50 },
  ];
}
