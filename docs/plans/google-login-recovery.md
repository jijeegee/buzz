# Google login recovery

Keep the existing onboarding order and account/key recovery contract. This change
addresses browser waiting and returning to Buzz, without adding signup steps,
reordering agent setup, changing profiles/invitations, or migrating identities.

Desktop browser waiting offers Cancel sign-in. Cancellation owns a particular
attempt and closes its loopback listener before allowing another attempt. Once
the validated callback advances to account processing, cancellation no longer
interrupts key/session persistence. The UI then says it is finishing in Buzz.
Errors remain visible beside the retryable sign-in action.

A valid callback restores/shows/focuses the existing main window. The browser
page says the response was delivered, not that account recovery has finished.
Failure to foreground the window does not turn a successful login into failure;
the page provides manual return instructions. No new global app deep link is
introduced, avoiding ambiguity between installed app variants.

Verify cancellation without callback, immediate retry, duplicate clicks, stale
attempt events, completion/cancel races, navigation away, and account errors.
Preserve PKCE/state validation, legacy account selection, and existing keys and
agent work. Mobile return changes require native workflow evidence; do not
infer a root cause from the report alone.

Android's Custom Tab fallback reproduced callback delivery with the browser
still covering the original task (API 35, Chrome 124, plugin 5.1.0). After a
callback, an Android-only platform call returns to the existing MainActivity
with CLEAR_TOP and SINGLE_TOP in its own task. Window-return errors preserve
the callback. Callback schemes, account persistence, ephemeral browsing, iOS,
and cancellation semantics remain unchanged.

The isolated native probe confirmed browser-to-app return after the change and
system-browser dismissal returning cancellation. This exercises the actual
launcher/plugin and Activity stack with a local callback, not Google account
authentication. Real Google login, newer Auth Tab browsers, iOS and a human
acceptance pass remain release checks.
