#!/usr/bin/env node
"use strict";
/*
 * Arbiter Node runner runtime (Layer B).
 *
 * Vendored, dependency-free. The worker writes this file once (reused across
 * runs) and invokes it per run with the handshake on argv; it requires the
 * user's module, runs the entrypoint, marshals the return value, captures
 * errors, and writes a result document. User code (Layer C) only implements
 * `run(ctx)` (and optionally `prepare(ctx)`) and never touches the wire format
 * -- swapping the transport (file -> socket) is a Layer B/A change.
 *
 * Handshake (argv): --module M --entry E (default "run") --result-file PATH
 * --run-id ID --transport file --protocol N. The process env is reserved for the
 * user's own variables (e.g. NODE_PATH); arbiter does not inject control vars.
 */

const fs = require("fs");

const PROTOCOL_VERSION = 1;

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; ) {
    const token = argv[i];
    if (token.startsWith("--")) {
      args[token.slice(2)] = i + 1 < argv.length ? argv[i + 1] : "";
      i += 2;
    } else {
      i += 1;
    }
  }
  return args;
}

function makeContext(args) {
  // Structured logs go to stderr (captured by the worker as run logs).
  const emit = (level, msg) => process.stderr.write(`[${level}] ${msg}\n`);
  return {
    log: {
      debug: (m) => emit("debug", m),
      info: (m) => emit("info", m),
      warning: (m) => emit("warning", m),
      error: (m) => emit("error", m),
    },
    state: {},
    params: {},
    runId: args["run-id"],
    jobId: args["job-id"],
    // v1: no event stream consumed yet; surface progress as a log line.
    progress: (pct) => emit("info", `progress ${pct}`),
  };
}

function structuredError(err) {
  if (err instanceof Error) {
    return {
      type: err.name || "Error",
      message: err.message || String(err),
      stack: err.stack ? String(err.stack).split("\n") : [],
    };
  }
  return { type: "Error", message: String(err), stack: [] };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const resultFile = args["result-file"];
  const moduleName = args["module"];
  const entry = args["entry"] || "run";

  const ctx = makeContext(args);
  let status = "success";
  let output = null;
  let error = null;

  try {
    if (!moduleName) {
      throw new Error("--module is not set");
    }
    const mod = require(moduleName);
    if (typeof mod.prepare === "function") {
      await mod.prepare(ctx);
    }
    const fn = mod[entry];
    if (typeof fn !== "function") {
      throw new TypeError(`entrypoint '${entry}' is not a function`);
    }
    const ret = await fn(ctx);
    output = ret === undefined ? null : ret;
  } catch (err) {
    status = "failed";
    error = structuredError(err);
  }

  const doc = {
    protocolVersion: PROTOCOL_VERSION,
    status,
    output,
    error,
  };
  if (resultFile) {
    fs.writeFileSync(resultFile, JSON.stringify(doc));
  }

  process.exit(status === "success" ? 0 : 1);
}

main();
