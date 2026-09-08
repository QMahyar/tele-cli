#!/usr/bin/env node
"use strict";

const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");

// Legacy alias notice: `telecli` was the pre-0.10 package name. Both spellings
// run the same binary; the alias will be removed in a future major release.
const invokedAs = path.basename(process.argv[1] || "", ".exe");
if (invokedAs === "telecli") {
  console.error("[telecli] deprecated alias of `tele`; will be removed in a future major release.");
}

pickExeAndSpawn();

function isMusl() {
  // process.report carries the runtime libc identity (node >= 12.17 on Linux);
  // ldd --version is only consulted when the report is unavailable.
  try {
    if (typeof process.report?.getReport === "function") {
      const header = process.report.getReport().header || {};
      if (header.glibcVersionRuntime) return false;
      if (process.report.getReport().header.osName) {
        // report exists but carries no glibc marker: musl builds simply lack it
        return true;
      }
    }
  } catch {
    // fall through to ldd
  }
  try {
    const out = require("child_process").execSync("ldd --version 2>&1 || true", {
      encoding: "utf8",
    });
    return out.includes("musl");
  } catch {
    return false;
  }
}

// Spawn the first existing candidate; on spawn failure (broken/missing binary
// despite the existsSync probe) fall through to the remaining candidates
// instead of dying on the first miss.
function pickExeAndSpawn() {
  const platform = process.platform;
  const arch = process.arch;
  const ext = platform === "win32" ? ".exe" : "";
  const prefixes = ["tele-", "telecli-"];
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

  for (const prefix of prefixes) {
    for (const triple of candidates) {
      const p = path.join(__dirname, `${prefix}${triple}${ext}`);
      if (!fs.existsSync(p)) continue;
      try {
        const child = spawn(p, process.argv.slice(2), { stdio: "inherit" });
        child.on("error", (err) => {
          console.error(`[tele] failed to start ${p}: ${err.message}`);
          process.exit(1);
        });
        child.on("exit", (code) => {
          process.exit(code ?? 1);
        });
        return;
      } catch (err) {
        console.error(`[tele] spawn failed for ${p}: ${err.message}; trying next candidate`);
      }
    }
  }
  console.error(
    `[tele] no runnable binary bundled for ${platform}-${arch}` +
      (candidates.length ? ` (tried: ${candidates.join(", ")})` : "") +
      ". Download from https://github.com/QMahyar/tele-cli/releases"
  );
  process.exit(1);
}
