/**
 * Kuzz users sign in with Google and never manage keys or pick a relay, so a
 * raw backend error such as `relay unreachable: request timed out` means
 * nothing to them.
 *
 * This maps ONLY known raw backend error strings (exact shapes emitted by
 * `desktop/src-tauri/src/relay.rs`, `relay/submit.rs` and the snapshot
 * importers). Apply it to the interpolated backend *detail* at the call site,
 * never to a whole composed sentence: the caller's own wording (for example
 * "X was created, but…") and user content such as agent or channel names must
 * reach the user unchanged. Anything not recognised is returned as-is.
 */

export const CONNECTIVITY_ERROR_MESSAGE =
  "Couldn't connect. Check your internet connection and try again.";
export const RATE_LIMITED_ERROR_MESSAGE =
  "Too many requests. Wait a moment and try again.";
export const SERVER_REJECTED_ERROR_MESSAGE =
  "The server couldn't complete the request. Try again.";

// `relay.rs::classify_request_error` / `classify_intercepted_response` /
// `classify_body_timeout`. Backend callers sometimes wrap it
// (`"<context>: relay unreachable: ..."`), so accept it after a `: ` too.
const RELAY_UNREACHABLE = /(?:^|: )relay unreachable:/;
// `relay.rs::relay_error_message` for HTTP 429.
const RELAY_RATE_LIMITED = /^relay rate-limited:/;
// `relay.rs::relay_error_message` / `admin/discovery.rs`:
// `relay returned <status>` optionally followed by `: <server message>`.
const RELAY_RETURNED = /^relay returned \d{3}\b/;
// `relay.rs` / `relay/submit.rs`: `relay rejected event: <reason>`;
// snapshot importers: `relay rejected engram: <reason>`.
const RELAY_REJECTED = /^relay rejected (?:event|engram):/;

/**
 * Map a raw backend error detail to plain wording when it is one of the
 * known backend shapes; otherwise return it unchanged.
 */
export function toUserFacingErrorDetail(detail: string): string {
  const trimmed = detail.trim();
  if (RELAY_UNREACHABLE.test(trimmed)) {
    return CONNECTIVITY_ERROR_MESSAGE;
  }
  if (RELAY_RATE_LIMITED.test(trimmed)) {
    return RATE_LIMITED_ERROR_MESSAGE;
  }
  // The server's own reason can itself name protocol internals (keys,
  // event kinds), so it is not carried over.
  if (RELAY_RETURNED.test(trimmed) || RELAY_REJECTED.test(trimmed)) {
    return SERVER_REJECTED_ERROR_MESSAGE;
  }
  return detail;
}

const BACKEND_ERROR_SHAPES: ReadonlyArray<readonly [RegExp, string]> = [
  [/(^|: )relay unreachable:/, CONNECTIVITY_ERROR_MESSAGE],
  [/(^|: )relay rate-limited:/, RATE_LIMITED_ERROR_MESSAGE],
  [/(^|: )relay returned \d{3}\b/, SERVER_REJECTED_ERROR_MESSAGE],
  [/(^|: )relay rejected (?:event|engram):/, SERVER_REJECTED_ERROR_MESSAGE],
];

/**
 * For whole toast texts: when a known raw backend error appears at the start
 * or as the detail after a caller's `"<context>: "` prefix, replace that
 * backend part (to the end of the text) and keep the caller's prefix.
 * Text with no known backend shape is returned unchanged.
 */
export function mapBackendErrorInText(text: string): string {
  let cut = -1;
  let replacement = "";
  for (const [shape, message] of BACKEND_ERROR_SHAPES) {
    const match = shape.exec(text);
    if (!match) continue;
    const start = match.index + match[1].length;
    if (cut === -1 || start < cut) {
      cut = start;
      replacement = message;
    }
  }
  return cut === -1 ? text : text.slice(0, cut) + replacement;
}
