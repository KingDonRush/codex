#!/usr/bin/env node

import { mkdirSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const claudexHome =
  nonEmptyEnv("CLAUDEX_HOME") ?? path.join(os.homedir(), ".claudex");
mkdirSync(claudexHome, { recursive: true, mode: 0o700 });

process.env.CODEX_HOME = claudexHome;
process.env.CLAUDEX = "1";

await import("./codex.js");

function nonEmptyEnv(name) {
  const value = process.env[name];
  return value && value.trim() ? value : undefined;
}
