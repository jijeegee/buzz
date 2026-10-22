import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { parse, print } from "./icu.mjs";
import {
  apply,
  check,
  hash,
  loadConfig,
  prepare,
  pseudoMessage,
  syncIosLanguages,
  validate,
} from "./i18n.mjs";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);
const quiet = () => {};

function fixture(english) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuzz-i18n-"));
  fs.mkdirSync(path.join(root, "i18n", "glossary"), { recursive: true });
  for (const name of ["languages.json", "glossary/terms.json"]) {
    fs.copyFileSync(
      path.join(repoRoot, "i18n", name),
      path.join(root, "i18n", name),
    );
  }
  fs.writeFileSync(
    path.join(root, "i18n", "sources.json"),
    JSON.stringify({
      sources: [
        { id: "desktop", path: "desktop/{lang}.json" },
        { id: "mobile", path: "mobile/app_{lang_}.arb", localeHeader: true },
      ],
    }),
  );
  fs.mkdirSync(path.join(root, "desktop"));
  fs.mkdirSync(path.join(root, "mobile"));
  fs.writeFileSync(
    path.join(root, "desktop", "en.json"),
    JSON.stringify(english.desktop),
  );
  fs.writeFileSync(
    path.join(root, "mobile", "app_en.arb"),
    JSON.stringify(english.mobile),
  );
  return loadConfig(root);
}

const readJson = (config, rel) =>
  JSON.parse(fs.readFileSync(path.join(config.root, rel), "utf8"));
const writeOut = (config, lang, out) =>
  fs.writeFileSync(
    path.join(config.root, "i18n", "work", `${lang}.out.json`),
    JSON.stringify(out),
  );

test("parse and print round-trip ICU messages", () => {
  for (const message of [
    "Hello {name}",
    "{count, plural, =0 {No members} one {# member} other {# members}}",
    "{who, select, me {You} other {{who}}} joined",
    "Read <b>the guide</b> or <link>open it</link><br/>",
    "Don't stop",
    "{n, number} at {t, time, short}",
  ]) {
    assert.equal(print(parse(message)), message);
  }
  assert.throws(() => parse("Hello {name"), /unclosed|expected/);
  assert.throws(() => parse("{n, plural, one {x}}"), /other/);
});

test("validate catches placeholder, plural and script mistakes", () => {
  const en = "{count, plural, one {# member} other {# members}}";
  assert.deepEqual(
    validate({
      en,
      tr: "{count, plural, other {멤버 #명}}",
      lang: "ko",
      script: ["Hangul"],
    }).errors,
    [],
  );
  assert.match(
    validate({
      en,
      tr: "{count, plural, one {# عضو} other {# أعضاء}}",
      lang: "ar",
      script: ["Arabic"],
    }).errors[0],
    /needs: zero, two, few, many/,
  );
  assert.match(
    validate({
      en: "Hi {name}",
      tr: "안녕 {이름}",
      lang: "ko",
      script: ["Hangul"],
    }).errors[0],
    /differ/,
  );
  assert.match(
    validate({ en: "Open Kuzz", tr: "열기", lang: "ko", script: ["Hangul"] })
      .errors[0],
    /Kuzz/,
  );
  assert.match(
    validate({
      en: "Send message",
      tr: "Send message",
      lang: "hi",
      script: ["Devanagari"],
    }).errors[0],
    /untranslated/,
  );
  assert.deepEqual(
    validate({ en: "Send message", tr: "Send message", lang: "fr" }).warnings,
    ["same as English"],
  );
  assert.match(
    validate({
      en: "Save",
      tr: "Änderungen jetzt speichern",
      lang: "de",
      note: "Button. Short.",
    }).warnings[0],
    /longer/,
  );
  assert.match(
    validate({
      en: "{who, select, me {You} other {Someone}}",
      tr: "{who, select, other {Jemand}}",
      lang: "de",
    }).errors[0],
    /options must be exactly/,
  );
});

test("prepare -> apply -> prepare only asks for new or changed English", () => {
  const config = fixture({
    desktop: {
      "chats.empty": "No chats yet",
      "@chats.empty": { description: "Empty Chats list title. Short." },
      "members.count": "{count, plural, one {# member} other {# members}}",
      "@members.count": { description: "Room header, after the name" },
    },
    mobile: {
      "chats.empty": "No chats yet",
      "@chats.empty": { description: "Empty Chats tab title. Short." },
    },
  });

  assert.deepEqual(prepare(config, ["ko"], quiet), { ko: 3 });
  writeOut(config, "ko", {
    "desktop:chats.empty": "아직 채팅이 없어요",
    "desktop:members.count": "{count, plural, other {멤버 #명}}",
    "mobile:chats.empty": "No chats yet",
  });
  assert.deepEqual(apply(config, ["ko"], quiet), {
    ko: { applied: 2, remaining: 1 },
  });
  assert.deepEqual(readJson(config, "desktop/ko.json"), {
    "chats.empty": "아직 채팅이 없어요",
    "members.count": "{count, plural, other {멤버 #명}}",
  });
  const left = readJson(config, "i18n/work/ko.json").items;
  assert.equal(left.length, 1);
  assert.match(left[0].problems[0], /untranslated/);

  // A fresh prepare offers the desktop translation of the same sentence.
  prepare(config, ["ko"], quiet);
  assert.equal(
    readJson(config, "i18n/work/ko.json").items[0].suggestion,
    "아직 채팅이 없어요",
  );
  writeOut(config, "ko", { "mobile:chats.empty": "아직 채팅이 없어요" });
  apply(config, ["ko"], quiet);
  assert.deepEqual(readJson(config, "mobile/app_ko.arb"), {
    "@@locale": "ko",
    "chats.empty": "아직 채팅이 없어요",
  });
  assert.deepEqual(prepare(config, ["ko"], quiet), {});

  // A human fix in the language file is kept; changed English is re-asked with the old text.
  fs.writeFileSync(
    path.join(config.root, "desktop", "ko.json"),
    JSON.stringify({
      "chats.empty": "채팅이 없어요",
      "members.count": "{count, plural, other {멤버 #명}}",
    }),
  );
  assert.deepEqual(prepare(config, ["ko"], quiet), {});
  const english = readJson(config, "desktop/en.json");
  english["members.count"] = "{count, plural, one {# person} other {# people}}";
  fs.writeFileSync(
    path.join(config.root, "desktop", "en.json"),
    JSON.stringify(english),
  );
  prepare(config, ["ko"], quiet);
  const [item] = readJson(config, "i18n/work/ko.json").items;
  assert.equal(item.id, "desktop:members.count");
  assert.equal(item.previous, "{count, plural, other {멤버 #명}}");
  assert.equal(
    readJson(config, "i18n/state/ko.json").desktop["chats.empty"],
    hash("No chats yet"),
  );
});

test("check reports invalid files and missing notes", () => {
  const config = fixture({
    desktop: {
      hello: "Hello {name}",
      "@hello": { description: "Greeting" },
      bye: "Bye",
    },
    mobile: {},
  });
  fs.writeFileSync(
    path.join(config.root, "desktop", "ko.json"),
    JSON.stringify({ hello: "안녕", gone: "x" }),
  );
  const problems = check(config, quiet);
  assert.ok(problems.some((p) => p.includes("bye: missing @bye.description")));
  assert.ok(
    problems.some((p) => p.includes("ko hello: placeholders/tags differ")),
  );
  assert.ok(problems.some((p) => p.includes("ko gone: not in English")));
});

test("pseudo language keeps ICU structure", () => {
  const out = pseudoMessage(
    "{count, plural, one {# member} other {# members}} in {room}",
  );
  assert.match(
    out,
    /^\[\{count, plural, one \{# ɱéɱƀéŕ\} other \{# ɱéɱƀéŕš\}\} íñ \{room\} ~+\]$/,
  );
  assert.doesNotThrow(() => parse(out));
});

test("repository language packs are valid", () => {
  assert.deepEqual(check(loadConfig(repoRoot), quiet), []);
});

test("iOS language list follows the shipped mobile files", () => {
  const config = fixture({
    desktop: {},
    mobile: {
      "@@locale": "en",
      chatsEmpty: "No chats yet",
      "@chatsEmpty": { description: "Empty Chats tab title. Short." },
    },
  });
  const mobile = config.sources.find((src) => src.id === "mobile");
  mobile.iosInfoPlist = "mobile/Info.plist";
  const plist = path.join(config.root, "mobile", "Info.plist");
  fs.writeFileSync(
    plist,
    "<dict>\n\t<key>CFBundleDevelopmentRegion</key>\n\t<string>en</string>\n\t<key>CFBundleName</key>\n</dict>\n",
  );

  assert.match(check(config, quiet).join("\n"), /CFBundleLocalizations/);
  assert.deepEqual(syncIosLanguages(config), ["mobile/Info.plist"]);
  assert.deepEqual(check(config, quiet), []);

  fs.writeFileSync(
    path.join(config.root, "mobile", "app_ko.arb"),
    JSON.stringify({ "@@locale": "ko", chatsEmpty: "아직 채팅이 없어요" }),
  );
  fs.writeFileSync(
    path.join(config.root, "mobile", "app_zh_Hant.arb"),
    JSON.stringify({ "@@locale": "zh_Hant", chatsEmpty: "還沒有聊天" }),
  );
  assert.deepEqual(syncIosLanguages(config), ["mobile/Info.plist"]);
  assert.equal(
    fs.readFileSync(plist, "utf8"),
    "<dict>\n\t<key>CFBundleDevelopmentRegion</key>\n\t<string>en</string>\n" +
      "\t<key>CFBundleLocalizations</key>\n\t<array>\n" +
      "\t\t<string>en</string>\n\t\t<string>zh-Hant</string>\n\t\t<string>ko</string>\n" +
      "\t</array>\n\t<key>CFBundleName</key>\n</dict>\n",
  );
  assert.deepEqual(syncIosLanguages(config), []);
});
