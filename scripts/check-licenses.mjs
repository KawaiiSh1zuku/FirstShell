import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const allowedIdentifiers = new Set([
  "0BSD",
  "AGPL-3.0-only",
  "Apache-2.0",
  "BSD-1-Clause",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BlueOak-1.0.0",
  "CC0-1.0",
  "ISC",
  "LGPL-2.1-or-later",
  "LLVM-exception",
  "MIT",
  "MIT-0",
  "MPL-2.0",
  "Unicode-3.0",
  "Unlicense",
  "Zlib",
]);
const operators = new Set(["AND", "OR", "WITH"]);

function validateLicense(owner, expression) {
  if (!expression) throw new Error(`${owner}: missing license expression`);
  const identifiers = expression.match(/[A-Za-z0-9][A-Za-z0-9.-]*/g) ?? [];
  const unknown = identifiers.filter(
    (identifier) => !operators.has(identifier) && !allowedIdentifiers.has(identifier),
  );
  if (unknown.length > 0) {
    throw new Error(`${owner}: unapproved license identifier(s): ${unknown.join(", ")}`);
  }
}

const cargo = spawnSync(
  process.platform === "win32" ? "cargo.exe" : "cargo",
  ["metadata", "--format-version", "1", "--locked"],
  { cwd: repositoryRoot, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
);
if (cargo.status !== 0) {
  throw new Error(`cargo metadata failed: ${cargo.error ?? cargo.stderr}`);
}
const cargoMetadata = JSON.parse(cargo.stdout);
for (const packageInfo of cargoMetadata.packages) {
  validateLicense(`cargo:${packageInfo.name}@${packageInfo.version}`, packageInfo.license);
}

const npmLock = JSON.parse(
  readFileSync(resolve(repositoryRoot, "apps/desktop/package-lock.json"), "utf8"),
);
for (const [path, packageInfo] of Object.entries(npmLock.packages)) {
  if (path === "") continue;
  validateLicense(`npm:${path}`, packageInfo.license);
}

console.log(
  `license check passed: ${cargoMetadata.packages.length} Cargo packages, ${Object.keys(npmLock.packages).length - 1} npm packages`,
);
