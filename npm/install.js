#!/usr/bin/env node

"use strict";

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const https = require("https");
const http = require("http");

const PLATFORM_PACKAGES = {
  "win32-x64": "@qmahyar/tele-cli-win32-x64",
  "darwin-arm64": "@qmahyar/tele-cli-darwin-arm64",
  "darwin-x64": "@qmahyar/tele-cli-darwin-x64",
  "linux-x64": "@qmahyar/tele-cli-linux-x64",
  "linux-arm64": "@qmahyar/tele-cli-linux-arm64",
};

const BINARY_NAMES = {
  win32: "tele.exe",
  darwin: "tele",
  linux: "tele",
};

const GITHUB_REPO = "QMahyar/tele-cli";

function getPlatformKey() {
  return `${process.platform}-${process.arch}`;
}

function getBinaryName() {
  return BINARY_NAMES[process.platform] || "tele";
}

function getPackageDir() {
  const platformKey = getPlatformKey();
  const pkgName = PLATFORM_PACKAGES[platformKey];
  if (!pkgName) {
    return null;
  }
  try {
    return path.dirname(require.resolve(`${pkgName}/package.json`));
  } catch {
    return null;
  }
}

function getBinPath() {
  const pkgDir = getPackageDir();
  if (pkgDir) {
    const binName = getBinaryName();
    return path.join(pkgDir, binName);
  }
  return null;
}

function getReleaseUrl() {
  const binName = getBinaryName();
  const ext = process.platform === "win32" ? ".zip" : ".tar.gz";
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";

  let target;
  switch (process.platform) {
    case "win32":
      target = `${arch}-pc-windows-msvc`;
      break;
    case "darwin":
      target = `${arch}-apple-darwin`;
      break;
    case "linux":
      target = `${arch}-unknown-linux-gnu`;
      break;
    default:
      return null;
  }

  return `https://github.com/${GITHUB_REPO}/releases/latest/download/tele-${target}${ext}`;
}

function download(url) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith("https") ? https : http;
    client
      .get(url, { headers: { "User-Agent": "tele-cli-installer" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return download(res.headers.location).then(resolve, reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", reject);
  });
}

async function installBinary() {
  const url = getReleaseUrl();
  if (!url) {
    console.warn(`[tele-cli] No prebuilt binary available for ${getPlatformKey()}.`);
    console.warn(`[tele-cli] Install from source: cargo install --locked telecli`);
    return;
  }

  const binDir = path.join(__dirname, "bin");
  const binName = getBinaryName();
  const binPath = path.join(binDir, binName);

  fs.mkdirSync(binDir, { recursive: true });

  console.log(`[tele-cli] Downloading tele for ${process.platform}-${process.arch}...`);

  try {
    const data = await download(url);
    fs.writeFileSync(binPath, data);
    fs.chmodSync(binPath, 0o755);
    console.log(`[tele-cli] Installed to ${binPath}`);
  } catch (err) {
    console.warn(`[tele-cli] Failed to download binary: ${err.message}`);
    console.warn(`[tele-cli] Install manually from https://github.com/${GITHUB_REPO}/releases`);
    console.warn(`[tele-cli] Or install from source: cargo install --locked telecli`);
  }
}

// Check if platform-specific package was installed via optionalDependencies
const existingBin = getBinPath();
if (existingBin && fs.existsSync(existingBin)) {
  // Binary already available from platform package, create symlink in bin/
  const binDir = path.join(__dirname, "bin");
  const binName = getBinaryName();
  const targetPath = path.join(binDir, binName);

  fs.mkdirSync(binDir, { recursive: true });

  if (!fs.existsSync(targetPath)) {
    try {
      fs.copyFileSync(existingBin, targetPath);
      fs.chmodSync(targetPath, 0o755);
      console.log(`[tele-cli] Linked binary from platform package`);
    } catch {
      // Fallback to download
      installBinary();
    }
  }
} else {
  // No platform package found, download from GitHub releases
  installBinary();
}
