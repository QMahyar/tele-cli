#!/usr/bin/env node
"use strict";

const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");

const exe = pickExe();
const child = spawn(exe, process.argv.slice(2), { stdio: "inherit" });

child.on("error", (err) => {
  console.error(`[telecli] failed to start ${exe}: ${err.message}`);
  process.exit(1);
});

child.on("exit", (code) => {
  process.exit(code ?? 1);
});

function pickExe() {
  const platform = process.platform;
  const arch = process.arch;
  const ext = platform === "win32" ? ".exe" : "";
  // Bundled binaries are named telecli-<target-triple>[.exe]
  const candidates = [];
  if (platform === "win32" && arch === "x64") candidates.push("x86_64-pc-windows-msvc");
  if (platform === "win32" && arch === "arm64") candidates.push("aarch64-pc-windows-msvc");
  if (platform === "darwin" && arch === "arm64") candidates.push("aarch64-apple-darwin");
  if (platform === "darwin" && arch === "x64") candidates.push("x86_64-apple-darwin");
  if (platform === "linux" && arch === "x64") {
    if (isMusl()) candidates.push("x86_64-unknown-linux-musl");
    candidates.push("x86_64-unknown-linux-gnu");
  }
  if (platform === "linux" && arch === "arm64") {
    if (isMusl()) candidates.push("aarch64-unknown-linux-musl");
    candidates.push("aarch64-unknown-linux-gnu");
  }
  if (platform === "linux" && arch === "arm") {
    if (isMusl()) candidates.push("armv7-unknown-linux-musleabihf");
    candidates.push("armv7-unknown-linux-gnueabihf");
  }
  if (platform === "linux" && arch === "ia32") candidates.push("i686-unknown-linux-musl");
  if (platform === "linux" && arch === "ppc64") candidates.push("powerpc64le-unknown-linux-gnu");
  if (platform === "linux" && arch === "riscv64") candidates.push("riscv64gc-unknown-linux-gnu");

  for (const triple of candidates) {
    const p = path.join(__dirname, `telecli-${triple}${ext}`);
    if (fs.existsSync(p)) return p;
  }
  console.error(
    `[telecli] no binary bundled for ${platform}-${arch}` +
      (candidates.length ? ` (tried: ${candidates.join(", ")})` : "") +
      ". Download from https://github.com/QMahyar/tele-cli/releases"
  );
  process.exit(1);
}

function isMusl() {
  try {
    const out = require("child_process").execSync("ldd --version 2>&1 || true", { encoding: "utf8" });
    return out.includes("musl");
  } catch {
    return false;
  }
}
