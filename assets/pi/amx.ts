// installed by amx
//
// amx's pi extension. amx writes this file, `amx uninstall` removes it and
// `amx doctor --fix` restores it, so local edits do not last.
//
// Does for pi what claude's hooks do for claude: reports each event to amx,
// one `amx _hook` call with the JSON payload on stdin. While a turn runs it
// also streams the text being written to `live` in the agent's record and
// touches `heartbeat` there. Nothing here throws, and it does nothing when no
// amx is found.
// @ts-nocheck
import { spawn } from "node:child_process";
import { existsSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { delimiter, join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// The agent's record directory: $AMX_DIR in a pane amx started. A pi started
// by hand and adopted has no $AMX_DIR, so `amx _hook` prints the directory in
// reply to each report and the last reply is used.
let answered: string | undefined;
function recordDir(): string | undefined {
  return process.env.AMX_DIR || answered;
}
// Minimum interval between writes of `live`.
const STREAM_EVERY_MS = 100;
// Interval between heartbeat touches while a turn runs. Well inside the few
// seconds amx trusts a report for, so a reader between two beats still finds
// a fresh one.
const BEAT_EVERY_MS = 3000;
// Longest string argument a tool report carries.
const ARGUMENT_CHARS = 200;

// The amx to report to: $AMX_BIN, else the first amx on the PATH, which is
// how a pi started by hand reaches `amx adopt`.
function amxBinary(): string | undefined {
  const named = process.env.AMX_BIN;
  if (named) return named;
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, "amx");
    if (existsSync(candidate)) return candidate;
  }
  return undefined;
}

// A message's text blocks joined, without thinking or tool calls.
function textOf(message): string {
  const content = message?.content;
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((block) => block?.type === "text" && typeof block.text === "string")
    .map((block) => block.text)
    .join("\n");
}

// A tool's string arguments, each cut to ARGUMENT_CHARS.
function trimmed(args): Record<string, string> {
  const kept: Record<string, string> = {};
  if (!args || typeof args !== "object") return kept;
  for (const [key, value] of Object.entries(args)) {
    if (typeof value === "string") kept[key] = value.slice(0, ARGUMENT_CHARS);
  }
  return kept;
}

export default function (pi: ExtensionAPI) {
  const amx = amxBinary();
  if (!amx) return;

  // Reports are sent one at a time, in event order, so the record never
  // moves backwards.
  let queue: Promise<void> = Promise.resolve();
  function report(event: string, fields: Record<string, unknown>): void {
    const payload = JSON.stringify({ hook_event_name: event, ...fields });
    queue = queue.then(() => deliver(payload)).catch(() => {});
  }

  function deliver(payload: string): Promise<void> {
    return new Promise((resolve) => {
      let child;
      try {
        child = spawn(amx, ["_hook"], { stdio: ["pipe", "pipe", "ignore"] });
      } catch {
        resolve();
        return;
      }
      let heard = "";
      child.on("error", () => resolve());
      child.on("close", () => {
        const line = heard.trim().split("\n").pop();
        if (line) answered = line;
        resolve();
      });
      child.stdout.on("data", (chunk) => {
        heard += chunk;
      });
      child.stdout.on("error", () => {});
      child.stdin.on("error", () => {});
      child.stdin.end(payload);
    });
  }

  // Fields every report carries: the session id, so an adopted pi finds its
  // record, the session file, so amx can read the conversation, and the cwd.
  function about(ctx): Record<string, unknown> {
    const fields: Record<string, unknown> = {};
    try {
      const id = ctx?.sessionManager?.getSessionId?.();
      if (typeof id === "string" && id) fields.session_id = id;
      const file = ctx?.sessionManager?.getSessionFile?.();
      if (typeof file === "string" && file) fields.transcript_path = file;
    } catch {
      // Report without them.
    }
    if (typeof ctx?.cwd === "string") fields.cwd = ctx.cwd;
    return fields;
  }

  // How a turn ended: the last assistant message on the branch and its stop
  // reason. Nothing when a user message or tool result comes after it.
  function ending(ctx): { answer?: string; stopReason?: string } {
    try {
      const branch = ctx.sessionManager.getBranch();
      for (let at = branch.length - 1; at >= 0; at--) {
        const entry = branch[at];
        if (entry?.type !== "message") continue;
        const role = entry.message?.role;
        if (role === "user" || role === "toolResult") return {};
        if (role === "assistant") {
          const text = textOf(entry.message).trim();
          return { answer: text || undefined, stopReason: entry.message.stopReason };
        }
      }
    } catch {
      // No branch, no answer.
    }
    return {};
  }

  // The text being written, saved whole to `live` (write then rename, so a
  // reader never sees half of it) and removed when the message ends.
  let pending: string | undefined;
  let streamed: string | undefined;
  let timer;
  function stream(text: string): void {
    // A message still thinking has no text yet.
    if (!recordDir() || !text.trim()) return;
    pending = text;
    if (timer) return;
    timer = setTimeout(() => {
      timer = undefined;
      flush();
    }, STREAM_EVERY_MS);
    timer.unref?.();
  }
  function flush(): void {
    if (pending === undefined || pending === streamed) return;
    const dir = recordDir();
    if (!dir) return;
    const path = join(dir, "live");
    try {
      writeFileSync(`${path}.tmp`, pending);
      renameSync(`${path}.tmp`, path);
      streamed = pending;
    } catch {
      // Best effort: the record may be gone.
    }
    pending = undefined;
  }
  function endStream(): void {
    if (timer) {
      clearTimeout(timer);
      timer = undefined;
    }
    pending = undefined;
    streamed = undefined;
    const dir = recordDir();
    if (!dir) return;
    try {
      unlinkSync(join(dir, "live"));
    } catch {
      // Nothing was streamed.
    }
  }

  // The heartbeat: touched every BEAT_EVERY_MS while a turn runs and removed
  // when it ends. amx trusts a report for a few seconds and then reads the
  // pane, and a mid-turn pi whose chrome an extension redrew matches no screen
  // rule, so without the beat a long tool call read as `unknown`. amx reads
  // only the file's mtime. The timer is unref'd, so it never keeps pi running.
  let beating;
  function beat(): void {
    const dir = recordDir();
    if (!dir) return;
    try {
      writeFileSync(join(dir, "heartbeat"), "");
    } catch {
      // Best effort: the record may be gone.
    }
  }
  function startBeating(): void {
    // An adopted pi learns its record only from the first reply, so each
    // beat looks the directory up again.
    beat();
    if (beating) return;
    beating = setInterval(beat, BEAT_EVERY_MS);
    beating.unref?.();
  }
  function stopBeating(): void {
    if (beating) {
      clearInterval(beating);
      beating = undefined;
    }
    const dir = recordDir();
    if (!dir) return;
    try {
      unlinkSync(join(dir, "heartbeat"));
    } catch {
      // No heartbeat to remove.
    }
  }

  // Only the TUI mode is watched: rpc, json and print modes have no pane and
  // no record.
  let watched = false;

  pi.on("session_start", (_event, ctx) => {
    watched = ctx?.mode === "tui";
    if (!watched) return;
    report("session_start", about(ctx));
  });

  pi.on("agent_start", (_event, ctx) => {
    if (!watched) return;
    report("agent_start", about(ctx));
    startBeating();
  });

  pi.on("tool_execution_start", (event, ctx) => {
    if (!watched) return;
    report("tool_execution_start", {
      ...about(ctx),
      tool_name: event.toolName,
      tool_input: trimmed(event.args),
    });
  });

  pi.on("ui_prompt_start", (event, ctx) => {
    if (!watched) return;
    report("ui_prompt_start", {
      ...about(ctx),
      kind: event.kind,
      message: event.title || `pi is waiting on a ${event.kind}`,
    });
  });

  pi.on("ui_prompt_end", (event, ctx) => {
    if (!watched) return;
    report("ui_prompt_end", { ...about(ctx), kind: event.kind });
  });

  // A user message starting is how pi signals that a message queued behind a
  // running turn went in, since it starts no new agent run for it. Assistant
  // and tool messages are part of the turn and are not reported.
  pi.on("message_start", (event, ctx) => {
    if (!watched || event.message?.role !== "user") return;
    report("message_start", { ...about(ctx), role: "user" });
  });

  pi.on("message_update", (event) => {
    if (!watched || event.message?.role !== "assistant") return;
    stream(textOf(event.message));
  });

  pi.on("message_end", () => {
    if (!watched) return;
    endStream();
  });

  pi.on("agent_settled", (_event, ctx) => {
    if (!watched) return;
    stopBeating();
    endStream();
    const fields = about(ctx);
    const { answer, stopReason } = ending(ctx);
    if (answer !== undefined) fields.last_assistant_message = answer;
    if (typeof stopReason === "string") fields.stop_reason = stopReason;
    report("agent_settled", fields);
  });
}
