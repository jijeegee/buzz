// Turns prepared work files into translator prompts for sub-agents.
//
//   node i18n/agents/make_prompts.mjs <outDir> <lang>... [--size 400] [--per 2]
//   node i18n/agents/make_prompts.mjs <outDir> --fix <lang>...
//
// Run `node i18n/scripts/i18n.mjs prepare <lang>...` first. For each language
// this splits i18n/work/<lang>.json into chunks of --size items, writes one
// prompt per --per chunks to <outDir>/<lang>_<n>.md, and appends the prompt
// paths to <outDir>/queue.txt. With --fix it writes <outDir>/fix_<lang>.md
// for the items `apply` rejected. See i18n/agents/RUNBOOK.md.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..").replaceAll("\\", "/");
const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(name);
  if (i < 0) return fallback;
  const [, value] = args.splice(i, 2);
  return Number(value);
};
const size = option("--size", 400);
const per = option("--per", 2);
const fix = args.includes("--fix");
const [outDir, ...langs] = args.filter((arg) => arg !== "--fix");
if (!outDir || !langs.length) {
  console.error("usage: make_prompts.mjs <outDir> [--fix] <lang>... [--size N] [--per N]");
  process.exit(1);
}
fs.mkdirSync(outDir, { recursive: true });

const languages = JSON.parse(fs.readFileSync(path.join(repo, "i18n/languages.json"), "utf8")).languages;
const styles = new Map(
  fs.readFileSync(path.join(here, "styles.tsv"), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => line.split("\t")),
);
const fill = (template, values) =>
  Object.entries(values).reduce((text, [key, value]) => text.replaceAll(`{${key}}`, value), template);
const queue = path.join(outDir, "queue.txt");

for (const lang of langs) {
  const info = languages.find((l) => l.code === lang);
  if (!info) throw new Error(`unknown language ${lang}`);
  const style = styles.get(lang) ?? "";
  if (fix) {
    const file = path.join(outDir, `fix_${lang}.md`);
    const text = fill(fs.readFileSync(path.join(here, "fix_prompt.md"), "utf8"), { REPO: repo, LANG: lang });
    fs.writeFileSync(file, `${text}\n${style}\n\nDo not start sub-agents.\n`);
    console.log(file);
    continue;
  }
  const chunksDir = path.join(repo, "i18n/work/chunks");
  for (const old of fs.existsSync(chunksDir) ? fs.readdirSync(chunksDir) : []) {
    if (old.startsWith(`${lang}.`)) fs.rmSync(path.join(chunksDir, old));
  }
  execFileSync(process.execPath, [path.join(here, "chunks.mjs"), "split", repo, lang, String(size)], { stdio: "inherit" });
  const chunks = fs.readdirSync(chunksDir).filter((f) => new RegExp(`^${lang}\\.\\d{2}\\.json$`).test(f)).sort();
  const template = fs.readFileSync(path.join(here, "translator_prompt.md"), "utf8");
  for (let k = 0, n = 1; k < chunks.length; k += per, n++) {
    const list = chunks.slice(k, k + per).map((f) => `${repo}/i18n/work/chunks/${f}`).join(", ");
    const file = path.join(outDir, `${lang}_${n}.md`).replaceAll("\\", "/");
    const text = fill(template, { REPO: repo, LANG: lang, LANGUAGE: `${info.english} (${info.name})`, CHUNKS: list });
    fs.writeFileSync(file, `${text}\n${style}\n\nDo not start sub-agents.\n`);
    fs.appendFileSync(queue, `${file}\n`);
  }
}
if (!fix) console.log(`${fs.readFileSync(queue, "utf8").split("\n").filter(Boolean).length} prompts in ${queue}`);
