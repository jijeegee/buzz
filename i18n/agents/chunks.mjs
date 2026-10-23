// node i18n/agents/chunks.mjs split <repo> <lang> <size>   -> i18n/work/chunks/<lang>.NN.json
// node i18n/agents/chunks.mjs merge <repo> <lang>          -> i18n/work/<lang>.out.json (from chunks/<lang>.NN.out.json)
// node i18n/agents/chunks.mjs status <repo> <lang>
import fs from "node:fs";
import path from "node:path";

const [cmd, repo, lang, sizeArg] = process.argv.slice(2);
const work = path.join(repo, "i18n", "work");
const dir = path.join(work, "chunks");
fs.mkdirSync(dir, { recursive: true });
const pad = (n) => String(n).padStart(2, "0");

if (cmd === "split") {
  const size = Number(sizeArg || 400);
  const src = JSON.parse(fs.readFileSync(path.join(work, `${lang}.json`), "utf8"));
  const { items, ...head } = src;
  let n = 0;
  for (let i = 0; i < items.length; i += size, n++) {
    const chunk = { ...head, items: items.slice(i, i + size) };
    fs.writeFileSync(path.join(dir, `${lang}.${pad(n)}.json`), JSON.stringify(chunk, null, 1));
  }
  console.log(`${lang}: ${items.length} items -> ${n} chunks`);
} else if (cmd === "merge" || cmd === "status") {
  const files = fs.readdirSync(dir).filter((f) => f.startsWith(`${lang}.`) && /^\S+\.\d\d\.json$/.test(f));
  const out = {};
  const missing = [];
  let expected = 0;
  for (const f of files.sort()) {
    const chunk = JSON.parse(fs.readFileSync(path.join(dir, f), "utf8"));
    expected += chunk.items.length;
    const outFile = path.join(dir, f.replace(/\.json$/, ".out.json"));
    if (!fs.existsSync(outFile)) { missing.push(f); continue; }
    const got = JSON.parse(fs.readFileSync(outFile, "utf8"));
    for (const item of chunk.items) {
      if (typeof got[item.id] === "string") out[item.id] = got[item.id];
    }
  }
  console.log(`${lang}: ${Object.keys(out).length}/${expected} translated; chunks missing: ${missing.join(" ") || "none"}`);
  if (cmd === "merge") {
    fs.writeFileSync(path.join(work, `${lang}.out.json`), JSON.stringify(out, null, 1));
  }
} else {
  console.error("usage: split|merge|status <repo> <lang> [size]");
  process.exit(1);
}
