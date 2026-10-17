import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { cleanup, render } from "@testing-library/react";
import { createElement } from "react";

import { ThreadMarkerLine } from "./ThreadMarkerLine.tsx";

const ME = "a".repeat(64);
const MINA = "b".repeat(64);
const ROOT = "c".repeat(64);

afterEach(() => cleanup());

test("marker replies read as system lines, own ones as You", () => {
  const { getAllByTestId } = render(
    createElement(
      "div",
      null,
      createElement(ThreadMarkerLine, {
        currentPubkey: ME,
        message: {
          author: "Me",
          id: "m1",
          pubkey: ME,
          tags: [["buzz:thread-closed", ROOT]],
        },
      }),
      createElement(ThreadMarkerLine, {
        currentPubkey: ME,
        message: {
          author: "Mina",
          id: "m2",
          pubkey: MINA,
          tags: [["buzz:thread-reopened", ROOT]],
        },
      }),
    ),
  );
  const lines = getAllByTestId("thread-marker-line").map(
    (el) => el.textContent,
  );
  assert.deepEqual(lines, [
    "You closed this thread",
    "Mina reopened this thread",
  ]);
});
