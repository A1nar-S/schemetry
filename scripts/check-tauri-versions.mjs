// Fails if a Tauri Rust crate and its npm package are on different major.minor
// versions — the same check `tauri build` runs, but without compiling anything.
// Compares the lockfiles, so it checks what CI will actually install.
import { readFileSync } from "node:fs";
import process from "node:process";

const cargoLock = readFileSync("Cargo.lock", "utf8");
const npmLock = JSON.parse(readFileSync("package-lock.json", "utf8"));

const crates = new Map();
for (const m of cargoLock.matchAll(/^name = "([^"]+)"\r?\nversion = "([^"]+)"/gm)) {
  crates.set(m[1], m[2]);
}

const npmVersion = (name) => npmLock.packages?.[`node_modules/${name}`]?.version;

// Each Rust crate paired with its npm package. Plugins are found automatically.
const pairs = [["tauri", "@tauri-apps/api"]];
for (const crate of crates.keys()) {
  const m = crate.match(/^tauri-plugin-(.+)$/);
  if (m) pairs.push([crate, `@tauri-apps/plugin-${m[1]}`]);
}

const majorMinor = (v) => v.split(".").slice(0, 2).join(".");
let failed = false;

for (const [crate, pkg] of pairs) {
  const rust = crates.get(crate);
  const js = npmVersion(pkg);
  if (!rust || !js) continue; // only one side is used (e.g. a Rust-only plugin)
  const ok = majorMinor(rust) === majorMinor(js);
  console.log(`${ok ? "ok  " : "FAIL"}  ${crate} ${rust}  <->  ${pkg} ${js}`);
  if (!ok) failed = true;
}

if (failed) {
  console.error(
    "\nTauri Rust crates and npm packages must share major.minor versions " +
      "(tauri build refuses otherwise). Update the npm package, or run " +
      "`cargo update -p <crate>` in the repo root.",
  );
  process.exit(1);
}
