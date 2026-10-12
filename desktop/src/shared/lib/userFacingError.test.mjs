import assert from "node:assert/strict";
import test from "node:test";

import {
  CONNECTIVITY_ERROR_MESSAGE,
  GENERIC_ERROR_MESSAGE,
  toUserFacingMessage,
} from "./userFacingError.ts";

test("toUserFacingMessage leaves plain text unchanged", () => {
  assert.equal(
    toUserFacingMessage("Couldn't save the recording."),
    "Couldn't save the recording.",
  );
  assert.equal(
    toUserFacingMessage("Network request timed out"),
    "Network request timed out",
  );
  assert.equal(toUserFacingMessage(""), "");
});

test("toUserFacingMessage maps connectivity errors with developer terms", () => {
  for (const message of [
    "relay unreachable: error sending request",
    "Archive failed: relay unreachable: connection refused",
    "Nostr websocket closed",
    "Failed to reach wss://relay.example.com",
    "Request to the Relay timed out",
    "relays offline",
  ]) {
    assert.equal(toUserFacingMessage(message), CONNECTIVITY_ERROR_MESSAGE);
  }
});

test("toUserFacingMessage maps other developer-term errors to a generic message", () => {
  for (const message of [
    "relay rejected event: blocked",
    "invalid npub",
    "npub1qqqsyqcyq5rqwzqf is not a member",
    "nsec1abc could not be decoded",
    "NIP-05 lookup failed",
    "nip-98 auth missing",
    "Invalid pubkey",
    "Public key does not match",
    "private key is malformed",
  ]) {
    assert.equal(toUserFacingMessage(message), GENERIC_ERROR_MESSAGE, message);
  }
});

test("toUserFacingMessage matches whole words only", () => {
  assert.equal(toUserFacingMessage("Relaying is fine"), "Relaying is fine");
  assert.equal(toUserFacingMessage("NIP-mapping"), "NIP-mapping");
});
