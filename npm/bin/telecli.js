#!/usr/bin/env node
"use strict";

const { spawn } = require("child_process");
const path = require("path");
const { createRequire } = require("module");

const PLATFORMS = {
  "win32-x64": ["@qmahyar/telecli-win32-x64"],
  "linux-x64": [
    "@qmahyar/telecli-linux-x64-gnu",
    "@qmahyar/telecli-linux-x64-musl",
  ],
  "linux-arm64": [
    "@qmahyar/telecli-linux-arm64-gnu",
    "@qmahyar/telecli-linux-arm64-musl",
  ],
  "darwin-arm64": ["@qmahyar/telecli-darwin-arm64"],
  "darwin-x64": ["@qmahyar/telecli-darwin-x64"],
};

function binaryPath() {
  const key = `${process.platform}-${process.arch}`;
  const candidates = PLATFORMS[key];
  if (!candidates) {
    console.error(
      `[telecli] unsupported platform: ${key}. ` +
        "Supported: win32-x64, linux-x64, linux-arm64, darwin-arm64, darwin-x64. " +
        "Static musl builds for every Linux arch are on the GitHub Releases page."
    );
    process.exit(1);
  }
  const req = createRequire(__filename);
  for (const pkg of candidates) {
    try {
      const dir = path.dirname(req.resolve(`${pkg}/package.json`));
      return path.join(dir, "bin", `telecli${process.platform === "win32" ? ".exe" : ""}`);
    } catch {
      continue;
    }
  }
  console.error(
    `[telecli] no binary package installed for ${key} ` +
      `(tried: ${candidates.join(", ")}). ` +
      "Reinstall with: npm install -g @qmahyar/telecli"
  );
  process.exit(1);
}

const exe = binaryPath();
const child = spawn(exe, process.argv.slice(2), { stdio: "inherit" });

child.on("error", (err) => {
  console.error(`[telecli] failed to start ${exe}: ${err.message}`);
  console.error("[telecli] reinstall with: npm install -g @qmahyar/telecli");
  process.exit(1);
});

child.on("exit", (code) => {
  process.exit(code ?? 1);
});
