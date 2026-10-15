#!/usr/bin/env node
/**
 * Cursor SDK sidecar for FinallyAValueBot's Cursor agent engine.
 *
 * Runs @cursor/sdk in-process (no cursor-sdk-bridge.js). Bot tools stay on
 * loopback MCP (mcp_servers on every send/resume).
 *
 * Environment:
 *   CURSOR_API_KEY
 *   CURSOR_RUN_CONCURRENCY          default 4
 *   CURSOR_BRIDGE_IDLE_TTL_SECS     idle agent-handle TTL, default 600
 *   CURSOR_BRIDGE_POOL_MAX          max warm agent handles, default 16
 *   CURSOR_SIDECAR_MAX_UPTIME_SECS  idle self-recycle, default 86400
 *   CURSOR_RUN_WAIT_TIMEOUT_MS      hang watchdog for run.wait() AFTER stream ends, default 120000 (not a turn budget)
 *   CURSOR_STREAM_IDLE_TIMEOUT_MS   cancel SDK stream when no events for this long, default 900000 (15 min)
 *   CURSOR_MAX_RUN_WALL_SECS        cancel only /run sessions older than this; keep younger ones, default 1200
 *   CURSOR_RUN_INTERACTIVE_RESERVE  slots reserved for interactive chat (scheduled waits), default 1
 *   CURSOR_SDK_NODE_PREFIX          runtime dir with sdk-shim.mjs + node_modules
 *
 * Every /run is a new Cursor session: Agent.create only (never Agent.resume).
 * JsonlLocalAgentStore lives under {runtime}/cursor-sdk-ephemeral/{runId} and
 * is deleted when the HTTP response is finished so no checkpoint dir dangles.
 * Gateway already sends full persona + chat context each turn.
 * Node 18 file ESM has no global `crypto`; polyfill Web Crypto before loading
 * `@cursor/sdk` (it calls `crypto.randomUUID()` for agent/run ids).
 * Sends pass `local.force` so a leftover run cannot block the next message.
 *
 * API: GET /health  GET /models  POST /run
 *      POST /admin/request_recycle  POST /admin/cancel_old_runs
 */

import crypto, { webcrypto } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// Must run before dynamic import("@cursor/sdk"). Node 18 enables global
// Web Crypto for `node -e` but not for `.mjs` files unless this is set.
if (typeof globalThis.crypto === "undefined" && webcrypto) {
  Object.defineProperty(globalThis, "crypto", {
    value: webcrypto,
    configurable: true,
    enumerable: true,
  });
}
if (
  globalThis.crypto &&
  typeof globalThis.crypto.randomUUID !== "function" &&
  typeof crypto.randomUUID === "function"
) {
  try {
    Object.defineProperty(globalThis.crypto, "randomUUID", {
      value: () => crypto.randomUUID(),
      configurable: true,
    });
  } catch {
    // Web Crypto object may be frozen on some Node builds.
  }
}

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));

const DEFAULT_PORT = 3848;
const DEFAULT_MODEL = "composer-2.5";
const DEFAULT_RUN_CONCURRENCY = 4;
const DEFAULT_AGENT_IDLE_TTL_SECS = 600;
const DEFAULT_AGENT_POOL_MAX = 16;
const DEFAULT_SIDECAR_MAX_UPTIME_SECS = 86400;
const DEFAULT_RUN_WAIT_TIMEOUT_MS = 120_000;
const DEFAULT_STREAM_IDLE_TIMEOUT_MS = 900_000;
const DEFAULT_MAX_RUN_WALL_SECS = 1200;
const DEFAULT_RUN_INTERACTIVE_RESERVE = 1;
const DEFAULT_REAPER_INTERVAL_SECS = 60;
const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const STARTED_AT_MONOTONIC = process.hrtime.bigint();
const STARTED_AT_UNIX = Date.now() / 1000;

let sdkModule = null;
/** @type {Map<string, { runKind: string, startedMonotonic: number, lastEventMonotonic: number, cancelFn: (() => Promise<void>)|null }>} */
const activeRuns = new Map();
let nextRunSeq = 1;
let recycleRequested = false;
let shutdownTriggered = false;
let reaperTimer = null;

/** @type {Map<string, PooledAgent>} */
const POOL = new Map();

class Mutex {
  constructor() {
    this._tail = Promise.resolve();
  }

  run(fn) {
    const run = this._tail.then(() => fn());
    this._tail = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }
}

/**
 * @typedef {object} PooledAgent
 * @property {string} poolKey
 * @property {string} cwd
 * @property {string} sessionScope
 * @property {any} agent
 * @property {Mutex} lock
 * @property {number} lastUsedMonotonic
 * @property {number} inUse
 */

function envInt(name, fallback, min) {
  const raw = (process.env[name] || "").trim();
  if (!raw) return fallback;
  const n = Number.parseInt(raw, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.max(min, n);
}

function runConcurrency() {
  return envInt("CURSOR_RUN_CONCURRENCY", DEFAULT_RUN_CONCURRENCY, 1);
}

function agentIdleTtlSecs() {
  return envInt("CURSOR_BRIDGE_IDLE_TTL_SECS", DEFAULT_AGENT_IDLE_TTL_SECS, 60);
}

function agentPoolMax() {
  return envInt("CURSOR_BRIDGE_POOL_MAX", DEFAULT_AGENT_POOL_MAX, 4);
}

function sidecarMaxUptimeSecs() {
  return envInt(
    "CURSOR_SIDECAR_MAX_UPTIME_SECS",
    DEFAULT_SIDECAR_MAX_UPTIME_SECS,
    60,
  );
}

function runWaitTimeoutMs() {
  return envInt("CURSOR_RUN_WAIT_TIMEOUT_MS", DEFAULT_RUN_WAIT_TIMEOUT_MS, 3000);
}

function streamIdleTimeoutMs() {
  return envInt(
    "CURSOR_STREAM_IDLE_TIMEOUT_MS",
    DEFAULT_STREAM_IDLE_TIMEOUT_MS,
    60_000,
  );
}

function maxRunWallAgeSecs() {
  return envInt("CURSOR_MAX_RUN_WALL_SECS", DEFAULT_MAX_RUN_WALL_SECS, 60);
}

function interactiveReserve() {
  return envInt(
    "CURSOR_RUN_INTERACTIVE_RESERVE",
    DEFAULT_RUN_INTERACTIVE_RESERVE,
    0,
  );
}

function monotonicNow() {
  return Number(process.hrtime.bigint()) / 1e9;
}

function runsInFlightCount() {
  return activeRuns.size;
}

function oldestRunAgeSecs() {
  const now = monotonicNow();
  let oldest = 0;
  for (const rec of activeRuns.values()) {
    const age = now - rec.startedMonotonic;
    if (age > oldest) oldest = age;
  }
  return Math.floor(oldest);
}

function beginTrackedRun(runKind) {
  const id = String(nextRunSeq++);
  const now = monotonicNow();
  activeRuns.set(id, {
    runKind,
    startedMonotonic: now,
    lastEventMonotonic: now,
    cancelFn: null,
  });
  return id;
}

function touchRunActivity(runId) {
  const rec = activeRuns.get(runId);
  if (rec) rec.lastEventMonotonic = monotonicNow();
}

function endTrackedRun(runId) {
  if (runId) activeRuns.delete(runId);
}

function isScheduledRunBody(body) {
  const scope = String(body?.session_scope || "").trim();
  if (scope.startsWith("scheduled:")) return true;
  if (scope.startsWith("background:") || scope.startsWith("bg:")) return true;
  return body?.run_kind === "scheduled";
}

function uptimeSecs() {
  return Number(process.hrtime.bigint() - STARTED_AT_MONOTONIC) / 1e9;
}

function apiKey() {
  return (process.env.CURSOR_API_KEY || "").trim();
}

function isEphemeralSessionScope(sessionScope) {
  const scope = (sessionScope || "").trim();
  if (!scope) return false;
  if (scope.startsWith("scheduled:")) return true;
  if (scope.startsWith("background:") || scope.startsWith("bg:")) return true;
  return UUID_RE.test(scope);
}

function runtimeDataDir() {
  const runtime = (process.env.FINALLY_A_VALUE_BOT_RUNTIME_DATA || "").trim();
  return runtime || path.join(SCRIPT_DIR, "..", "workspace", "runtime");
}

function ephemeralStoreDir(runId) {
  const safe = String(runId || crypto.randomUUID()).replace(
    /[^a-zA-Z0-9._-]/g,
    "_",
  );
  return path.join(runtimeDataDir(), "cursor-sdk-ephemeral", safe);
}

function createEphemeralStore(sdk, storeDir) {
  const Store = sdk?.JsonlLocalAgentStore;
  if (typeof Store !== "function") {
    throw new Error(
      "JsonlLocalAgentStore is not exported from @cursor/sdk; upgrade the package",
    );
  }
  fs.mkdirSync(storeDir, { recursive: true });
  return new Store(storeDir);
}

function removeStoreDir(storeDir) {
  if (!storeDir) return;
  try {
    fs.rmSync(storeDir, { recursive: true, force: true });
  } catch (err) {
    logErr(`ephemeral store cleanup failed (${storeDir}): ${err}`);
  }
}

function pruneLegacyResumeStores() {
  const runtime = runtimeDataDir();
  for (const name of ["cursor-sdk-state", "cursor-sdk-ephemeral"]) {
    const dir = path.join(runtime, name);
    try {
      if (fs.existsSync(dir)) {
        fs.rmSync(dir, { recursive: true, force: true });
        logErr(`pruned ${dir}`);
      }
    } catch (err) {
      logErr(`failed to prune ${dir}: ${err}`);
    }
  }
}

function poolKey(cwd, sessionScope) {
  let resolved = cwd;
  try {
    resolved = fs.realpathSync(cwd);
  } catch {
    resolved = path.resolve(cwd);
  }
  const persona = crypto
    .createHash("sha256")
    .update(resolved)
    .digest("hex")
    .slice(0, 16);
  const scope = (sessionScope || "").trim();
  const session = scope
    ? crypto.createHash("sha256").update(scope).digest("hex").slice(0, 16)
    : "main";
  return `${persona}:${session}`;
}

function clientIsLoopback(req) {
  const addr = req.socket?.remoteAddress || "";
  return (
    addr === "127.0.0.1" ||
    addr === "::1" ||
    addr === "::ffff:127.0.0.1" ||
    addr === "localhost"
  );
}

function logErr(message) {
  process.stderr.write(`[cursor-sdk-runner] ${message}\n`);
}

async function loadSdk() {
  if (sdkModule) return sdkModule;
  const prefix = (process.env.CURSOR_SDK_NODE_PREFIX || "").trim();
  if (prefix) {
    const shim = path.join(prefix, "sdk-shim.mjs");
    sdkModule = await import(pathToFileURL(shim).href);
  } else {
    sdkModule = await import("@cursor/sdk");
  }
  logErr(
    `loaded @cursor/sdk; local store=JsonlLocalAgentStore (node ${process.versions.node}; webcrypto=${typeof globalThis.crypto?.randomUUID})`,
  );
  return sdkModule;
}

function sdkInstalledSync() {
  const prefix = (process.env.CURSOR_SDK_NODE_PREFIX || "").trim();
  if (prefix) {
    return fs.existsSync(path.join(prefix, "sdk-shim.mjs"));
  }
  try {
    import.meta.resolve("@cursor/sdk");
    return true;
  } catch {
    return false;
  }
}

function isBusyAgentError(err) {
  const name = String(err?.name || err?.constructor?.name || "");
  if (name.includes("AgentBusy")) return true;
  const msg = String(err?.message || err).toLowerCase();
  return (
    msg.includes("already has active run") ||
    msg.includes("agent is busy") ||
    msg.includes("agent_busy")
  );
}

function buildSendOptions(mcpServers) {
  const options = { local: { force: true } };
  if (mcpServers) options.mcpServers = mcpServers;
  return options;
}

function isRetryableSdkError(err) {
  if (err?.isRetryable === true || err?.is_retryable === true) return true;
  const msg = String(err?.message || err).toLowerCase();
  return (
    msg.includes("timed out") ||
    msg.includes("econnreset") ||
    msg.includes("econnrefused") ||
    msg.includes("socket hang up") ||
    msg.includes("fetch failed")
  );
}

async function disposeAgent(agent) {
  if (!agent) return;
  try {
    if (typeof agent[Symbol.asyncDispose] === "function") {
      await agent[Symbol.asyncDispose]();
    } else if (typeof agent.close === "function") {
      await agent.close();
    }
  } catch (err) {
    logErr(`agent dispose failed: ${err}`);
  }
}

async function cancelRun(run) {
  if (!run) return;
  try {
    if (typeof run.supports === "function" && !run.supports("cancel")) return;
    if (typeof run.cancel === "function") await run.cancel();
  } catch {
    // ignore
  }
}

/**
 * Hang watchdog for run.wait() after the stream has already ended.
 * Tool rounds and long replies live in the stream loop and are not timed here.
 * A hung wait() would otherwise pin runsInFlight and block idle recycle.
 */
async function waitForRunResult(run, timeoutMs) {
  if (!run || typeof run.wait !== "function") {
    return { status: "finished", result: "", timedOut: false };
  }
  let timer;
  const waitP = Promise.resolve()
    .then(() => run.wait())
    .then((value) => ({ timedOut: false, value }))
    .catch((err) => {
      logErr(`run.wait rejected: ${err}`);
      return { timedOut: false, value: { status: "finished", result: "" } };
    });
  try {
    const raced = await Promise.race([
      waitP,
      new Promise((resolve) => {
        timer = setTimeout(() => {
          resolve({ timedOut: true, value: null });
        }, timeoutMs);
        if (typeof timer.unref === "function") timer.unref();
      }),
    ]);
    if (raced.timedOut) {
      logErr(`run.wait timed out after ${timeoutMs}ms; cancelling`);
      await cancelRun(run);
      return { status: "finished", result: "", timedOut: true };
    }
    const value = raced.value || {};
    return {
      status: value.status || "finished",
      result: typeof value.result === "string" ? value.result : "",
      timedOut: false,
    };
  } finally {
    if (timer) clearTimeout(timer);
  }
}

function stringifyUnknown(value) {
  if (value == null) return "";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function serializeModelEntry(model) {
  const modelId = model?.id || model?.model;
  if (!modelId) return null;
  const entry = { id: String(modelId) };
  const displayName = model.displayName || model.display_name;
  if (displayName) entry.display_name = String(displayName);

  const parameters = [];
  for (const param of model.parameters || []) {
    const paramId = param?.id;
    if (!paramId) continue;
    const values = [];
    for (const value of param.values || []) {
      const raw = value?.value;
      if (raw == null) continue;
      const valueEntry = { value: String(raw) };
      const label = value.displayName || value.display_name;
      if (label) valueEntry.display_name = String(label);
      values.push(valueEntry);
    }
    if (!values.length) continue;
    const paramEntry = { id: String(paramId), values };
    const paramLabel = param.displayName || param.display_name;
    if (paramLabel) paramEntry.display_name = String(paramLabel);
    parameters.push(paramEntry);
  }
  if (parameters.length) entry.parameters = parameters;

  const variants = [];
  for (const variant of model.variants || []) {
    const variantName = variant?.displayName || variant?.display_name;
    if (!variantName) continue;
    const variantParams = [];
    for (const p of variant.params || []) {
      if (p?.id && p?.value != null) {
        variantParams.push({ id: String(p.id), value: String(p.value) });
      }
    }
    const variantEntry = {
      params: variantParams,
      display_name: String(variantName),
      is_default: Boolean(variant.isDefault || variant.is_default),
    };
    if (variant.description) variantEntry.description = String(variant.description);
    variants.push(variantEntry);
  }
  if (variants.length) entry.variants = variants;
  return entry;
}

function buildModelSelection(model, modelParams) {
  if (!Array.isArray(modelParams) || modelParams.length === 0) {
    return { id: model };
  }
  const params = modelParams
    .filter((p) => p && p.id && p.value != null)
    .map((p) => ({ id: String(p.id), value: String(p.value) }));
  return params.length ? { id: model, params } : { id: model };
}

function ndjson(obj) {
  return `${JSON.stringify(obj)}\n`;
}

function writeNdjson(res, obj) {
  if (res.writableEnded || res.destroyed) return false;
  return res.write(ndjson(obj));
}

async function nextStreamEventWithIdleTimeout(iter, idleMs) {
  let timer;
  try {
    return await Promise.race([
      iter.next(),
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          const err = new Error("STREAM_IDLE_TIMEOUT");
          err.code = "STREAM_IDLE_TIMEOUT";
          reject(err);
        }, idleMs);
        if (typeof timer.unref === "function") timer.unref();
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function streamAgentTurn(agent, prompt, mcpServers, active, runId) {
  const run = await agent.send(prompt, buildSendOptions(mcpServers));
  active.run = run;
  const rec = activeRuns.get(runId);
  if (rec) {
    rec.cancelFn = async () => {
      active.cancelRequested = true;
      await cancelRun(run);
    };
  }
  try {
    if (active.cancelRequested) {
      await cancelRun(run);
      return { cancelled: true, status: "cancelled", text: "", evict: true };
    }

    const textParts = [];
    const stream = typeof run.stream === "function" ? run.stream() : run.messages();
    const streamIter = stream[Symbol.asyncIterator]();
    const idleMs = streamIdleTimeoutMs();

    while (true) {
      if (active.cancelRequested) {
        await cancelRun(run);
        return { cancelled: true, status: "cancelled", text: "", evict: true };
      }
      let result;
      try {
        result = await nextStreamEventWithIdleTimeout(streamIter, idleMs);
        touchRunActivity(runId);
      } catch (err) {
        if (err?.code === "STREAM_IDLE_TIMEOUT") {
          logErr(
            `stream idle timeout after ${idleMs}ms (runId=${runId}); cancelling`,
          );
          await cancelRun(run);
          return {
            cancelled: false,
            status: "idle_timeout",
            text: joinCursorUtterances(textParts),
            evict: true,
          };
        }
        throw err;
      }
      if (result.done) break;
      emitStreamEvent(result.value, textParts, active);
    }

    if (active.cancelRequested) {
      await cancelRun(run);
      return { cancelled: true, status: "cancelled", text: "", evict: true };
    }

    const result = await waitForRunResult(run, runWaitTimeoutMs());
    if (active.cancelRequested) {
      await cancelRun(run);
      return { cancelled: true, status: "cancelled", text: "", evict: true };
    }
    const status = result.status || "finished";
    const resultText = typeof result.result === "string" ? result.result.trim() : "";
    const streamed = joinCursorUtterances(textParts);
    // SDK wait().result is the authoritative final write-up; streamed parts are
    // often token fragments or progress lines that must not be concatenated with it.
    const text = resultText || streamed;
    return {
      cancelled: false,
      status,
      text,
      evict: result.timedOut,
    };
  } finally {
    active.run = null;
  }
}

function joinCursorUtterances(parts) {
  const trimmed = parts
    .map((p) => String(p || "").trim())
    .filter(Boolean);
  if (looksLikeFragmentStream(trimmed)) {
    return joinStreamFragments(trimmed);
  }
  return trimmed.join("\n\n");
}

function looksLikeFragmentStream(parts) {
  if (parts.length < 12) {
    return false;
  }
  const short = parts.filter((p) => p.length <= 72 && !p.includes("\n\n")).length;
  if ((short * 100) / parts.length < 85) {
    return false;
  }
  const planning = parts.filter((p) => cursorPlanningOpener(p)).length;
  return planning <= 2;
}

function cursorPlanningOpener(text) {
  const first = String(text || "")
    .trim()
    .split("\n")[0]
    .trim()
    .toLowerCase();
  return (
    first.startsWith("i'll ") ||
    first.startsWith("i will ") ||
    first.startsWith("let me ") ||
    first.startsWith("first i") ||
    first.startsWith("next i") ||
    first.startsWith("i'm ") ||
    first.startsWith("i am ") ||
    first.startsWith("i already")
  );
}

function cursorTextEventIsStreamFragment(text) {
  const t = String(text || "").trim();
  if (!t || t.includes("\n\n") || t.startsWith("### ")) {
    return false;
  }
  if (cursorPlanningOpener(t)) {
    return false;
  }
  if (t.length > 72) {
    return false;
  }
  if (t.length >= 20 && t.split(/\s+/).length >= 4) {
    if (/[.!?]$/.test(t)) {
      return false;
    }
  }
  return true;
}

function fragmentJoinNeedsSpace(left, right) {
  if (!left || !right) {
    return false;
  }
  if (/^\s/.test(right)) {
    return false;
  }
  if (/^[,.;:!?)}\]"'/-]/.test(right)) {
    return false;
  }
  if (/[(\[{"'/-]$/.test(left)) {
    return false;
  }
  if (/\s$/.test(left)) {
    return false;
  }
  if (
    /[a-z0-9]$/i.test(left) &&
    /^[a-z]/.test(right) &&
    right.length <= 24 &&
    !right.includes("\n")
  ) {
    return false;
  }
  return true;
}

function joinStreamFragments(parts) {
  let out = "";
  for (const part of parts) {
    const chunk = String(part || "").trim();
    if (!chunk) {
      continue;
    }
    if (!out) {
      out = chunk;
      continue;
    }
    if (fragmentJoinNeedsSpace(out, chunk)) {
      out += " ";
    }
    out += chunk;
  }
  return out;
}

function pushCursorTextPart(textParts, chunk) {
  const raw = String(chunk || "");
  if (!raw.trim()) {
    return;
  }
  if (cursorTextEventIsStreamFragment(raw)) {
    if (/^\s/.test(raw) && textParts.length > 0) {
      textParts[textParts.length - 1] += raw;
      return;
    }
    const piece = raw.trimStart();
    if (!piece) {
      return;
    }
    if (textParts.length === 0) {
      textParts.push(piece);
      return;
    }
    const last = textParts[textParts.length - 1];
    if (fragmentJoinNeedsSpace(last, piece)) {
      textParts[textParts.length - 1] = `${last} ${piece}`;
    } else {
      textParts[textParts.length - 1] = `${last}${piece}`;
    }
    return;
  }
  const t = raw.trim();
  const last = (textParts.at(-1) || "").trim();
  if (last === t || last.endsWith(t)) {
    return;
  }
  if (t.startsWith(last) && t.length > last.length) {
    textParts[textParts.length - 1] = t;
    return;
  }
  textParts.push(t);
}

function emitStreamEvent(event, textParts, active) {
  const type = event?.type;
  if (type === "assistant" || type === "assistant_message" || type === "message") {
    const payload = event.message || event;
    const content = payload.content;
    if (typeof content === "string" && content.trim()) {
      pushCursorTextPart(textParts, content);
      writeNdjson(active.res, { type: "text", text: content });
      return;
    }
    for (const block of content || []) {
      if (typeof block === "string") {
        if (block.trim()) {
          pushCursorTextPart(textParts, block);
          writeNdjson(active.res, { type: "text", text: block });
        }
        continue;
      }
      if (block?.type === "text" && block.text) {
        pushCursorTextPart(textParts, block.text);
        writeNdjson(active.res, { type: "text", text: block.text });
      } else if (block?.type === "tool_use") {
        const input =
          block.input && typeof block.input === "object" ? block.input : {};
        writeNdjson(active.res, {
          type: "tool_use",
          name: block.name || "",
          input,
        });
      }
    }
    return;
  }
  if (type === "thinking" && event.text) {
    writeNdjson(active.res, { type: "thinking", thinking: event.text });
    return;
  }
  if (type === "tool_call" || type === "tool_use") {
    const name = event.name || "";
    const status = event.status || "running";
    if (status === "running" || status === "started") {
      const args = event.args;
      const input = args && typeof args === "object" && !Array.isArray(args) ? args : {};
      writeNdjson(active.res, { type: "tool_use", name, input });
    } else if (status === "completed" || status === "error") {
      writeNdjson(active.res, {
        type: "tool_result",
        name,
        output: stringifyUnknown(event.result),
        is_error: status === "error",
      });
    }
  }
}

async function openAgent(sdk, { model, modelParams, cwd, key, mcpServers, store }) {
  const { Agent } = sdk;
  const options = {
    apiKey: key,
    model: buildModelSelection(model, modelParams),
    local: { cwd, store, force: true },
  };
  if (mcpServers) options.mcpServers = mcpServers;
  return await Agent.create(options);
}

async function evictPooled(key) {
  const entry = POOL.get(key);
  if (!entry) return;
  POOL.delete(key);
  await disposeAgent(entry.agent);
  entry.agent = null;
}

async function evictIdleAndOverCap() {
  const now = Number(process.hrtime.bigint()) / 1e9;
  const ttl = agentIdleTtlSecs();
  const cap = agentPoolMax();
  const toClose = [];
  for (const [key, entry] of POOL) {
    if (entry.inUse > 0) continue;
    if (now - entry.lastUsedMonotonic >= ttl) {
      POOL.delete(key);
      toClose.push(entry);
    }
  }
  if (POOL.size > cap) {
    const overflow = [...POOL.values()]
      .filter((e) => e.inUse === 0)
      .sort((a, b) => a.lastUsedMonotonic - b.lastUsedMonotonic)
      .slice(0, Math.max(0, POOL.size - cap));
    for (const entry of overflow) {
      POOL.delete(entry.poolKey);
      toClose.push(entry);
    }
  }
  await Promise.all(toClose.map((e) => disposeAgent(e.agent)));
}

async function closeAllPooled() {
  const entries = [...POOL.values()];
  POOL.clear();
  await Promise.all(entries.map((e) => disposeAgent(e.agent)));
}

async function getPooledSlot(cwd, sessionScope) {
  const key = poolKey(cwd, sessionScope);
  let entry = POOL.get(key);
  if (!entry) {
    await evictIdleAndOverCap();
    entry = {
      poolKey: key,
      cwd,
      sessionScope,
      agent: null,
      lock: new Mutex(),
      lastUsedMonotonic: Number(process.hrtime.bigint()) / 1e9,
      inUse: 0,
    };
    POOL.set(key, entry);
  }
  return entry;
}

async function triggerCleanShutdown(reason) {
  if (shutdownTriggered) return;
  shutdownTriggered = true;
  logErr(`shutting down for recycle (${reason})`);
  if (reaperTimer) clearInterval(reaperTimer);
  await closeAllPooled();
  process.exit(0);
}

async function cancelRunsOlderThan(maxAgeSecs) {
  const now = monotonicNow();
  const cancelled = [];
  for (const [id, rec] of [...activeRuns.entries()]) {
    const wallAge = now - rec.startedMonotonic;
    if (wallAge < maxAgeSecs) continue;
    logErr(
      `cancelling old run id=${id} kind=${rec.runKind} wall=${wallAge.toFixed(0)}s (younger sessions kept)`,
    );
    try {
      if (rec.cancelFn) await rec.cancelFn();
    } catch (err) {
      logErr(`old run cancel failed id=${id}: ${err}`);
    }
    cancelled.push({
      id,
      kind: rec.runKind,
      wall_age_secs: Math.floor(wallAge),
    });
    activeRuns.delete(id);
  }
  return cancelled;
}

async function cancelStuckRuns() {
  const now = monotonicNow();
  const idleLimitSec = streamIdleTimeoutMs() / 1000;
  const wallLimitSec = maxRunWallAgeSecs();
  for (const [id, rec] of [...activeRuns.entries()]) {
    const idleAge = now - rec.lastEventMonotonic;
    const wallAge = now - rec.startedMonotonic;
    if (idleAge <= idleLimitSec * 1.05 && wallAge < wallLimitSec) {
      continue;
    }
    logErr(
      `reaper cancelling stuck run id=${id} kind=${rec.runKind} idle=${idleAge.toFixed(0)}s wall=${wallAge.toFixed(0)}s`,
    );
    try {
      if (rec.cancelFn) await rec.cancelFn();
    } catch (err) {
      logErr(`stuck run cancel failed id=${id}: ${err}`);
    }
    activeRuns.delete(id);
  }
}

async function reaperTick() {
  try {
    await cancelStuckRuns();
    await evictIdleAndOverCap();
    if (uptimeSecs() >= sidecarMaxUptimeSecs()) {
      recycleRequested = true;
    }
    if (recycleRequested && runsInFlightCount() === 0) {
      await triggerCleanShutdown("idle_recycle");
    }
  } catch (err) {
    logErr(`reaper error: ${err}`);
  }
}

function tryBeginRun(body) {
  const limit = runConcurrency();
  const reserve = interactiveReserve();
  const inFlight = runsInFlightCount();
  if (inFlight >= limit) return null;
  const scheduled = isScheduledRunBody(body);
  const scheduledCap = Math.max(1, limit - reserve);
  if (scheduled && inFlight >= scheduledCap) {
    return null;
  }
  return beginTrackedRun(scheduled ? "scheduled" : "interactive");
}

function endRun(runId) {
  endTrackedRun(runId);
}

function jsonResponse(res, status, body) {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": Buffer.byteLength(payload),
  });
  res.end(payload);
}

function readJsonBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      const raw = Buffer.concat(chunks).toString("utf8");
      if (!raw.trim()) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(raw));
      } catch (err) {
        reject(err);
      }
    });
    req.on("error", reject);
  });
}

async function handleHealth(_req, res) {
  let installed = sdkInstalledSync();
  if (!installed) {
    try {
      await loadSdk();
      installed = true;
    } catch {
      installed = false;
    }
  }
  jsonResponse(res, 200, {
    ok: true,
    service: "cursor-sdk-runner",
    runtime: "node",
    node_version: process.versions.node,
    local_store: "jsonl",
    api_key_configured: Boolean(apiKey()),
    cursor_sdk_installed: installed,
    mcp_supported: true,
    persona_bridges_active: POOL.size,
    session_scoped_bridges: true,
    bridge_idle_ttl_secs: agentIdleTtlSecs(),
    bridge_pool_max: agentPoolMax(),
    run_concurrency: runConcurrency(),
    runs_in_flight: runsInFlightCount(),
    interactive_reserve: interactiveReserve(),
    oldest_run_age_secs: oldestRunAgeSecs(),
    os_bridge_pids: 0,
    started_at_unix: Math.floor(STARTED_AT_UNIX),
    uptime_secs: Math.floor(uptimeSecs()),
    max_uptime_secs: sidecarMaxUptimeSecs(),
    recycle_requested: recycleRequested,
  });
}

async function handleModels(_req, res) {
  const key = apiKey();
  if (!key) {
    jsonResponse(res, 503, {
      ok: false,
      error: "CURSOR_API_KEY is not set on the sidecar host",
    });
    return;
  }
  let sdk;
  try {
    sdk = await loadSdk();
  } catch (err) {
    jsonResponse(res, 503, {
      ok: false,
      error: `@cursor/sdk is not installed (${err})`,
    });
    return;
  }
  try {
    const { Cursor } = sdk;
    const models = await Cursor.models.list({ apiKey: key });
    const payload = [];
    for (const model of models || []) {
      const entry = serializeModelEntry(model);
      if (entry) payload.push(entry);
    }
    jsonResponse(res, 200, { ok: true, models: payload });
  } catch (err) {
    jsonResponse(res, 502, { ok: false, error: String(err?.message || err) });
  }
}

async function handleRequestRecycle(req, res) {
  if (!clientIsLoopback(req)) {
    jsonResponse(res, 403, { accepted: false, reason: "loopback_only" });
    return;
  }
  if (runsInFlightCount() > 0) {
    jsonResponse(res, 202, {
      accepted: false,
      runs_in_flight: runsInFlightCount(),
      reason: "busy",
    });
    return;
  }
  recycleRequested = true;
  jsonResponse(res, 200, {
    accepted: true,
    runs_in_flight: 0,
    reason: "accepted",
  });
  setImmediate(() => {
    void triggerCleanShutdown("admin_recycle");
  });
}

async function handleCancelOldRuns(req, res) {
  if (!clientIsLoopback(req)) {
    jsonResponse(res, 403, { ok: false, reason: "loopback_only" });
    return;
  }
  let body = {};
  try {
    body = await readJsonBody(req);
  } catch {
    body = {};
  }
  const requested = Number.parseInt(String(body?.max_age_secs ?? ""), 10);
  const maxAgeSecs =
    Number.isFinite(requested) && requested >= 60
      ? requested
      : maxRunWallAgeSecs();
  const cancelled = await cancelRunsOlderThan(maxAgeSecs);
  jsonResponse(res, 200, {
    ok: true,
    max_age_secs: maxAgeSecs,
    cancelled: cancelled.length,
    runs: cancelled,
    runs_in_flight: runsInFlightCount(),
  });
}

async function handleRun(req, res) {
  let body;
  try {
    body = await readJsonBody(req);
  } catch (err) {
    jsonResponse(res, 400, {
      type: "error",
      message: `invalid JSON: ${err}`,
    });
    return;
  }
  const runId = tryBeginRun(body);
  if (!runId) {
    jsonResponse(res, 503, {
      type: "error",
      message: `Cursor sidecar at capacity (CURSOR_RUN_CONCURRENCY=${runConcurrency()}, interactive_reserve=${interactiveReserve()})`,
    });
    return;
  }

  res.writeHead(200, {
    "Content-Type": "application/x-ndjson",
    "Cache-Control": "no-cache",
    "X-Accel-Buffering": "no",
  });

  const active = {
    cancelRequested: false,
    run: null,
    res,
    runId,
  };
  const rec = activeRuns.get(runId);
  if (rec) {
    rec.cancelFn = async () => {
      active.cancelRequested = true;
      await cancelRun(active.run);
    };
  }
  const onClose = () => {
    if (active.cancelRequested) return;
    active.cancelRequested = true;
    logErr("client disconnected during /run; requesting turn cancel");
    void cancelRun(active.run);
  };
  req.on("close", onClose);

  try {
    await streamRun(body && typeof body === "object" ? body : {}, active);
  } catch (err) {
    logErr(`/run stream failed (${err}); requesting turn cancel`);
    await cancelRun(active.run);
    writeNdjson(res, { type: "error", message: String(err?.message || err) });
  } finally {
    req.off("close", onClose);
    if (!res.writableEnded) res.end();
    endRun(runId);
  }
}

async function streamRun(body, active) {
  const prompt = String(body.prompt || "").trim();
  const cwd = String(body.cwd || ".").trim() || ".";
  const model = String(body.model || DEFAULT_MODEL).trim() || DEFAULT_MODEL;
  const modelParams = Array.isArray(body.model_params) ? body.model_params : null;
  const sessionScope = String(body.session_scope || "").trim();
  let mcpServers = body.mcp_servers;
  if (!mcpServers || typeof mcpServers !== "object" || Array.isArray(mcpServers)) {
    mcpServers = null;
  }

  if (!prompt) {
    writeNdjson(active.res, { type: "error", message: "prompt required" });
    return;
  }
  const key = apiKey();
  if (!key) {
    writeNdjson(active.res, {
      type: "error",
      message: "CURSOR_API_KEY is not set on the sidecar host",
    });
    return;
  }

  let sdk;
  try {
    sdk = await loadSdk();
  } catch {
    writeNdjson(active.res, {
      type: "error",
      message: "@cursor/sdk is not installed (npm install @cursor/sdk)",
    });
    return;
  }

  const storeDir = ephemeralStoreDir(active.runId);
  const slot = await getPooledSlot(cwd, sessionScope);
  slot.inUse += 1;
  const maxAttempts = 3;
  const backoff = [500, 1500, 3000];

  try {
    let store;
    try {
      store = createEphemeralStore(sdk, storeDir);
    } catch (err) {
      writeNdjson(active.res, {
        type: "error",
        message: String(err?.message || err),
      });
      return;
    }

    for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
      try {
        const outcome = await slot.lock.run(async () => {
          slot.lastUsedMonotonic = Number(process.hrtime.bigint()) / 1e9;
          let agent = null;
          try {
            if (slot.agent) {
              const leftover = slot.agent;
              slot.agent = null;
              await disposeAgent(leftover);
            }
            agent = await openAgent(sdk, {
              model,
              modelParams,
              cwd,
              key,
              mcpServers,
              store,
            });
            slot.agent = agent;
            const result = await streamAgentTurn(
              agent,
              prompt,
              mcpServers,
              active,
              active.runId,
            );
            return result;
          } finally {
            slot.agent = null;
            await disposeAgent(agent);
          }
        });

        if (outcome.cancelled) {
          writeNdjson(active.res, { type: "error", message: "Run cancelled" });
          return;
        }
        writeNdjson(active.res, {
          type: "done",
          status: outcome.status,
          result: outcome.text,
        });
        slot.lastUsedMonotonic = Number(process.hrtime.bigint()) / 1e9;
        return;
      } catch (err) {
        if (attempt + 1 < maxAttempts && isBusyAgentError(err)) {
          logErr(
            `agent busy (attempt ${attempt + 1}/${maxAttempts}); retrying with local.force: ${err}`,
          );
          continue;
        }
        if (attempt + 1 < maxAttempts && isRetryableSdkError(err)) {
          logErr(
            `retryable SDK error (attempt ${attempt + 1}/${maxAttempts}): ${err}`,
          );
          await new Promise((r) => setTimeout(r, backoff[attempt]));
          continue;
        }
        const startup =
          err?.name === "CursorAgentError" ||
          String(err?.constructor?.name || "").includes("CursorAgent") ||
          isBusyAgentError(err);
        writeNdjson(active.res, {
          type: "error",
          message: startup
            ? `Cursor SDK startup failed: ${err?.message || err}`
            : String(err?.message || err),
        });
        return;
      }
    }
  } finally {
    slot.inUse = Math.max(0, slot.inUse - 1);
    removeStoreDir(storeDir);
  }
}

async function handleRequest(req, res) {
  const url = new URL(req.url || "/", "http://127.0.0.1");
  const route = `${req.method} ${url.pathname}`;
  try {
    if (route === "GET /health") {
      await handleHealth(req, res);
      return;
    }
    if (route === "GET /models") {
      await handleModels(req, res);
      return;
    }
    if (route === "POST /run") {
      await handleRun(req, res);
      return;
    }
    if (route === "POST /admin/request_recycle") {
      await handleRequestRecycle(req, res);
      return;
    }
    if (route === "POST /admin/cancel_old_runs") {
      await handleCancelOldRuns(req, res);
      return;
    }
    jsonResponse(res, 404, { ok: false, error: "not found" });
  } catch (err) {
    logErr(`handler error ${route}: ${err}`);
    if (!res.headersSent) {
      jsonResponse(res, 500, { ok: false, error: String(err?.message || err) });
    } else if (!res.writableEnded) {
      res.end();
    }
  }
}

async function runSelfTests() {
  if (!isEphemeralSessionScope("scheduled:17:2026-08-11T07:00:49Z")) {
    throw new Error("scheduled scope should be ephemeral");
  }
  if (!isEphemeralSessionScope("a1b2c3d4-e5f6-7890-abcd-ef1234567890")) {
    throw new Error("uuid scope should be ephemeral");
  }
  if (isEphemeralSessionScope("") || isEphemeralSessionScope("focus:project-x")) {
    throw new Error("main/focus scopes should not be ephemeral");
  }
  const key = poolKey(os.tmpdir(), "");
  if (!key.includes(":main")) throw new Error("pool key should use main scope");
  const storeDir = ephemeralStoreDir("run-self-test");
  if (!storeDir.includes("cursor-sdk-ephemeral")) {
    throw new Error("store root should live under cursor-sdk-ephemeral");
  }
  if (storeDir.includes(":")) {
    throw new Error("store directory must not contain colons");
  }
  if (runConcurrency() < 1) throw new Error("concurrency");
  if (agentIdleTtlSecs() < 60) throw new Error("idle ttl");
  if (sidecarMaxUptimeSecs() < 300) throw new Error("max uptime");
  if (typeof globalThis.crypto?.randomUUID !== "function") {
    throw new Error("globalThis.crypto.randomUUID unavailable");
  }
  if (
    !isBusyAgentError(
      new Error(
        "Agent agent-1c9d98ad-57cf-46ae-87ac-f6c5910b081c already has active run",
      ),
    )
  ) {
    throw new Error("busy error should match already-has-active-run");
  }
  if (isBusyAgentError(new Error("prompt required"))) {
    throw new Error("prompt required is not a busy error");
  }
  {
    const scheduledIds = [];
    const scheduledCap = Math.max(1, runConcurrency() - interactiveReserve());
    for (let i = 0; i < scheduledCap; i += 1) {
      const id = tryBeginRun({ run_kind: "scheduled" });
      if (!id) throw new Error(`scheduled slot ${i} should start`);
      scheduledIds.push(id);
    }
    if (tryBeginRun({ run_kind: "scheduled" }) != null) {
      throw new Error("scheduled should be blocked at capacity minus reserve");
    }
    const interactiveId = tryBeginRun({ run_kind: "interactive" });
    if (!interactiveId && runsInFlightCount() < runConcurrency()) {
      throw new Error("interactive should get a slot when below total concurrency");
    }
    for (const id of scheduledIds) endTrackedRun(id);
    if (interactiveId) endTrackedRun(interactiveId);
  }
  {
    const young = tryBeginRun({ run_kind: "interactive" });
    const old = tryBeginRun({ run_kind: "interactive" });
    if (!young || !old) throw new Error("cancel-old-runs self-test needs two slots");
    const oldRec = activeRuns.get(old);
    const youngRec = activeRuns.get(young);
    oldRec.startedMonotonic -= 1300;
    let oldCancelled = false;
    let youngCancelled = false;
    oldRec.cancelFn = async () => {
      oldCancelled = true;
    };
    youngRec.cancelFn = async () => {
      youngCancelled = true;
    };
    const cancelled = await cancelRunsOlderThan(1200);
    if (cancelled.length !== 1 || cancelled[0].id !== old) {
      throw new Error("cancel-old-runs should cancel only the 20min+ session");
    }
    if (!oldCancelled) throw new Error("old session cancelFn should run");
    if (youngCancelled) throw new Error("younger session must not be cancelled");
    if (!activeRuns.has(young)) throw new Error("younger session should remain in flight");
    if (activeRuns.has(old)) throw new Error("old session should be dropped from tracking");
    endTrackedRun(young);
  }
  const sendOpts = buildSendOptions(null);
  if (!sendOpts.local?.force) {
    throw new Error("send should force-expire stuck local runs");
  }
  process.stderr.write("cursor-sdk-runner self-tests ok\n");
}

function main() {
  if (process.argv[2] === "--self-test") {
    runSelfTests().catch((err) => {
      process.stderr.write(`cursor-sdk-runner self-test failed: ${err}\n`);
      process.exit(1);
    });
    return;
  }
  pruneLegacyResumeStores();
  const port = Number.parseInt(process.argv[2] || String(DEFAULT_PORT), 10);
  const server = http.createServer((req, res) => {
    void handleRequest(req, res);
  });
  server.keepAliveTimeout = 0;
  reaperTimer = setInterval(() => {
    void reaperTick();
  }, DEFAULT_REAPER_INTERVAL_SECS * 1000);
  reaperTimer.unref?.();
  server.listen(port, "0.0.0.0", () => {
    logErr(`Cursor SDK runner listening on 0.0.0.0:${port}`);
  });
  const onSignal = (sig) => {
    logErr(`received ${sig}`);
    void triggerCleanShutdown(sig);
  };
  process.on("SIGTERM", () => onSignal("SIGTERM"));
  process.on("SIGINT", () => onSignal("SIGINT"));
}

main();
