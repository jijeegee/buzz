import assert from "node:assert/strict";
import test from "node:test";

// Group-photo and profile avatars load the relay's downsized JPEG variants
// (`.small.jpg` up to 56 CSS px, `.thumb.jpg` above) instead of the
// multi-megabyte original, fall back small -> thumb -> original as loads
// fail, and the Manage group sheet opens the original full-size when the
// large photo is clicked.

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { GroupLetterAvatar } = await import(
  "../../home/ui/ConversationLetterAvatar.tsx"
);
const { GroupPhotoField } = await import("./GroupPhotoField.tsx");
const { ProfileAvatar } = await import("../../profile/ui/ProfileAvatar.tsx");
const { UserAvatar } = await import("../../../shared/ui/UserAvatar.tsx");

const HASH = "c".repeat(64);
const ORIGINAL = `https://kuzz.example/media/${HASH}.png`;
const PROXIED_SMALL = `kuzz-media://localhost/media/${HASH}.small.jpg`;
const PROXIED_THUMB = `kuzz-media://localhost/media/${HASH}.thumb.jpg`;
const PROXIED_ORIGINAL = `kuzz-media://localhost/media/${HASH}.png`;

async function mount(element) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(element));
  return {
    container,
    rerender: async (next) => act(async () => root.render(next)),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("small group avatar walks small, thumb, original, then the letter", async () => {
  const { container, unmount } = await mount(
    React.createElement(GroupLetterAvatar, {
      channelId: "group-1",
      label: "Team",
      picture: ORIGINAL,
      size: "md",
      testId: "avatar",
    }),
  );
  const image = () => container.querySelector("[data-testid='avatar-image']");

  assert.equal(image().getAttribute("src"), PROXIED_SMALL);
  await act(async () => fireEvent.error(image()));
  assert.equal(image().getAttribute("src"), PROXIED_THUMB);
  await act(async () => fireEvent.error(image()));
  assert.equal(image().getAttribute("src"), PROXIED_ORIGINAL);
  await act(async () => fireEvent.error(image()));
  assert.equal(image(), null);
  assert.ok(container.textContent.includes("T"));
  await unmount();
});

test("group GIF photos keep the animated original", async () => {
  const gif = `https://kuzz.example/media/${HASH}.gif`;
  const { container, unmount } = await mount(
    React.createElement(GroupLetterAvatar, {
      channelId: "group-1",
      label: "Team",
      picture: gif,
      testId: "avatar",
    }),
  );
  assert.equal(
    container.querySelector("[data-testid='avatar-image']").getAttribute("src"),
    `kuzz-media://localhost/media/${HASH}.gif`,
  );
  await unmount();
});

test("the 80px group avatar starts from the thumb variant", async () => {
  const { container, unmount } = await mount(
    React.createElement(GroupLetterAvatar, {
      channelId: "group-1",
      label: "Team",
      picture: ORIGINAL,
      size: "xl",
      testId: "avatar",
    }),
  );
  const image = () => container.querySelector("[data-testid='avatar-image']");
  assert.equal(image().getAttribute("src"), PROXIED_THUMB);
  await act(async () => fireEvent.error(image()));
  assert.equal(image().getAttribute("src"), PROXIED_ORIGINAL);
  await unmount();
});

test("a changed group photo starts again from its own variant", async () => {
  const other = `https://kuzz.example/media/${"d".repeat(64)}.jpg`;
  const props = { channelId: "group-1", label: "Team", testId: "avatar" };
  const { container, rerender, unmount } = await mount(
    React.createElement(GroupLetterAvatar, { ...props, picture: ORIGINAL }),
  );
  const image = () => container.querySelector("[data-testid='avatar-image']");
  await act(async () => fireEvent.error(image()));
  await act(async () => fireEvent.error(image()));
  assert.equal(image().getAttribute("src"), PROXIED_ORIGINAL);

  await rerender(
    React.createElement(GroupLetterAvatar, { ...props, picture: other }),
  );
  assert.equal(
    image().getAttribute("src"),
    `kuzz-media://localhost/media/${"d".repeat(64)}.small.jpg`,
  );
  await unmount();
});

test("clicking the group photo in Manage group opens the original", async () => {
  const { unmount } = await mount(
    React.createElement(GroupPhotoField, {
      channelId: "group-1",
      isPrivate: false,
      label: "Team",
      onPictureChange: () => {},
      picture: ORIGINAL,
    }),
  );
  const button = document.querySelector("[data-testid='group-photo-view']");
  assert.equal(button.getAttribute("aria-label"), "View group photo");
  // The sheet avatar itself still shows the thumbnail.
  assert.equal(
    document
      .querySelector("[data-testid='group-photo-avatar-image']")
      .getAttribute("src"),
    PROXIED_THUMB,
  );

  await act(async () => fireEvent.click(button));
  const fullSize = document.querySelector("img[alt='Team group photo']");
  assert.ok(fullSize, "lightbox image is shown");
  assert.equal(fullSize.getAttribute("src"), PROXIED_ORIGINAL);
  await unmount();
});

test("without a group photo there is nothing to open", async () => {
  const { unmount } = await mount(
    React.createElement(GroupPhotoField, {
      channelId: "group-1",
      isPrivate: false,
      label: "Team",
      onPictureChange: () => {},
      picture: "",
    }),
  );
  assert.equal(
    document.querySelector("[data-testid='group-photo-view']"),
    null,
  );
  await unmount();
});

// Radix Avatar probes each `src` with a detached `new window.Image()`; record
// the probes and let the test fail them.
function installImageSpy() {
  const probes = [];
  const OriginalImage = window.Image;
  class SpyImage {
    constructor() {
      this.listeners = {};
      probes.push(this);
    }
    addEventListener(type, listener) {
      this.listeners[type] = listener;
    }
    removeEventListener() {}
    fail() {
      this.listeners.error?.();
      this.onerror?.();
    }
  }
  window.Image = SpyImage;
  return {
    probes,
    restore: () => {
      window.Image = OriginalImage;
    },
  };
}

const SMALL_CHAIN = [PROXIED_SMALL, PROXIED_THUMB, PROXIED_ORIGINAL];
const THUMB_CHAIN = [PROXIED_THUMB, PROXIED_ORIGINAL];

for (const [name, render, chain] of [
  [
    "ProfileAvatar (default, larger spots)",
    (url) =>
      React.createElement(ProfileAvatar, { avatarUrl: url, label: "Ada" }),
    THUMB_CHAIN,
  ],
  [
    "ProfileAvatar mediaVariant=small",
    (url) =>
      React.createElement(ProfileAvatar, {
        avatarUrl: url,
        label: "Ada",
        mediaVariant: "small",
      }),
    SMALL_CHAIN,
  ],
  [
    "UserAvatar",
    (url) =>
      React.createElement(UserAvatar, { avatarUrl: url, displayName: "Ada" }),
    SMALL_CHAIN,
  ],
]) {
  test(`${name} walks its variants down to the original`, async () => {
    const spy = installImageSpy();
    try {
      const { unmount } = await mount(render(ORIGINAL));
      const latest = () => spy.probes.at(-1).src;
      for (const [index, expected] of chain.entries()) {
        assert.equal(latest(), expected);
        if (index < chain.length - 1) {
          await act(async () => spy.probes.at(-1).fail());
        }
      }
      await unmount();
    } finally {
      spy.restore();
    }
  });
}
