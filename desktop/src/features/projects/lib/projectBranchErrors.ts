/**
 * Relay push-policy denial token for a repository with no `buzz-channel`
 * binding. Declared in Rust as `kuzz-core::git_perms::
 * GIT_NO_CHANNEL_BINDING_TOKEN`; the relay's denial body starts with it
 * ("no_channel_binding: repository has no channel binding"). The legacy
 * spaced phrase is kept as a second matcher so this build also recognizes
 * denials from relays deployed before the token existed.
 */
const NO_CHANNEL_BINDING_TOKEN = "no_channel_binding";
const NO_CHANNEL_BINDING_LEGACY_PHRASE = "no channel binding";

const NO_CHANNEL_BINDING_COPY =
  "This repository isn't linked to a channel yet, so no one can open it. " +
  "The repository owner can link a channel from the Access menu.";

/** True when a git/relay error text is the unbound-repository denial. */
export function isNoChannelBindingError(message: string): boolean {
  return (
    message.includes(NO_CHANNEL_BINDING_TOKEN) ||
    message.includes(NO_CHANNEL_BINDING_LEGACY_PHRASE)
  );
}

/** Map a thrown branch-operation error to user-facing dialog copy. */
export function projectBranchErrorMessage(
  error: unknown,
  fallback: string,
): string {
  if (!(error instanceof Error)) return fallback;
  if (isNoChannelBindingError(error.message)) {
    return NO_CHANNEL_BINDING_COPY;
  }
  return error.message;
}
