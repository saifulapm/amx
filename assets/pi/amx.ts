// installed by amx
//
// amx writes this file and `amx uninstall` removes it; `amx doctor --fix` puts
// it back the way it ships, so an edit here is an edit that goes. It does for
// pi what claude's hooks do for claude: report what the agent is doing to the
// amx that started this pane, one `amx _hook` per moment with the payload on
// stdin, stream what pi is saying to the pane's record while a turn runs, and
// beat on that record for as long as the turn lasts.
//
// It stays out of the way. Every report is fire-and-forget, nothing here
// throws, and a pi that no amx has anything to do with runs it as nothing.
// @ts-nocheck
import { spawn } from "node:child_process";
import { existsSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { delimiter, join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// Where the record is, for the stream. A pane amx started says so in its
// environment. A pi somebody started by hand and `amx adopt` took over carries
// nothing, so the hook says: it answers every report about this session with
// the record's directory, and the last answer is where the stream goes.
let answered: string | undefined;
function recordDir(): string | undefined {
  return process.env.AMX_DIR || answered;
}
// How often the stream is written, at most.
const STREAM_EVERY_MS = 100;
// How often a running turn says on the record that it is still running. Well
// inside the few seconds amx believes a report for, so a reader asking between
// two beats still finds a fresh one.
const BEAT_EVERY_MS = 3000;
// How much of a tool's arguments a report carries: the argument worth a row,
// not a file's whole contents.
const ARGUMENT_CHARS = 200;

// The amx to report to: the one that started this pane says where it is, and
// a pi somebody started by hand reports to whichever amx is on the PATH, which
// is how `amx adopt` hears from it. Neither is a pi amx is not watching.
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

// The words of one message: its text blocks, and nothing of its thinking or
// its tool calls.
function textOf(message): string {
  const content = message?.content;
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((block) => block?.type === "text" && typeof block.text === "string")
    .map((block) => block.text)
    .join("\n");
}

// A tool's arguments cut down to what a row can say about them.
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

  // Reports go one at a time, in the order the moments happened: a hook that
  // landed before the one it follows would move the record backwards.
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

  // What every report says about whose it is: the session, so a pi amx did
  // not start finds its record, and the file that session writes, so the
  // record can read the conversation back.
  function about(ctx): Record<string, unknown> {
    const fields: Record<string, unknown> = {};
    try {
      const id = ctx?.sessionManager?.getSessionId?.();
      if (typeof id === "string" && id) fields.session_id = id;
      const file = ctx?.sessionManager?.getSessionFile?.();
      if (typeof file === "string" && file) fields.transcript_path = file;
    } catch {
      // A session manager that will not say is a report with less on it.
    }
    if (typeof ctx?.cwd === "string") fields.cwd = ctx.cwd;
    return fields;
  }

  // The answer a turn ended on: the last thing the assistant said on the
  // branch, unless a prompt stands after it, which is a turn that ended with
  // nothing said.
  function lastAnswer(ctx): string | undefined {
    try {
      const branch = ctx.sessionManager.getBranch();
      for (let at = branch.length - 1; at >= 0; at--) {
        const entry = branch[at];
        if (entry?.type !== "message") continue;
        const role = entry.message?.role;
        if (role === "user") return undefined;
        if (role === "assistant") {
          const text = textOf(entry.message).trim();
          return text || undefined;
        }
      }
    } catch {
      // No branch to read is no answer to report.
    }
    return undefined;
  }

  // The stream: what pi is saying at this moment, written whole beside the
  // record and taken away when the message ends. Whole and renamed in, so a
  // reader never sees half of it.
  let pending: string | undefined;
  let streamed: string | undefined;
  let timer;
  function stream(text: string): void {
    // Nothing said yet — a message still thinking — is nothing to stream.
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
      // A record that cannot be written to streams nothing.
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
      // Nothing streamed is nothing to take away.
    }
  }

  // The beat: while a turn runs this extension is alive and knows it, so it
  // says so beside the record every few seconds and takes the file away when
  // the turn ends. amx believes what a vendor reports for a few seconds and
  // then reads the pane, and a mid-turn pi whose chrome an extension has
  // redrawn is a screen no rule claims — so a tool call longer than that
  // window read as nothing at all. A beat is the same thing a hook says, and
  // is heard the same way.
  //
  // The file's mtime is the whole of what it says, so nothing is written in
  // it, and the timer holds nothing open: a pi that goes away mid-turn takes
  // its beating with it and leaves a record nothing has spoken for since.
  let beating;
  function beat(): void {
    const dir = recordDir();
    if (!dir) return;
    try {
      writeFileSync(join(dir, "heartbeat"), "");
    } catch {
      // A record that cannot be written to is a turn nothing hears about.
    }
  }
  function startBeating(): void {
    // A pane amx did not start has no record to beat on until the first report
    // has been answered with one, so the beat asks where it is every time
    // rather than once.
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
      // Nothing beaten is nothing to take away.
    }
  }

  // Only a pi with a pane is a pi amx is watching: in rpc, json and print
  // modes there is no screen and no record behind it.
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
    const answer = lastAnswer(ctx);
    if (answer !== undefined) fields.last_assistant_message = answer;
    report("agent_settled", fields);
  });
}
