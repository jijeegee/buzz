import assert from "node:assert/strict";
import { test } from "node:test";

import { buildIntl, getIntl, setIntl, t } from "./intl.ts";
import { languageDirection, matchLocale } from "./locales.ts";

test("matchLocale picks the first supported device language", () => {
  const available = ["en", "ko", "zh-Hans", "zh-Hant", "pt-BR", "es", "fil"];
  assert.equal(matchLocale(["ko-KR", "en-US"], available), "ko");
  assert.equal(matchLocale(["de-DE", "es-MX"], available), "es");
  assert.equal(matchLocale(["zh-TW"], available), "zh-Hant");
  assert.equal(matchLocale(["zh-Hant-HK"], available), "zh-Hant");
  assert.equal(matchLocale(["zh-CN"], available), "zh-Hans");
  assert.equal(matchLocale(["pt-PT"], available), "pt-BR");
  assert.equal(matchLocale(["tl-PH"], available), "fil");
  assert.equal(matchLocale(["ja-JP"], available), "en");
  assert.equal(matchLocale([], available), "en");
  // A language without a shipped file falls through to the next preference.
  assert.equal(matchLocale(["ko"], ["en", "ja"]), "en");
});

test("languageDirection follows i18n/languages.json", () => {
  assert.equal(languageDirection("ar"), "rtl");
  assert.equal(languageDirection("ko"), "ltr");
  assert.equal(languageDirection("xx"), "ltr");
});

test("t() shows English for strings a language lacks", () => {
  const previous = getIntl();
  try {
    assert.equal(t("nav.chats"), "Chats");
    setIntl(buildIntl("ko", { "nav.chats": "채팅" }));
    assert.equal(t("nav.chats"), "채팅");
    assert.equal(t("nav.friends"), "Friends");
  } finally {
    setIntl(previous);
  }
});

test("dates and numbers follow the app language, not the system", async () => {
  const { localeDateTimeFormat, localeNumberFormat, localeRelativeTimeFormat } =
    await import("./format.ts");
  const month = localeDateTimeFormat({ month: "long" });
  const count = localeNumberFormat();
  const ago = localeRelativeTimeFormat({ numeric: "always" });
  const date = new Date(2026, 9, 23);
  const previous = getIntl();
  try {
    setIntl(buildIntl("en", {}));
    assert.equal(month.format(date), "October");
    assert.equal(count.format(1234.5), "1,234.5");
    assert.equal(ago.format(-3, "hour"), "3 hours ago");
    setIntl(buildIntl("de", {}));
    assert.equal(month.format(date), "Oktober");
    assert.equal(count.format(1234.5), "1.234,5");
    setIntl(buildIntl("ko", {}));
    assert.equal(ago.format(-3, "hour"), "3시간 전");
  } finally {
    setIntl(previous);
  }
});
