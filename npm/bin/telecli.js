#!/usr/bin/env node
"use strict";

const { spawn } = require("child_process");
const path = require("path");

const exe = path.join(__dirname, "telecli.exe");
const child = spawn(exe, process.argv.slice(2), { stdio: "inherit" });

child.on("error", (err) => {
  console.error(`[telecli] failed to start ${exe}: ${err.message}`);
  console.error("[telecli] reinstall with: npm install -g telecli");
  process.exit(1);
});

child.on("exit", (code) => {
  process.exit(code ?? 1);
});
