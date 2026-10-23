#!/usr/bin/env node
// Language-pack tooling. No network and no AI calls: agents translate the
// work files this script prepares (see i18n/TRANSLATING.md).
//
//   node i18n/scripts/i18n.mjs prepare [lang...]  new/changed English -> i18n/work/<lang>.json
//   node i18n/scripts/i18n.mjs apply [lang...]    i18n/work/<lang>.out.json -> language files
//   node i18n/scripts/i18n.mjs check              validate every language file (CI)
//   node i18n/scripts/i18n.mjs status [lang...]   per language and tier: texts translated / total
//   node i18n/scripts/i18n.mjs park [lang...]     move translations above the current tier to i18n/later/
//   node i18n/scripts/i18n.mjs pseudo             write the en-XA pseudo language (dev only)

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  choices,
  IcuError,
  mapText,
  parse,
  print,
  signature,
  visibleText,
} from "./icu.mjs";

export const PSEUDO = "en-XA";
const SHORT_NOTE = /\bshort\b/i;
const MAX_SHORT_RATIO = 1.6;

// ---------- config and files ----------

export function loadConfig(root) {
  const read = (name) =>
    JSON.parse(fs.readFileSync(path.join(root, "i18n", name), "utf8"));
  const { source, languages } = read("languages.json");
  const { sources } = read("sources.json");
  return { root, source, languages, sources };
}

export function sourcePath(config, src, lang) {
  return path.join(
    config.root,
    src.path
      .replace("{lang_}", lang.replaceAll("-", "_"))
      .replace("{lang}", lang),
  );
}

function readJson(file) {
  return fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")) : null;
}

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

/** English source: { key: { text, note } } in file order, or null when the app has none yet. */
export function readSource(config, src) {
  const raw = readJson(sourcePath(config, src, config.source));
  if (!raw) return null;
  const out = {};
  for (const [key, value] of Object.entries(raw)) {
    if (key.startsWith("@")) continue;
    out[key] = { text: value, note: raw[`@${key}`]?.description ?? "" };
  }
  return out;
}

/** A translated file as { key: text }, or {} when missing. */
export function readTarget(config, src, lang) {
  const raw = readJson(sourcePath(config, src, lang)) ?? {};
  return Object.fromEntries(
    Object.entries(raw).filter(([key]) => !key.startsWith("@")),
  );
}

function writeTarget(config, src, lang, english, messages) {
  const out = src.localeHeader ? { "@@locale": lang.replaceAll("-", "_") } : {};
  for (const key of Object.keys(english)) {
    if (key in messages) out[key] = messages[key];
  }
  writeJson(sourcePath(config, src, lang), out);
}

export function hash(text) {
  return createHash("sha256").update(text).digest("hex").slice(0, 12);
}

const statePath = (config, lang) =>
  path.join(config.root, "i18n", "state", `${lang}.json`);
const laterPath = (config, lang) =>
  path.join(config.root, "i18n", "later", `${lang}.json`);
const glossaryPath = (config, lang) =>
  path.join(config.root, "i18n", "glossary", `${lang}.json`);

// ---------- glossary and tiers ----------

const escapeRegExp = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * Matches `term` as a plain word in English, singular or plural, any case;
 * not inside code-like tokens (`@mention`, kuzz-agent, URLs) or the product
 * name "Kuzz Agent".
 */
function termPattern(term) {
  const word = escapeRegExp(term);
  const plural = term.endsWith("y")
    ? `${escapeRegExp(term.slice(0, -1))}(?:y|ies)`
    : `${word}(?:s|es)?`;
  return new RegExp(
    `(?<![\\w@./:-])(?<!Kuzz )${plural}(?![\\w./:-]*\\w)`,
    "i",
  );
}

/** English text with code spans and URLs removed, for term matching. */
const plainEnglish = (text) =>
  text.replace(/`[^`]*`/g, " ").replace(/\b\w+:\/\/\S+/g, " ");

/**
 * Product words that must always be said the same way in `lang`:
 * [{ term, forms }]. Terms marked `fixed` in glossary/terms.json; the language
 * glossary gives one word or a list of accepted forms (first is preferred).
 */
export function fixedTerms(config, lang) {
  const terms =
    readJson(path.join(config.root, "i18n", "glossary", "terms.json")) ?? {};
  const words = readJson(glossaryPath(config, lang)) ?? {};
  return Object.entries(words)
    .filter(([term]) => terms[term]?.fixed)
    .map(([term, value]) => ({
      term,
      pattern: termPattern(term),
      forms: [value].flat(),
    }));
}

const wordCount = (text) =>
  text.trim().split(/\s+/).filter(Boolean).length;

function ruleMatches(rule, id, text, words) {
  if (rule.keys && !rule.keys.some((prefix) => id.startsWith(prefix)))
    return false;
  if (rule.text && !new RegExp(rule.text, "i").test(text)) return false;
  if (rule.maxWords !== undefined && words > rule.maxWords) return false;
  return true;
}

/**
 * Release tiers from i18n/tiers.json: `current` is the highest tier being
 * translated, `of(text)` the tier of an English text (a number, or "skip" for
 * text that stays English). The first matching rule wins; one text has one
 * tier, the earliest any of its uses needs. Without tiers.json all is tier 1.
 */
export function readTiers(config, english) {
  const rules = readJson(path.join(config.root, "i18n", "tiers.json"));
  const rank = (tier) => (tier === "skip" ? Infinity : tier);
  const byText = new Map();
  for (const src of config.sources) {
    for (const [key, { text }] of Object.entries(english[src.id] ?? {})) {
      let tier = 1;
      if (rules) {
        const id = `${src.id}:${key}`;
        const words = wordCount(visibleText(parse(text)));
        const rule = rules.rules.find((r) => ruleMatches(r, id, text, words));
        tier = rule ? rule.tier : rules.default;
      }
      const prev = byText.get(text);
      if (prev === undefined || rank(tier) < rank(prev)) byText.set(text, tier);
    }
  }
  const current = rules?.current ?? 1;
  return {
    current,
    of: (text) => byText.get(text) ?? 1,
    open: (text) => rank(byText.get(text) ?? 1) <= current,
    all: [...new Set(byText.values())]
      .filter((tier) => tier !== "skip")
      .sort((a, b) => a - b),
  };
}
const workPath = (config, lang, suffix = "") =>
  path.join(config.root, "i18n", "work", `${lang}${suffix}.json`);

// ---------- iOS language list ----------

// iOS only offers an app the languages named in its Info.plist
// CFBundleLocalizations, so that list follows the shipped mobile files.
const PLIST_BLOCK =
  /\t<key>CFBundleLocalizations<\/key>\n\t<array>\n(?:\t\t<string>[^<]*<\/string>\n)*\t<\/array>\n/;

/** Languages (source first) that ship a file for `src`, in languages.json order. */
function shippedLanguages(config, src) {
  return config.languages
    .map((lang) => lang.code)
    .filter(
      (code) =>
        code === config.source || fs.existsSync(sourcePath(config, src, code)),
    );
}

/** The Info.plist text with CFBundleLocalizations set for `src`. */
function iosPlistText(config, src) {
  const file = path.join(config.root, src.iosInfoPlist);
  const text = fs.readFileSync(file, "utf8");
  const block = `\t<key>CFBundleLocalizations</key>\n\t<array>\n${shippedLanguages(
    config,
    src,
  )
    .map((code) => `\t\t<string>${code}</string>\n`)
    .join("")}\t</array>\n`;
  if (PLIST_BLOCK.test(text))
    return { file, text, next: text.replace(PLIST_BLOCK, block) };
  const anchor =
    /(\t<key>CFBundleDevelopmentRegion<\/key>\n\t<string>[^<]*<\/string>\n)/;
  if (!anchor.test(text))
    throw new Error(`${src.iosInfoPlist}: no CFBundleDevelopmentRegion entry`);
  return { file, text, next: text.replace(anchor, `$1${block}`) };
}

/** Rewrites Info.plist language lists; returns the files that changed. */
export function syncIosLanguages(config) {
  const changed = [];
  for (const src of config.sources) {
    if (!src.iosInfoPlist) continue;
    const { file, text, next } = iosPlistText(config, src);
    if (next !== text) {
      fs.writeFileSync(file, next);
      changed.push(src.iosInfoPlist);
    }
  }
  return changed;
}

// ---------- validation ----------

function pluralCategories(lang, ordinal) {
  return new Intl.PluralRules(lang, {
    type: ordinal ? "ordinal" : "cardinal",
  }).resolvedOptions().pluralCategories;
}

/**
 * Checks one translation against its English source.
 * Returns { errors: string[], warnings: string[] }.
 */
export function validate({ en, tr, lang, note = "", script = [], terms = [] }) {
  const errors = [];
  const warnings = [];
  if (typeof tr !== "string" || tr.trim() === "")
    return { errors: ["empty translation"], warnings };

  let enAst;
  let trAst;
  try {
    enAst = parse(en);
  } catch (error) {
    return { errors: [`English is not valid ICU: ${error.message}`], warnings };
  }
  try {
    trAst = parse(tr);
  } catch (error) {
    if (!(error instanceof IcuError)) throw error;
    return { errors: [`not valid ICU: ${error.message}`], warnings };
  }

  const want = signature(enAst).join(", ");
  const got = signature(trAst).join(", ");
  if (want !== got)
    errors.push(`placeholders/tags differ: expected [${want}] got [${got}]`);

  const enChoices = new Map(choices(enAst).map((node) => [node.name, node]));
  for (const node of choices(trAst)) {
    const selectors = Object.keys(node.options);
    if (node.type === "select") {
      const enNode = enChoices.get(node.name);
      const expected = enNode
        ? Object.keys(enNode.options).sort().join(",")
        : "";
      if (selectors.slice().sort().join(",") !== expected) {
        errors.push(
          `select {${node.name}} options must be exactly: ${expected}`,
        );
      }
      continue;
    }
    const missing = pluralCategories(
      lang,
      node.type === "selectordinal",
    ).filter((cat) => !selectors.includes(cat));
    if (missing.length)
      errors.push(`plural {${node.name}} needs: ${missing.join(", ")}`);
  }

  if (/\bKuzz\b/.test(en) && !/\bKuzz\b/.test(tr))
    errors.push(`keep "Kuzz" as is`);

  const enText = visibleText(enAst).trim();
  const trText = visibleText(trAst).trim();
  if (script.length && /\p{L}{3}/u.test(enText)) {
    const pattern = new RegExp(
      script.map((name) => `\\p{Script=${name}}`).join("|"),
      "u",
    );
    if (!pattern.test(trText))
      errors.push(`no ${script.join("/")} letters: looks untranslated`);
  } else if (trText === enText && /\p{L}+\s+\p{L}+/u.test(enText)) {
    warnings.push("same as English");
  }

  const lowered = trText.toLowerCase();
  const plain = plainEnglish(enText);
  for (const { term, pattern, forms } of terms) {
    if (
      pattern.test(plain) &&
      !forms.some((form) => lowered.includes(form.toLowerCase()))
    )
      errors.push(`say "${term}" as the glossary does: ${forms.join(" / ")}`);
  }

  if (
    SHORT_NOTE.test(note) &&
    [...trText].length > MAX_SHORT_RATIO * [...enText].length + 2
  ) {
    warnings.push(`longer than ${MAX_SHORT_RATIO}x English for a short label`);
  }
  return { errors, warnings };
}

// ---------- commands ----------

function targetLanguages(config, requested) {
  const all = config.languages.filter((lang) => lang.code !== config.source);
  if (!requested.length) return all.filter((lang) => lang.enabled);
  const unknown = requested.filter(
    (code) => !all.some((lang) => lang.code === code),
  );
  if (unknown.length)
    throw new Error(`unknown language: ${unknown.join(", ")}`);
  return all.filter((lang) => requested.includes(lang.code));
}

/** Up-to-date translations of each English sentence, across every app (translation memory). */
function memory(config, english, lang, state) {
  const out = new Map();
  for (const src of config.sources) {
    const target = readTarget(config, src, lang);
    for (const [key, { text }] of Object.entries(english[src.id] ?? {})) {
      if (
        key in target &&
        state[src.id]?.[key] === hash(text) &&
        !out.has(text)
      )
        out.set(text, target[key]);
    }
  }
  return out;
}

export function prepare(config, requested = [], log = console.log) {
  const english = Object.fromEntries(
    config.sources.map((src) => [src.id, readSource(config, src)]),
  );
  const glossaryTerms = readJson(
    path.join(config.root, "i18n", "glossary", "terms.json"),
  );
  const results = {};
  const tiers = readTiers(config, english);
  for (const lang of targetLanguages(config, requested)) {
    const state = readJson(statePath(config, lang.code)) ?? {};
    const later = readJson(laterPath(config, lang.code)) ?? {};
    const tm = memory(config, english, lang.code, state);
    // One item per English text; its other uses ride along in `also`.
    const byText = new Map();
    for (const src of config.sources) {
      const source = english[src.id];
      if (!source) continue;
      const target = readTarget(config, src, lang.code);
      for (const [key, { text, note }] of Object.entries(source)) {
        if (!tiers.open(text)) continue;
        const current = target[key];
        if (current !== undefined && state[src.id]?.[key] === hash(text))
          continue;
        const id = `${src.id}:${key}`;
        const same = byText.get(text);
        if (same) {
          (same.also ??= []).push(id);
          continue;
        }
        const item = { id, en: text, note };
        const parked = later[src.id]?.[key];
        if (current !== undefined) item.previous = current;
        if (tm.has(text)) item.suggestion = tm.get(text);
        else if (parked?.from === hash(text)) item.suggestion = parked.text;
        byText.set(text, item);
      }
    }
    const items = [...byText.values()];
    const work = workPath(config, lang.code);
    if (!items.length) {
      fs.rmSync(work, { force: true });
      log(`${lang.code}: up to date`);
      continue;
    }
    const glossaryFile = path.join(
      config.root,
      "i18n",
      "glossary",
      `${lang.code}.json`,
    );
    writeJson(work, {
      lang: lang.code,
      language: `${lang.english} (${lang.name})`,
      dir: lang.dir,
      glossary: readJson(glossaryFile),
      glossaryTerms: fs.existsSync(glossaryFile) ? undefined : glossaryTerms,
      items,
    });
    results[lang.code] = items.length;
    log(
      `${lang.code}: ${items.length} to translate -> ${path.relative(config.root, work)}`,
    );
  }
  return results;
}

export function apply(config, requested = [], log = console.log) {
  const summary = {};
  for (const lang of targetLanguages(config, requested)) {
    const work = readJson(workPath(config, lang.code));
    const out = readJson(workPath(config, lang.code, ".out"));
    if (!work || !out) continue;
    const state = readJson(statePath(config, lang.code)) ?? {};
    const terms = fixedTerms(config, lang.code);
    const bySource = new Map();
    const remaining = [];
    let applied = 0;
    for (const item of work.items) {
      const tr = out[item.id];
      if (tr === undefined) {
        remaining.push(item);
        continue;
      }
      const { errors, warnings } = validate({
        en: item.en,
        tr,
        lang: lang.code,
        note: item.note,
        script: lang.script,
        terms,
      });
      if (errors.length) {
        remaining.push({ ...item, rejected: tr, problems: errors });
        continue;
      }
      for (const warning of warnings)
        log(`  warn ${lang.code} ${item.id}: ${warning}`);
      for (const id of [item.id, ...(item.also ?? [])]) {
        const [srcId, ...rest] = id.split(":");
        const key = rest.join(":");
        if (!bySource.has(srcId)) bySource.set(srcId, {});
        bySource.get(srcId)[key] = tr;
        state[srcId] = { ...state[srcId], [key]: hash(item.en) };
        applied += 1;
      }
    }
    for (const src of config.sources) {
      const english = readSource(config, src);
      if (!english) continue;
      const messages = {
        ...readTarget(config, src, lang.code),
        ...bySource.get(src.id),
      };
      if (Object.keys(messages).length)
        writeTarget(config, src, lang.code, english, messages);
      if (state[src.id]) {
        state[src.id] = Object.fromEntries(
          Object.entries(state[src.id]).filter(([key]) => key in english),
        );
      }
    }
    writeJson(statePath(config, lang.code), sortKeys(state));
    fs.rmSync(workPath(config, lang.code, ".out"), { force: true });
    if (remaining.length)
      writeJson(workPath(config, lang.code), { ...work, items: remaining });
    else fs.rmSync(workPath(config, lang.code), { force: true });
    summary[lang.code] = { applied, remaining: remaining.length };
    log(
      `${lang.code}: applied ${applied}, ${remaining.length} left in work file`,
    );
  }
  for (const file of syncIosLanguages(config))
    log(`${file}: language list updated`);
  return summary;
}

function sortKeys(state) {
  return Object.fromEntries(
    Object.keys(state)
      .sort()
      .map((id) => [
        id,
        Object.fromEntries(
          Object.keys(state[id])
            .sort()
            .map((key) => [key, state[id][key]]),
        ),
      ]),
  );
}

/** Returns a list of problems; empty means every language file is valid. */
export function check(config, log = console.log) {
  const problems = [];
  const codes = new Set(config.languages.map((lang) => lang.code));
  if (!codes.has(config.source))
    problems.push(
      `source language ${config.source} missing from languages.json`,
    );
  for (const src of config.sources) {
    const english = readSource(config, src);
    if (!english) continue;
    for (const [key, { text, note }] of Object.entries(english)) {
      try {
        parse(text);
      } catch (error) {
        problems.push(`${src.id} en ${key}: not valid ICU: ${error.message}`);
      }
      if (!note.trim())
        problems.push(
          `${src.id} en ${key}: missing @${key}.description (where it appears)`,
        );
    }
    for (const lang of config.languages) {
      if (lang.code === config.source) continue;
      const file = sourcePath(config, src, lang.code);
      if (!fs.existsSync(file)) continue;
      const target = readTarget(config, src, lang.code);
      const terms = fixedTerms(config, lang.code);
      let missing = 0;
      for (const key of Object.keys(target)) {
        if (!(key in english))
          problems.push(
            `${src.id} ${lang.code} ${key}: not in English (stale key)`,
          );
      }
      for (const [key, { text, note }] of Object.entries(english)) {
        if (!(key in target)) {
          missing += 1;
          continue;
        }
        const { errors } = validate({
          en: text,
          tr: target[key],
          lang: lang.code,
          note,
          script: lang.script,
          terms,
        });
        for (const error of errors)
          problems.push(`${src.id} ${lang.code} ${key}: ${error}`);
      }
      if (missing)
        log(
          `${src.id} ${lang.code}: ${missing} not translated yet (shown in English)`,
        );
    }
    if (src.iosInfoPlist) {
      const { text, next } = iosPlistText(config, src);
      if (next !== text)
        problems.push(
          `${src.iosInfoPlist}: CFBundleLocalizations does not match the ${src.id} language files (run i18n.mjs apply)`,
        );
    }
  }
  return problems;
}

/**
 * Per language and tier, how many English texts are translated and up to
 * date: { lang: { tier: { done, total } } }. A text used in several places
 * counts once, and is done when every use is.
 */
export function status(config, requested = [], log = console.log) {
  const english = Object.fromEntries(
    config.sources.map((src) => [src.id, readSource(config, src)]),
  );
  const tiers = readTiers(config, english);
  const result = {};
  for (const lang of targetLanguages(config, requested)) {
    const state = readJson(statePath(config, lang.code)) ?? {};
    const done = new Map();
    for (const src of config.sources) {
      const target = readTarget(config, src, lang.code);
      for (const [key, { text }] of Object.entries(english[src.id] ?? {})) {
        const ok =
          key in target && state[src.id]?.[key] === hash(text);
        done.set(text, (done.get(text) ?? true) && ok);
      }
    }
    const row = {};
    for (const tier of tiers.all) row[tier] = { done: 0, total: 0 };
    for (const [text, ok] of done) {
      const tier = tiers.of(text);
      if (tier === "skip") continue;
      row[tier].total += 1;
      if (ok) row[tier].done += 1;
    }
    result[lang.code] = row;
    log(
      `${lang.code.padEnd(8)}${tiers.all
        .map((tier) => `  tier ${tier}: ${row[tier].done}/${row[tier].total}`)
        .join("")}`,
    );
  }
  log(`(current tier: ${tiers.current})`);
  return result;
}

/**
 * Moves translations of text above the current tier (or marked "skip") out
 * of the language files into i18n/later/<lang>.json, so every language shows
 * the same English. prepare offers them again when their tier opens.
 */
export function park(config, requested = [], log = console.log) {
  const english = Object.fromEntries(
    config.sources.map((src) => [src.id, readSource(config, src)]),
  );
  const tiers = readTiers(config, english);
  const summary = {};
  for (const lang of targetLanguages(config, requested)) {
    const state = readJson(statePath(config, lang.code)) ?? {};
    const later = readJson(laterPath(config, lang.code)) ?? {};
    let moved = 0;
    for (const src of config.sources) {
      const source = english[src.id];
      if (!source) continue;
      const target = readTarget(config, src, lang.code);
      for (const [key, { text }] of Object.entries(source)) {
        if (!(key in target) || tiers.open(text)) continue;
        later[src.id] = {
          ...later[src.id],
          [key]: { text: target[key], from: state[src.id]?.[key] ?? "" },
        };
        delete target[key];
        if (state[src.id]) delete state[src.id][key];
        moved += 1;
      }
      if (fs.existsSync(sourcePath(config, src, lang.code)))
        writeTarget(config, src, lang.code, source, target);
    }
    if (!moved) continue;
    writeJson(statePath(config, lang.code), sortKeys(state));
    writeJson(laterPath(config, lang.code), sortKeys(later));
    summary[lang.code] = moved;
    log(`${lang.code}: parked ${moved} translations in i18n/later/`);
  }
  return summary;
}

const ACCENTS = Object.fromEntries(
  [..."abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"].map((ch, i) => [
    ch,
    [..."áƀçđéƒĝĥíĵķĺɱñóþǫŕšţúṽŵẋýžÁƁÇĐÉƑĜĤÍĴĶĹṀÑÓÞǪŔŠŢÚṼŴẊÝŽ"][i],
  ]),
);

/** Accented and ~40% longer, so untranslated text and tight layouts stand out. */
export function pseudoMessage(text) {
  const ast = parse(text);
  const accented = mapText(ast, (value) =>
    [...value].map((ch) => ACCENTS[ch] ?? ch).join(""),
  );
  const pad = "~".repeat(
    Math.max(1, Math.round([...visibleText(ast)].length * 0.4)),
  );
  return `[${print(accented)} ${pad}]`;
}

export function pseudo(config, log = console.log) {
  for (const src of config.sources) {
    const english = readSource(config, src);
    if (!english) continue;
    const messages = Object.fromEntries(
      Object.entries(english).map(([key, { text }]) => [
        key,
        pseudoMessage(text),
      ]),
    );
    writeTarget(config, src, PSEUDO, english, messages);
    log(
      `${src.id}: wrote ${path.relative(config.root, sourcePath(config, src, PSEUDO))}`,
    );
  }
}

// ---------- CLI ----------

function main(argv) {
  const root = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "..",
    "..",
  );
  const config = loadConfig(root);
  const [command, ...args] = argv;
  switch (command) {
    case "prepare":
      prepare(config, args);
      return 0;
    case "apply":
      apply(config, args);
      return 0;
    case "check": {
      const problems = check(config);
      for (const problem of problems) console.error(problem);
      if (problems.length) {
        console.error(`${problems.length} language-pack problem(s)`);
        return 1;
      }
      console.log("language packs ok");
      return 0;
    }
    case "status":
      status(config, args);
      return 0;
    case "park":
      park(config, args);
      return 0;
    case "pseudo":
      pseudo(config);
      return 0;
    default:
      console.error(
        "usage: i18n.mjs prepare|apply|check|status|park|pseudo [lang...]",
      );
      return 1;
  }
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  process.exitCode = main(process.argv.slice(2));
}
