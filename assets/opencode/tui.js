// installed by amx
//
// amx's opencode TUI plugin. amx writes this file, `amx uninstall` removes it
// and `amx doctor --fix` restores it, so local edits do not last.
//
// Reports what the session in this pane is doing to amx, one `amx _hook` call
// per event with the JSON payload on stdin. While a turn runs it also streams
// the text being written to `live` in the agent's record and touches
// `heartbeat` there.
//
// opencode loads it into the TUI, so the environment is the pane's. The event
// feed carries every session the service holds, so only events for the
// session this pane shows are reported. Nothing here throws, and it does
// nothing in a pane amx did not start.
import { spawn } from "node:child_process";
import { existsSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { delimiter, join } from "node:path";

// Minimum interval between writes of `live`.
const STREAM_EVERY_MS = 100;
// Interval between heartbeat touches while a turn runs.
const BEAT_EVERY_MS = 3000;
// Longest string argument a tool report carries.
const ARGUMENT_CHARS = 200;
// `--prompt` fills the home route's composer without sending it, so the
// plugin presses submit at this interval until a session opens, for at most
// SUBMIT_FOR_MS.
const SUBMIT_EVERY_MS = 500;
const SUBMIT_FOR_MS = 30000;
// The tool that draws a question form. Its input is reported untrimmed.
const QUESTION_TOOL = "question";
// The conversation, written to the agent's record directory.
const TRANSCRIPT = "opencode-messages.jsonl";

// The amx to report to: $AMX_BIN, else the first amx on the PATH.
function amxBinary() {
  const named = process.env.AMX_BIN;
  if (named) return named;
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, "amx");
    if (existsSync(candidate)) return candidate;
  }
  return undefined;
}

// A tool's string arguments, each cut to ARGUMENT_CHARS.
function trimmed(args) {
  const kept = {};
  if (!args || typeof args !== "object") return kept;
  for (const [key, value] of Object.entries(args)) {
    if (typeof value === "string") kept[key] = value.slice(0, ARGUMENT_CHARS);
  }
  return kept;
}

export default {
  id: "amx",
  setup(ctx) {
    if (!process.env.AMX_ID) return;
    const amx = amxBinary();
    if (!amx) return;
    const record = process.env.AMX_DIR;

    // Reports are sent one at a time, in event order. `fields` may be a
    // promise; later reports wait for it.
    let queue = Promise.resolve();
    function report(event, fields) {
      queue = queue
        .then(() => fields)
        .then((settled) => deliver(JSON.stringify({ hook_event_name: event, ...settled })))
        .catch(() => {});
    }
    function deliver(payload) {
      return new Promise((resolve) => {
        let child;
        try {
          child = spawn(amx, ["_hook"], { stdio: ["pipe", "ignore", "ignore"] });
        } catch {
          resolve();
          return;
        }
        child.on("error", () => resolve());
        child.on("close", () => resolve());
        child.stdin.on("error", () => {});
        child.stdin.end(payload);
      });
    }

    // The session this pane shows, walked up to its root, since a subagent's
    // session belongs to its parent's conversation.
    function mine() {
      try {
        const route = ctx.ui.router.current();
        if (route?.type !== "session" || !route.sessionID) return undefined;
        return ctx.data.session.root(route.sessionID) || route.sessionID;
      } catch {
        return undefined;
      }
    }

    // opencode has no event for a pane opening a session, so the first
    // session route counts as the session starting.
    const resumed = process.argv.some((word) => word === "--session" || word.startsWith("--session="));
    let selected = false;
    function select() {
      if (selected) return;
      const id = mine();
      if (!id) return;
      selected = true;
      const fields = { session_id: id, source: resumed ? "resume" : "startup" };
      // Named from the start, so a card opened during the first turn reads
      // the conversation file instead of the screen.
      if (record) fields.transcript_path = join(record, TRANSCRIPT);
      report("session.selected", fields);
    }

    // The text being written, saved whole to `live` (write then rename) and
    // removed when the text part ends.
    let pending;
    let streamed;
    let timer;
    function stream(text) {
      if (!record || !text.trim()) return;
      pending = text;
      if (timer) return;
      timer = setTimeout(() => {
        timer = undefined;
        flush();
      }, STREAM_EVERY_MS);
      timer.unref?.();
    }
    function flush() {
      if (pending === undefined || pending === streamed) return;
      const path = join(record, "live");
      try {
        writeFileSync(`${path}.tmp`, pending);
        renameSync(`${path}.tmp`, path);
        streamed = pending;
      } catch {
        // Best effort: the record may be gone.
      }
      pending = undefined;
    }
    function endStream() {
      if (timer) {
        clearTimeout(timer);
        timer = undefined;
      }
      pending = undefined;
      streamed = undefined;
      if (!record) return;
      try {
        unlinkSync(join(record, "live"));
      } catch {
        // Nothing was streamed.
      }
    }

    // The heartbeat, as in the pi extension: amx reads only its mtime.
    let beating;
    function beat() {
      if (!record) return;
      try {
        writeFileSync(join(record, "heartbeat"), "");
      } catch {
        // Best effort: the record may be gone.
      }
    }
    function startBeating() {
      beat();
      if (beating) return;
      beating = setInterval(beat, BEAT_EVERY_MS);
      beating.unref?.();
    }
    function stopBeating() {
      if (beating) {
        clearInterval(beating);
        beating = undefined;
      }
      if (!record) return;
      try {
        unlinkSync(join(record, "heartbeat"));
      } catch {
        // No heartbeat to remove.
      }
    }

    // The whole conversation, one message per line, rewritten after each tool
    // call, each text part and each turn's end. amx reads this file and never
    // opens opencode's database. Writes are serialised.
    let writing = Promise.resolve();
    function transcript(id) {
      writing = writing.then(() => write(id));
      return writing;
    }
    async function write(id) {
      if (!record) return undefined;
      try {
        await ctx.data.session.message.sync(id);
        const list = ctx.data.session.message.list(id) ?? [];
        const path = join(record, TRANSCRIPT);
        writeFileSync(`${path}.tmp`, list.map((message) => `${JSON.stringify(message)}\n`).join(""));
        renameSync(`${path}.tmp`, path);
        return path;
      } catch {
        return undefined;
      }
    }

    // State of the current turn.
    let running = false;
    let prompt;
    const steered = new Set();
    const tools = new Map();
    let said = [];
    let finish;
    let text = "";

    const offs = [];
    function on(type, handle) {
      offs.push(
        ctx.data.on(type, (event) => {
          select();
          const data = event?.data;
          const id = mine();
          if (!id || !data || (data.sessionID ?? data.form?.sessionID) !== id) return;
          try {
            handle(data, id);
          } catch {
            // An event of an unexpected shape reports nothing.
          }
        }),
      );
    }

    on("session.inbox.enqueued", (data) => {
      prompt = data.item?.payload?.text;
      if (running) steered.add(data.inboxID);
    });
    on("session.execution.started", (_data, id) => {
      running = true;
      said = [];
      finish = undefined;
      report("session.execution.started", { session_id: id, prompt });
      startBeating();
    });
    on("session.inbox.delivered", (data, id) => {
      if (steered.delete(data.inboxID)) report("session.inbox.delivered", { session_id: id });
    });
    on("session.tool.input.started", (data) => {
      tools.set(data.id, data.name);
    });
    on("session.tool.called", (data, id) => {
      const name = tools.get(data.id);
      tools.delete(data.id);
      report("session.tool.called", {
        session_id: id,
        tool_name: name,
        tool_input: name === QUESTION_TOOL ? data.input : trimmed(data.input),
      });
      transcript(id);
    });
    on("permission.asked", (data, id) => {
      report("permission.asked", { session_id: id, tool_name: data.action });
    });
    on("permission.replied", (data, id) => {
      if (data.reply === "reject") report("permission.rejected", { session_id: id });
    });
    on("form.created", (data, id) => {
      const form = data.form;
      if (form?.metadata?.kind !== "question") return;
      const words = (form.fields ?? []).map((field) => field.description).filter(Boolean);
      report("form.created", { session_id: id, kind: "question", message: words.join("\n") });
    });
    on("form.cancelled", (_data, id) => {
      report("permission.rejected", { session_id: id });
    });
    on("session.step.started", () => {
      said = [];
    });
    on("session.step.ended", (data) => {
      finish = data.finish;
    });
    on("session.text.started", () => {
      text = "";
    });
    on("session.text.delta", (data) => {
      text += data.delta ?? "";
      stream(text);
    });
    on("session.text.ended", (data, id) => {
      if (typeof data.text === "string") said.push(data.text);
      text = "";
      endStream();
      transcript(id);
    });
    for (const how of ["succeeded", "failed", "interrupted"]) {
      on(`session.execution.${how}`, (_data, id) => {
        running = false;
        steered.clear();
        tools.clear();
        stopBeating();
        endStream();
        const fields = { session_id: id };
        if (how === "succeeded") {
          fields.stop_reason = finish;
          const answer = said.join("\n").trim();
          if (answer) fields.last_assistant_message = answer;
        } else {
          fields.stop_reason = how === "failed" ? "error" : "aborted";
        }
        report(
          "session.execution.ended",
          transcript(id).then((path) => (path ? { ...fields, transcript_path: path } : fields)),
        );
      });
    }

    // Submit a `--prompt=` task left in the home route's composer. The route
    // is also polled for the session start, since a pane resumed onto an idle
    // session gets no event.
    let submitting = process.argv.some((word) => word.startsWith("--prompt="));
    const giveUp = setTimeout(() => {
      submitting = false;
    }, SUBMIT_FOR_MS);
    giveUp.unref?.();
    offs.push(
      ctx.data.on("session.execution.started", () => {
        submitting = false;
      }),
    );
    const watching = setInterval(() => {
      select();
      if (selected) submitting = false;
      if (submitting) {
        try {
          if (ctx.ui.router.current()?.type === "home") ctx.keymap.dispatch("prompt.submit");
        } catch {
          // The task stays in the composer.
        }
      }
      if (selected && !submitting) clearInterval(watching);
    }, SUBMIT_EVERY_MS);
    watching.unref?.();

    // `amx stop` sends SIGUSR2 to end the turn before it closes the pane.
    // `resume: false` keeps the session without continuing it.
    function interrupt() {
      const id = mine();
      if (!id) return;
      try {
        Promise.resolve(ctx.client.session.interrupt({ sessionID: id, resume: false })).catch(() => {});
      } catch {
        // The turn then ends with the pane.
      }
    }
    process.on("SIGUSR2", interrupt);

    return () => {
      process.off("SIGUSR2", interrupt);
      clearInterval(watching);
      clearTimeout(giveUp);
      for (const off of offs) off();
      stopBeating();
      endStream();
    };
  },
};
