import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

const desktopRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const configDir = path.join(desktopRoot, "src-tauri");
const wrapper = path.join(desktopRoot, "scripts/tauri-command.mjs");
const fakeCli = path.join(tmpdir(), `kuzz-fake-tauri-${process.pid}.mjs`);

writeFileSync(
  fakeCli,
  `import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
const args = process.argv.slice(2);
const configIndex = args.lastIndexOf("--config");
const override = JSON.parse(args[configIndex + 1]);
const configured = override.build.frontendDist;
// Tauri resolves frontendDist against the directory holding tauri.conf.json
// (config_parent.join(path) in tauri-codegen), not against the process cwd.
const output = path.resolve(process.env.KUZZ_TEST_CONFIG_DIR, configured);
// Write through the producer path the wrapper publishes and read back through
// the config-resolved consumer path. Doing both against one path would make the
// fake agree with itself no matter where the wrapper pointed frontendDist.
const producer = process.env.KUZZ_PROTECTED_BUILD_OUTPUT;
mkdirSync(producer, { recursive: true });
writeFileSync(path.join(producer, "variant.txt"), process.env.VITE_KUZZ_BESTIE);
await new Promise((resolve) => setTimeout(resolve, 100));
const observed = readFileSync(path.join(output, "variant.txt"), "utf8");
writeFileSync(
  process.env.KUZZ_TEST_RESULT,
  JSON.stringify({ args, configured, output, observed }),
);
`,
);

function packageVariant(variant, result, runnerArguments = []) {
  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [wrapper, "build", ...runnerArguments],
      {
        cwd: desktopRoot,
        env: {
          ...process.env,
          KUZZ_TAURI_CLI_ENTRYPOINT: fakeCli,
          KUZZ_TEST_CONFIG_DIR: configDir,
          KUZZ_TEST_RESULT: result,
          VITE_KUZZ_BESTIE: variant,
        },
        stdio: "inherit",
      },
    );
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0 ? resolve() : reject(new Error(`wrapper exited ${code}`)),
    );
  });
}

test("opposite Tauri package variants own private frontend artifacts", async () => {
  const resultRoot = path.join(tmpdir(), `kuzz-tauri-results-${process.pid}`);
  mkdirSync(resultRoot, { recursive: true });
  const ossResult = path.join(resultRoot, "oss.json");
  const internalResult = path.join(resultRoot, "internal.json");

  await Promise.all([
    packageVariant("0", ossResult),
    packageVariant("1", internalResult),
  ]);

  const oss = JSON.parse(readFileSync(ossResult, "utf8"));
  const internal = JSON.parse(readFileSync(internalResult, "utf8"));
  assert.equal(oss.observed, "0");
  assert.equal(internal.observed, "1");
  assert.notEqual(oss.output, internal.output);
});

test("private config precedes Cargo runner arguments", async () => {
  const result = path.join(
    tmpdir(),
    `kuzz-tauri-runner-arguments-${process.pid}.json`,
  );
  await packageVariant("0", result, [
    "--config",
    '{"bundle":{"active":false}}',
    "--",
    "--locked",
  ]);

  const invocation = JSON.parse(readFileSync(result, "utf8"));
  const delimiterIndex = invocation.args.indexOf("--");
  const privateConfigIndex = invocation.args.lastIndexOf("--config");
  assert.ok(privateConfigIndex < delimiterIndex);
  assert.equal(invocation.args[delimiterIndex + 1], "--locked");
  assert.equal(
    JSON.parse(invocation.args[privateConfigIndex + 1]).build.frontendDist,
    invocation.configured,
  );
});

test("private frontendDist is never mistaken for a URL", async () => {
  const result = path.join(
    tmpdir(),
    `kuzz-tauri-frontend-dist-${process.pid}.json`,
  );
  await packageVariant("0", result);
  const invocation = JSON.parse(readFileSync(result, "utf8"));

  // `FrontendDist` is an untagged enum whose first variant is `Url(Url)`, and
  // tauri-codegen embeds *no assets without erroring* for that variant. A
  // Windows absolute path parses as a URL -- `C:` becomes the scheme -- so an
  // absolute frontendDist produces a UI-less app that still exits 0.
  assert.ok(
    !path.isAbsolute(invocation.configured),
    `frontendDist must stay relative, got ${invocation.configured}`,
  );
  // Rust's `url` crate and Node's `URL` both implement the WHATWG standard, so
  // this is the same parse serde performs. It only rejects absolute paths on
  // Windows, which is why the assertion above carries the check on Linux/macOS.
  assert.throws(() => new URL(invocation.configured));
  // The relative path still has to reach the directory the wrapper published.
  assert.equal(invocation.observed, "0");
});
