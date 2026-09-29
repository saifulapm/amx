// opencode's plugin, `assets/opencode/tui.js`, replayed against the events
// opencode 2.0.16 sent on 2026-09-29 (`tests/opencode/events/`, read in
// `docs/opencode-screens.md`). The ctx is a fake that hands each captured
// event to the handlers `ctx.data.on` registered for its type, and amx is
// `./amx`, which writes down every report it is handed.
import { test, mock, afterEach } from "node:test";
import assert from "node:assert/strict";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  unlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import plugin from "../../assets/opencode/tui.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const OPENCODE = join(HERE, "..", "opencode");
const STUB = join(HERE, "amx");

function lines(path) {
  return readFileSync(path, "utf8")
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => JSON.parse(line));
}
const events = (scenario) => lines(join(OPENCODE, "events", `${scenario}.jsonl`));
const messages = (scenario) => lines(join(OPENCODE, "messages", `${scenario}.jsonl`));

const saved = { env: { ...process.env }, argv: process.argv };
let undo = [];
afterEach(() => {
  for (const step of undo.reverse()) step();
  undo = [];
  process.env = { ...saved.env };
  process.argv = saved.argv;
  mock.timers.reset();
});

// One pane: a scratch record, the stub on AMX_BIN, and a ctx whose route the
// test moves. `amx: false` is a pane amx did not start.
function pane({ scenario = "turn", argv = [], amx = true, bin = STUB } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "amx-oc-plugin-"));
  undo.push(() => rmSync(dir, { recursive: true, force: true }));
  const record = join(dir, "record");
  mkdirSync(record);
  const rec = join(dir, "reports");
  process.env.AMX_REC = rec;
  delete process.env.AMX_ID;
  delete process.env.AMX_DIR;
  delete process.env.AMX_BIN;
  if (amx) {
    process.env.AMX_ID = "a1";
    process.env.AMX_DIR = record;
  }
  if (bin) process.env.AMX_BIN = bin;
  process.argv = ["/usr/bin/opencode", "--standalone", ...argv];

  const handlers = new Map();
  const seen = { on: [], dispatched: [], interrupted: [], synced: [] };
  let route = { type: "home" };
  const ctx = {
    data: {
      on(type, handler) {
        seen.on.push(type);
        if (!handlers.has(type)) handlers.set(type, new Set());
        handlers.get(type).add(handler);
        return () => handlers.get(type).delete(handler);
      },
      listen() {
        throw new Error("the plugin reads by ctx.data.on");
      },
      session: {
        root: (id) => id,
        message: {
          async sync(id) {
            seen.synced.push(id);
          },
          list: () => messages(scenario),
        },
      },
    },
    ui: { router: { current: () => route } },
    keymap: {
      dispatch(id) {
        seen.dispatched.push(id);
      },
    },
    client: {
      session: {
        async interrupt(input) {
          seen.interrupted.push(input);
        },
      },
    },
  };

  const self = {
    ctx,
    seen,
    record,
    dir,
    route: (next) => {
      route = next;
    },
    async start() {
      const cleanup = await plugin.setup(ctx);
      if (typeof cleanup === "function") undo.push(() => cleanup());
      return cleanup;
    },
    emit(event) {
      for (const handler of handlers.get(event.type) ?? []) handler(event);
    },
    // Hands the scenario's events over in order. The route moves to the new
    // session with its `session.created`, as it did live.
    replay(list = events(scenario)) {
      for (const event of list) {
        if (event.type === "session.created") {
          route = { type: "session", sessionID: event.data.sessionID };
        }
        self.emit(event);
      }
    },
    reports() {
      if (!existsSync(rec)) return [];
      // The last piece is a report still being written.
      return readFileSync(rec, "utf8")
        .split("\n")
        .slice(0, -1)
        .map((line) => {
          const at = line.indexOf(" ");
          return { bin: line.slice(0, at), ...JSON.parse(line.slice(at + 1)) };
        });
    },
  };
  return self;
}

// Reports go out through child processes; this waits for `count` of them.
async function reported(p, count) {
  const began = performance.now();
  while (p.reports().length < count) {
    if (performance.now() - began > 10000) {
      assert.fail(`waited for ${count} reports, have ${JSON.stringify(p.reports())}`);
    }
    await new Promise((resolve) => setImmediate(resolve));
  }
  // Give a report that should not be there the time to turn up.
  await new Promise((resolve) => setImmediate(resolve));
  const settle = performance.now();
  while (performance.now() - settle < 300) await new Promise((r) => setImmediate(r));
  return p.reports();
}

const names = (reports) => reports.map((r) => r.hook_event_name);

test("a turn is Started, Prompted, Calling and Ended, with its transcript", async () => {
  const p = pane({ scenario: "turn" });
  await p.start();
  p.replay();
  const got = await reported(p, 4);
  const session = "ses_f11706b91ffeWbVsNizrHjNypj";
  assert.deepEqual(names(got), [
    "session.selected",
    "session.execution.started",
    "session.tool.called",
    "session.execution.ended",
  ]);
  for (const r of got) {
    assert.equal(r.session_id, session);
    assert.equal(r.bin, STUB);
  }
  assert.equal(got[0].source, "startup");
  assert.equal(got[0].transcript_path, join(p.record, "opencode-messages.jsonl"));
  assert.match(got[1].prompt, /^Run the shell command `sleep 40`/);
  assert.equal(got[2].tool_name, "shell");
  assert.deepEqual(got[2].tool_input, { command: "sleep 40" });
  const ended = got[3];
  assert.equal(ended.last_assistant_message, "done");
  assert.equal(ended.stop_reason, "stop");
  const transcript = join(p.record, "opencode-messages.jsonl");
  assert.equal(ended.transcript_path, transcript);
  assert.ok(p.seen.synced.length >= 2, "written mid-turn and at the end");
  assert.ok(p.seen.synced.every((id) => id === session));
  assert.deepEqual(lines(transcript), messages("turn"));
  assert.equal(readFileSync(transcript, "utf8").split("\n").filter(Boolean).length, messages("turn").length);
});

test("a session opened by --session is Started as a resume", async () => {
  const p = pane({ scenario: "turn", argv: ["--session", "ses_f11706b91ffeWbVsNizrHjNypj"] });
  p.route({ type: "session", sessionID: "ses_f11706b91ffeWbVsNizrHjNypj" });
  await p.start();
  p.replay();
  const got = await reported(p, 4);
  assert.equal(got[0].hook_event_name, "session.selected");
  assert.equal(got[0].source, "resume");
  assert.equal(names(got).filter((n) => n === "session.selected").length, 1);
});

test("a message steered into a running turn is Taken", async () => {
  const p = pane({ scenario: "steer" });
  await p.start();
  p.replay();
  const got = await reported(p, 5);
  assert.deepEqual(names(got), [
    "session.selected",
    "session.execution.started",
    "session.tool.called",
    "session.inbox.delivered",
    "session.execution.ended",
  ]);
  assert.equal(got[4].last_assistant_message, "done\n\nbanana");
});

test("a permission card is Asked, and a rejected one Refused and Ended", async () => {
  const p = pane({ scenario: "permission" });
  await p.start();
  p.replay();
  const got = await reported(p, 10);
  assert.deepEqual(names(got), [
    "session.selected",
    "session.execution.started",
    "session.tool.called",
    "permission.asked",
    "session.execution.ended",
    "session.execution.started",
    "session.tool.called",
    "permission.asked",
    "permission.rejected",
    "session.execution.ended",
  ]);
  assert.equal(got[3].tool_name, "shell");
  assert.equal(got[4].stop_reason, "stop");
  assert.equal(got[4].last_assistant_message, "done");
  assert.equal(got[9].stop_reason, "aborted");
  assert.equal(got[9].last_assistant_message, undefined);
});

test("a question is Calling with its questions, Notified, and a dismissed one Refused", async () => {
  const p = pane({ scenario: "question" });
  await p.start();
  p.replay();
  const got = await reported(p, 10);
  assert.deepEqual(names(got), [
    "session.selected",
    "session.execution.started",
    "session.tool.called",
    "form.created",
    "session.execution.ended",
    "session.execution.started",
    "session.tool.called",
    "form.created",
    "permission.rejected",
    "session.execution.ended",
  ]);
  assert.equal(got[2].tool_name, "question");
  assert.equal(got[2].tool_input.questions[0].question, "Tea or coffee?");
  assert.equal(got[2].tool_input.questions[0].options[0].label, "Tea");
  assert.equal(got[3].kind, "question");
  assert.equal(got[3].message, "Tea or coffee?");
  assert.equal(got[7].message, "Milk or no milk?");
  assert.equal(got[9].stop_reason, "aborted");
});

test("an interrupted turn ends aborted and a failed one ends in error", async () => {
  const cut = pane({ scenario: "interrupt" });
  await cut.start();
  cut.replay();
  const got = await reported(cut, 4);
  assert.equal(got[3].hook_event_name, "session.execution.ended");
  assert.equal(got[3].stop_reason, "aborted");
  undo.reverse().forEach((step) => step());
  undo = [];

  const failed = pane({ scenario: "failure" });
  await failed.start();
  failed.replay();
  const after = await reported(failed, 3);
  assert.deepEqual(names(after), [
    "session.selected",
    "session.execution.started",
    "session.execution.ended",
  ]);
  assert.equal(after[2].stop_reason, "error");
  assert.equal(after[2].last_assistant_message, undefined);
});

test("SIGUSR2 interrupts the pane's session and keeps it", async () => {
  const p = pane({ scenario: "interrupt" });
  await p.start();
  p.replay(events("interrupt").slice(0, 5));
  // Node writes a diagnostic report on SIGUSR2 where it is set to.
  const reporting = process.report.reportOnSignal;
  process.report.reportOnSignal = false;
  undo.push(() => {
    process.report.reportOnSignal = reporting;
  });
  process.kill(process.pid, "SIGUSR2");
  const began = performance.now();
  while (!p.seen.interrupted.length && performance.now() - began < 5000) {
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.deepEqual(p.seen.interrupted, [
    { sessionID: "ses_f116d8d9fffep8KwH5Y1aEaqf0", resume: false },
  ]);
  assert.deepEqual(names(await reported(p, 2)), ["session.selected", "session.execution.started"]);
});

test("another session's events are not reported", async () => {
  const p = pane({ scenario: "turn" });
  p.route({ type: "session", sessionID: "ses_somebody_else" });
  await p.start();
  for (const event of events("turn")) p.emit(event);
  const got = await reported(p, 1);
  assert.deepEqual(names(got), ["session.selected"]);
  assert.equal(got[0].session_id, "ses_somebody_else");
});

test("a pane amx did not start registers nothing and reports nothing", async () => {
  const listeners = process.listenerCount("SIGUSR2");
  mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const p = pane({ scenario: "turn", amx: false, argv: ["--prompt=fix it"] });
  await p.start();
  p.replay();
  mock.timers.tick(31000);
  assert.deepEqual(p.seen.on, []);
  assert.deepEqual(p.seen.dispatched, []);
  assert.equal(process.listenerCount("SIGUSR2"), listeners);
  await reported(p, 0);
  assert.deepEqual(p.reports(), []);
  assert.equal(existsSync(join(p.record, "heartbeat")), false);
});

test("amx on the PATH is the one told when AMX_BIN names none", async () => {
  const p = pane({ scenario: "failure", bin: null });
  const path = join(p.dir, "path");
  mkdirSync(path);
  copyFileSync(STUB, join(path, "amx"));
  chmodSync(join(path, "amx"), 0o755);
  process.env.PATH = `${path}:${process.env.PATH}`;
  await p.start();
  p.replay();
  const got = await reported(p, 3);
  for (const r of got) assert.equal(r.bin, join(path, "amx"));
});

test("--prompt= on the home route is submitted every 500 ms until a session route", async () => {
  mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const p = pane({ argv: ["--prompt=fix it"] });
  await p.start();
  assert.deepEqual(p.seen.dispatched, []);
  mock.timers.tick(500);
  assert.deepEqual(p.seen.dispatched, ["prompt.submit"]);
  mock.timers.tick(500);
  assert.equal(p.seen.dispatched.length, 2);
  p.route({ type: "session", sessionID: "ses_f11706b91ffeWbVsNizrHjNypj" });
  mock.timers.tick(500);
  mock.timers.tick(5000);
  assert.equal(p.seen.dispatched.length, 2);
  const got = await reported(p, 1);
  assert.deepEqual(names(got), ["session.selected"]);
});

test("--prompt= stops being submitted at a turn's start or after 30 s", async () => {
  mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const started = pane({ argv: ["--prompt=fix it"] });
  await started.start();
  mock.timers.tick(500);
  started.emit(events("turn").find((e) => e.type === "session.execution.started"));
  mock.timers.tick(5000);
  assert.deepEqual(started.seen.dispatched, ["prompt.submit"]);
  undo.reverse().forEach((step) => step());
  undo = [];

  const stuck = pane({ argv: ["--prompt=fix it"] });
  await stuck.start();
  mock.timers.tick(30000);
  const count = stuck.seen.dispatched.length;
  assert.ok(count >= 59 && count <= 60, `${count} submits in 30 s`);
  mock.timers.tick(10000);
  assert.equal(stuck.seen.dispatched.length, count);
});

test("no --prompt= word is no submit", async () => {
  mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const p = pane({ argv: ["--prompt", "fix it"] });
  await p.start();
  mock.timers.tick(31000);
  assert.deepEqual(p.seen.dispatched, []);
});

test("a running turn beats every 3 s and the beat stops at its end", async () => {
  mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const p = pane({ scenario: "turn" });
  await p.start();
  const all = events("turn");
  const at = all.findIndex((e) => e.type === "session.execution.started");
  const beat = join(p.record, "heartbeat");
  p.replay(all.slice(0, at));
  assert.equal(existsSync(beat), false);
  p.replay(all.slice(at, at + 1));
  assert.equal(existsSync(beat), true);
  unlinkSync(beat);
  mock.timers.tick(2999);
  assert.equal(existsSync(beat), false);
  mock.timers.tick(1);
  assert.equal(existsSync(beat), true);
  unlinkSync(beat);
  mock.timers.tick(3000);
  assert.equal(existsSync(beat), true);
  p.replay(all.slice(at + 1));
  assert.equal(existsSync(beat), false);
  mock.timers.tick(6000);
  assert.equal(existsSync(beat), false);
});

test("the answer streams to live while it is written and goes at its end", async () => {
  mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const p = pane({ scenario: "steer" });
  await p.start();
  const all = events("steer");
  const live = join(p.record, "live");
  const last = all.findLastIndex((e) => e.type === "session.text.delta");
  p.replay(all.slice(0, last + 1));
  assert.equal(existsSync(live), false);
  mock.timers.tick(100);
  assert.equal(readFileSync(live, "utf8"), "done\n\nbanana");
  p.replay(all.slice(last + 1));
  assert.equal(existsSync(live), false);
});
