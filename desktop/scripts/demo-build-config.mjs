import { randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const PRODUCTION_IDENTIFIER = "app.kuzz.app";
// The build ID suffix is 17 characters including its separator, and the Rust
// build contract caps the complete demo slug at 48 ASCII bytes.
const MAX_DEMO_SLUG_LENGTH = 48;
const DEMO_BUILD_ID_SUFFIX_LENGTH = 17;
const MAX_DEMO_NAME_LENGTH = MAX_DEMO_SLUG_LENGTH - DEMO_BUILD_ID_SUFFIX_LENGTH;

export const productionBuildIdentity = Object.freeze({
  productName: "Kuzz",
  identifier: PRODUCTION_IDENTIFIER,
  deepLinkScheme: "kuzz",
  keyringService: "kuzz-desktop",
  nestName: ".kuzz",
  cliName: "kuzz",
});

export function demoBuildConfig(
  rawName,
  buildId = randomBytes(8).toString("hex"),
) {
  if (typeof rawName !== "string") throw new Error("Demo name must be text");
  const name = rawName.trim().replace(/\s+/g, " ");
  if (!name) throw new Error("Demo name must not be empty");
  if (name.length > MAX_DEMO_NAME_LENGTH) {
    throw new Error(
      `Demo name must be at most ${MAX_DEMO_NAME_LENGTH} characters`,
    );
  }
  if (!/^[A-Za-z0-9][A-Za-z0-9 -]*$/.test(name)) {
    throw new Error(
      "Demo name may contain ASCII letters, numbers, spaces, and hyphens only",
    );
  }

  if (!/^[a-f0-9]{16}$/.test(buildId)) {
    throw new Error(
      "Demo build ID must be sixteen lowercase hexadecimal characters",
    );
  }

  const readableSlug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  const slug = `${readableSlug}-${buildId}`;
  const productName = `Kuzz ${name}`;
  return {
    name,
    slug,
    productName,
    dmgVolumeName: productName,
    dmgFileStem: productName.replace(/ /g, "_"),
    identifier: `${PRODUCTION_IDENTIFIER}.demo.${slug}`,
    appDataIdentity: `${PRODUCTION_IDENTIFIER}.demo.${slug}`,
    deepLinkScheme: `kuzz-demo-${slug}`,
    keyringService: `kuzz-desktop-demo.${slug}`,
    nestName: `.kuzz-demo-${slug}`,
    cliName: `kuzz-demo-${slug}`,
    tauriConfig: {
      productName,
      identifier: `${PRODUCTION_IDENTIFIER}.demo.${slug}`,
      plugins: { "deep-link": { desktop: { schemes: [`kuzz-demo-${slug}`] } } },
      bundle: { targets: ["app"] },
    },
  };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const [name, outputPath, buildId] = process.argv.slice(2);
  if (!outputPath) {
    console.error(
      "Usage: demo-build-config.mjs <demo-name> <output-config-path>",
    );
    process.exit(2);
  }
  try {
    const config = demoBuildConfig(name, buildId);
    writeFileSync(
      outputPath,
      `${JSON.stringify(config.tauriConfig, null, 2)}\n`,
    );
    console.log(JSON.stringify(config));
  } catch (error) {
    console.error(`Invalid demo build: ${error.message}`);
    process.exit(1);
  }
}
