import assert from "node:assert/strict";
import test from "node:test";

// Group photo (Groups S4): the letter avatars show the photo when set and
// fall back to the letter when it is missing or fails to load; the edit
// dialog's photo field sends a new URL on upload and "" on Remove photo.

const uploads = [];
const tauriMock = {
  invoke(command, args) {
    if (command === "upload_media_bytes") {
      uploads.push(args);
      return Promise.resolve({
        url: "https://relay.example/media/aa.png",
        type: "image/png",
        size: 4,
        sha256: "aa",
      });
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { ConversationLetterAvatar, GroupLetterAvatar } = await import(
  "../../home/ui/ConversationLetterAvatar.tsx"
);
const { GroupPhotoField } = await import("./GroupPhotoField.tsx");

const CHANNEL_ID = "11111111-1111-4111-8111-111111111111";
const PHOTO = "https://example.com/group.png";

async function render(element) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(element));
  return {
    container,
    rerender: (next) => act(async () => root.render(next)),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("GroupLetterAvatar shows only the letter without a photo", async () => {
  const view = await render(
    React.createElement(GroupLetterAvatar, {
      channelId: CHANNEL_ID,
      label: "design",
      testId: "avatar",
    }),
  );
  const avatar = view.container.querySelector("[data-testid='avatar']");
  assert.equal(avatar.textContent, "D");
  assert.equal(view.container.querySelector("img"), null);
  await view.unmount();
});

test("GroupLetterAvatar shows the photo and keeps the lock badge", async () => {
  const view = await render(
    React.createElement(GroupLetterAvatar, {
      channelId: CHANNEL_ID,
      isPrivate: true,
      label: "design",
      picture: PHOTO,
      testId: "avatar",
    }),
  );
  const img = view.container.querySelector("[data-testid='avatar-image']");
  assert.ok(img, "photo is rendered");
  assert.equal(img.getAttribute("src"), PHOTO);
  assert.match(img.className, /rounded-full/);
  assert.ok(view.container.querySelector("[data-group-avatar-lock]"));
  await view.unmount();
});

test("GroupLetterAvatar falls back to the letter when the photo fails", async () => {
  const view = await render(
    React.createElement(GroupLetterAvatar, {
      channelId: CHANNEL_ID,
      label: "design",
      picture: PHOTO,
      testId: "avatar",
    }),
  );
  const img = view.container.querySelector("[data-testid='avatar-image']");
  await act(async () => fireEvent.error(img));
  assert.equal(
    view.container.querySelector("[data-testid='avatar-image']"),
    null,
  );
  assert.equal(
    view.container.querySelector("[data-testid='avatar']").textContent,
    "D",
  );
  // A new photo URL gets a fresh try.
  await view.rerender(
    React.createElement(GroupLetterAvatar, {
      channelId: CHANNEL_ID,
      label: "design",
      picture: "https://example.com/other.png",
      testId: "avatar",
    }),
  );
  assert.ok(view.container.querySelector("[data-testid='avatar-image']"));
  await view.unmount();
});

test("ConversationLetterAvatar (Chats rows) shows the group photo", async () => {
  const view = await render(
    React.createElement(ConversationLetterAvatar, {
      channelId: CHANNEL_ID,
      channelLabel: "design",
      isPrivate: true,
      isThread: false,
      picture: PHOTO,
      testId: "row",
    }),
  );
  assert.equal(
    view.container
      .querySelector("[data-testid='row-image']")
      ?.getAttribute("src"),
    PHOTO,
  );
  assert.ok(view.container.querySelector("[data-testid='row-lock']"));
  await view.unmount();
});

test("GroupPhotoField: Remove photo sends an empty value", async () => {
  const changes = [];
  const view = await render(
    React.createElement(GroupPhotoField, {
      channelId: CHANNEL_ID,
      isPrivate: false,
      label: "design",
      onPictureChange: (value) => changes.push(value),
      picture: PHOTO,
    }),
  );
  const change = view.container.querySelector(
    "[data-testid='group-photo-change']",
  );
  assert.equal(change.textContent, "Change photo");
  await act(async () =>
    fireEvent.click(
      view.container.querySelector("[data-testid='group-photo-remove']"),
    ),
  );
  assert.deepEqual(changes, [""]);
  await view.unmount();
});

test("GroupPhotoField: no Remove photo without a photo", async () => {
  const view = await render(
    React.createElement(GroupPhotoField, {
      channelId: CHANNEL_ID,
      isPrivate: false,
      label: "design",
      onPictureChange: () => {},
      picture: "",
    }),
  );
  assert.equal(
    view.container.querySelector("[data-testid='group-photo-remove']"),
    null,
  );
  assert.ok(view.container.querySelector("[data-testid='group-photo-change']"));
  await view.unmount();
});

test("GroupPhotoField: an uploaded image becomes the new photo", async () => {
  uploads.length = 0;
  const changes = [];
  const view = await render(
    React.createElement(GroupPhotoField, {
      channelId: CHANNEL_ID,
      isPrivate: false,
      label: "design",
      onPictureChange: (value) => changes.push(value),
      picture: "",
    }),
  );
  const input = view.container.querySelector(
    "[data-testid='group-photo-input']",
  );
  const file = new File([new Uint8Array([1, 2, 3, 4])], "g.png", {
    type: "image/png",
  });
  file.arrayBuffer ??= async () => new Uint8Array([1, 2, 3, 4]).buffer;
  Object.defineProperty(input, "files", { value: [file], configurable: true });
  await act(async () => fireEvent.change(input));
  for (let i = 0; i < 40 && changes.length === 0; i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
  assert.equal(uploads.length, 1);
  assert.deepEqual(changes, ["https://relay.example/media/aa.png"]);
  await view.unmount();
});

test("GroupPhotoField: a non-image file is refused", async () => {
  uploads.length = 0;
  const changes = [];
  const view = await render(
    React.createElement(GroupPhotoField, {
      channelId: CHANNEL_ID,
      isPrivate: false,
      label: "design",
      onPictureChange: (value) => changes.push(value),
      picture: "",
    }),
  );
  const input = view.container.querySelector(
    "[data-testid='group-photo-input']",
  );
  const file = new File(["hi"], "a.txt", { type: "text/plain" });
  Object.defineProperty(input, "files", { value: [file], configurable: true });
  await act(async () => fireEvent.change(input));
  assert.equal(uploads.length, 0);
  assert.deepEqual(changes, []);
  assert.match(
    view.container.querySelector("[data-testid='group-photo-error']")
      .textContent,
    /PNG, JPG, GIF, or WebP/,
  );
  await view.unmount();
});
