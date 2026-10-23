// node i18n/agents/pop.mjs <queue.txt> [n]   prints and removes the first n prompt paths
// node i18n/agents/pop.mjs <queue.txt> --back <path>   puts a path back at the front
import fs from "node:fs";

const [file, arg, back] = process.argv.slice(2);
const lines = fs.existsSync(file) ? fs.readFileSync(file, "utf8").split("\n").filter(Boolean) : [];
if (arg === "--back") {
  fs.writeFileSync(file, [back, ...lines].map((line) => `${line}\n`).join(""));
} else {
  const n = Number(arg ?? 1);
  console.log(lines.slice(0, n).join("\n"));
  fs.writeFileSync(file, lines.slice(n).map((line) => `${line}\n`).join(""));
}
