import assert from "node:assert/strict";
import test from "node:test";

// Settings › Appearance › Language: device language first, then the shipped
// languages by their own names; a pick is remembered on this computer.

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { fireEvent } = await import("@testing-library/react");
const { LanguageSetting } = await import("./AppearanceSettingsControls.tsx");
const { LOCALE_STORAGE_KEY } = await import(
  "../../../shared/i18n/localePreference.ts"
);

test("lists the device language and saves a pick", async () => {
  localStorage.removeItem(LOCALE_STORAGE_KEY);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(LanguageSetting)));

  const select = container.querySelector('[data-testid="language-select"]');
  const options = [...select.options].map((option) => [
    option.value,
    option.textContent,
  ]);
  assert.deepEqual(options, [
    ["system", "Same as device (English)"],
    ["en", "English"],
  ]);
  assert.equal(select.value, "system");

  await act(async () => fireEvent.change(select, { target: { value: "en" } }));
  assert.equal(select.value, "en");
  assert.equal(localStorage.getItem(LOCALE_STORAGE_KEY), "en");

  await act(async () =>
    fireEvent.change(select, { target: { value: "system" } }),
  );
  assert.equal(localStorage.getItem(LOCALE_STORAGE_KEY), null);

  await act(async () => root.unmount());
  container.remove();
});
