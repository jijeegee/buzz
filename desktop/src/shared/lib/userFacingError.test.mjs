import assert from "node:assert/strict";
import test from "node:test";

import {
  CONNECTIVITY_ERROR_MESSAGE,
  RATE_LIMITED_ERROR_MESSAGE,
  mapBackendErrorInText,
  SERVER_REJECTED_ERROR_MESSAGE,
  toUserFacingErrorDetail,
} from "./userFacingError.ts";

test("maps the backend's relay-unreachable classifications", () => {
  for (const detail of [
    "relay unreachable: request timed out",
    "relay unreachable: could not connect to relay",
    "relay unreachable: relay host not found",
    "relay unreachable: network error",
    "relay unreachable: relay returned an unexpected HTML page (VPN or proxy sign-in?)",
    "failed to publish: relay unreachable: network error",
  ]) {
    assert.equal(toUserFacingErrorDetail(detail), CONNECTIVITY_ERROR_MESSAGE);
  }
});

test("maps rate-limit, status and rejection errors", () => {
  assert.equal(
    toUserFacingErrorDetail("relay rate-limited: retry in 4s"),
    RATE_LIMITED_ERROR_MESSAGE,
  );
  assert.equal(
    toUserFacingErrorDetail("relay rate-limited: quota exceeded"),
    RATE_LIMITED_ERROR_MESSAGE,
  );
  for (const detail of [
    "relay returned 500 Internal Server Error",
    "relay returned 403 Forbidden: pubkey not a member",
    "relay rejected event: blocked: pubkey not allowed",
    "relay rejected engram: invalid: bad signature",
  ]) {
    assert.equal(
      toUserFacingErrorDetail(detail),
      SERVER_REJECTED_ERROR_MESSAGE,
      detail,
    );
  }
});

test("leaves unrecognised text unchanged", () => {
  for (const detail of [
    "",
    "Couldn't save the recording.",
    "Network request timed out",
    "Invalid pubkey",
    "runtime binary not found",
  ]) {
    assert.equal(toUserFacingErrorDetail(detail), detail);
  }
});

test("never rewrites user content that mentions relay", () => {
  for (const text of [
    "Relay",
    "Could not start Relay — your message was sent, but the agent may not respond.",
    "#relay-ops",
    "Relay was created, but it did not start: missing runtime",
  ]) {
    assert.equal(toUserFacingErrorDetail(text), text);
  }
});

test("partial-success sentences keep their full text; only the detail is mapped", () => {
  const name = "Relay Bot";
  const spawnError = "relay unreachable: request timed out";
  const toast = `${name} was created, but it did not start: ${toUserFacingErrorDetail(spawnError)}`;
  assert.equal(
    toast,
    `Relay Bot was created, but it did not start: ${CONNECTIVITY_ERROR_MESSAGE}`,
  );

  const detached = `Could not start ${name} — your message was sent, but the agent may not respond. ${toUserFacingErrorDetail("agent process exited")}`;
  assert.equal(
    detached,
    "Could not start Relay Bot — your message was sent, but the agent may not respond. agent process exited",
  );
});

test("mapBackendErrorInText keeps the caller's prefix and maps the backend part", () => {
  assert.equal(
    mapBackendErrorInText(
      "Failed to load channels: relay unreachable: timed out",
    ),
    `Failed to load channels: ${CONNECTIVITY_ERROR_MESSAGE}`,
  );
  assert.equal(
    mapBackendErrorInText("relay returned 500: pubkey mismatch"),
    SERVER_REJECTED_ERROR_MESSAGE,
  );
});

test("mapBackendErrorInText leaves user content and unknown text alone", () => {
  for (const text of [
    'Agent "Relay" was created, but the profile did not sync.',
    "Couldn't open #relay-ops",
    "Your message was sent, but the agent may not respond.",
  ]) {
    assert.equal(mapBackendErrorInText(text), text);
  }
});
