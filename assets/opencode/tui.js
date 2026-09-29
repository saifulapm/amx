// installed by amx
//
// amx writes this file and `amx uninstall` removes it; `amx doctor --fix` puts
// it back the way it ships, so an edit here is an edit that goes. It does for
// opencode what `assets/pi/amx.ts` does for pi: report what the agent is doing
// to the amx that started this pane, one `amx _hook` per moment with the
// payload on stdin, stream what opencode is saying to the pane's record while
// a turn runs, and beat on that record for as long as the turn lasts.
//
// opencode loads it into the TUI, the pane itself, so the environment is the
// pane's. The event feed is the service's and carries every pane's sessions,
// so a report is only ever about the session this pane shows.
//
// It stays out of the way. Every report is fire-and-forget, nothing here
// throws, and an opencode that no amx started runs it as nothing.
import { spawn } from "node:child_process";
import { existsSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { delimiter, join } from "node:path";

// How often the stream is written, at most.
const STREAM_EVERY_MS = 100;
// How often a running turn says on the record that it is still running.
const BEAT_EVERY_MS = 3000;
// How much of a tool's arguments a report carries.
const ARGUMENT_CHARS = 200;
// `--prompt` fills the home route's composer and never sends it, so the
// plugin presses submit this often until a session opens, for this long.
const SUBMIT_EVERY_MS = 500;
const SUBMIT_FOR_MS = 30000;
// The tool that draws a question form, whose input is the questions whole.
const QUESTION_TOOL = "question";
// The conversation's file beside the record.
const TRANSCRIPT = "opencode-messages.jsonl";

// The amx to report to: the one that started this pane says where it is, else
// whichever amx is on the PATH.
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

// A tool's arguments cut down to what a row can say about them.
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

    // Reports go one at a time, in the order the moments happened. `fields`
    // may be a promise, which holds every report behind it until it settles.
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

    // The session this pane shows, walked to its root: a subagent's session
    // routed in the pane is still its parent's conversation.
    function mine() {
      try {
        const route = ctx.ui.router.current();
        if (route?.type !== "session" || !route.sessionID) return undefined;
        return ctx.data.session.root(route.sessionID) || route.sessionID;
      } catch {
        return undefined;
      }
    }

    // Started is the first session route: opencode names no moment for a
    // pane opening onto its session.
    const resumed = process.argv.some((word) => word === "--session" || word.startsWith("--session="));
    let selected = false;
    function select() {
      if (selected) return;
      const id = mine();
      if (!id) return;
      selected = true;
      const fields = { session_id: id, source: resumed ? "resume" : "startup" };
      // Named from the start, so a card opened in the first turn reads the
      // conversation (the task, until the first write) rather than the screen,
      // whose last rows are opencode's composer.
      if (record) fields.transcript_path = join(record, TRANSCRIPT);
      report("session.selected", fields);
    }

    // The stream: what opencode is saying at this moment, written whole beside
    // the record and taken away when the text ends.
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
        // A record that cannot be written to streams nothing.
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
        // Nothing streamed is nothing to take away.
      }
    }

    // The beat, as pi's: the heartbeat file's mtime says the turn goes on.
    let beating;
    function beat() {
      if (!record) return;
      try {
        writeFileSync(join(record, "heartbeat"), "");
      } catch {
        // A record that cannot be written to is a turn nothing hears about.
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
        // Nothing beaten is nothing to take away.
      }
    }

    // The conversation, written whole beside the record after each tool call,
    // each text and each turn's end, one message a line. amx reads it there
    // and never opens opencode's db. Writes go one at a time.
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

    // What the turn has said and done so far.
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
            // An event of a shape this was not written for reports nothing.
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

    // A `--prompt=` task on the home route waits in the composer until it is
    // sent. The route is watched besides for Started, which a pane resumed
    // onto a quiet session hears no event for.
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
          // A keymap that will not take it leaves the task in the composer.
        }
      }
      if (selected && !submitting) clearInterval(watching);
    }, SUBMIT_EVERY_MS);
    watching.unref?.();

    // `amx stop` sends SIGUSR2 to end the turn before it ends the pane. The
    // session is kept, not resumed.
    function interrupt() {
      const id = mine();
      if (!id) return;
      try {
        Promise.resolve(ctx.client.session.interrupt({ sessionID: id, resume: false })).catch(() => {});
      } catch {
        // A client that will not interrupt leaves the turn to the pane's end.
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
