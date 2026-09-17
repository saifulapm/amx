// installed by amx
//
// amx writes this file and `amx uninstall` removes it; `amx doctor --fix` puts
// it back the way it ships, so an edit here is an edit that goes.
//
// It is a second extension beside amx.ts, and a person opts into it: amx.ts
// reports what the agent does, and this gives the agent something to do — a
// `subagent` tool that hands a task to `amx sub` and waits for the child's
// answer. The two are kept apart on purpose. Reporting is plumbing every amx
// pane wants; a tool is a capability, it spends a turn and writes a record,
// and a person who runs an extension of their own with a tool of that name
// should not have amx putting a second one in the agent's hands.
//
// Only a pane amx started carries `AMX_ID`, and that is the whole of where the
// tool registers. `AMX_BIN` rides beside it on the same pane, so the child is
// raised by the amx that raised this one. Nothing here throws: a child that
// could not be started is an answer the tool gives back, not a broken pane.
// @ts-nocheck
import { spawn } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

// How much of a child's own words a failure carries. A refusal is a line or
// two; a wall of it is not worth reading in a tool result.
const SAID_CHARS = 2000;

// What one run of the child answered, before any of it is read.
interface Ran {
  code: number;
  out: string;
  err: string;
}

// Run `amx` and give back what it said. `--json` is what this tool asks for,
// since a child's id and phase are what a caller needs and a person reading a
// screen is not here to parse them out of a sentence.
function runAmx(amx: string, args: string[], signal?: AbortSignal): Promise<Ran> {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(amx, args, { stdio: ["ignore", "pipe", "pipe"] });
    } catch (error) {
      resolve({ code: 1, out: "", err: String(error) });
      return;
    }
    let out = "";
    let err = "";
    const kill = () => {
      try {
        child.kill();
      } catch {
        // A child that is already gone is nothing to kill.
      }
    };
    signal?.addEventListener?.("abort", kill, { once: true });
    child.on("error", (error) => resolve({ code: 1, out, err: err || String(error) }));
    child.stdout.on("data", (chunk) => {
      out += chunk;
    });
    child.stderr.on("data", (chunk) => {
      err += chunk;
    });
    child.on("close", (code) => {
      signal?.removeEventListener?.("abort", kill);
      resolve({ code: code ?? 1, out, err });
    });
  });
}

// The one object `amx sub --json` writes, or nothing when there was no child
// to write one about — a refusal happens before amx has an id to report.
function reported(out: string): Record<string, unknown> | undefined {
  const line = out.trim();
  if (!line) return undefined;
  try {
    const object = JSON.parse(line);
    return object && typeof object.id === "string" ? object : undefined;
  } catch {
    return undefined;
  }
}

export default function (pi: ExtensionAPI) {
  if (!process.env.AMX_ID) return;
  const amx = process.env.AMX_BIN;
  if (!amx) return;

  pi.registerTool({
    name: "subagent",
    label: "Subagent",
    description:
      "Hand a task to a child agent and return its answer. The child runs in this agent's directory and stands under it on the wall; say what it should do, not how.",
    parameters: Type.Object({
      task: Type.String({ description: "What the child should do." }),
      agent: Type.Optional(
        Type.String({
          description: "The agent command to run the child with, instead of this one's.",
        }),
      ),
      model: Type.Optional(
        Type.String({
          description: "The model for the child, when it runs this one's agent.",
        }),
      ),
      effort: Type.Optional(
        Type.String({ description: "How much reasoning effort the child spends." }),
      ),
    }),
    async execute(_toolCallId, params, signal) {
      const args = ["sub", "--json", params.task];
      if (params.agent) args.push("--agent", params.agent);
      if (params.model) args.push("--model", params.model);
      if (params.effort) args.push("--effort", params.effort);

      const ran = await runAmx(amx, args, signal);
      const view = reported(ran.out);
      const id = view?.id;
      const text = (said: string) => ({ content: [{ type: "text", text: said }] });

      if (ran.code === 0 && view) {
        return {
          ...text(view.answer ?? "(the child said nothing)"),
          details: { id, phase: view.phase },
        };
      }
      if (ran.code === 2 && view) {
        // The child stopped on a question and is still standing. Whoever reads
        // this can read the question off `amx result` and put an answer to it.
        return {
          ...text(
            `The child stopped on a question (${id}). Read it with \`amx result ${id}\` and answer it with \`amx answer ${id}\`.`,
          ),
          details: { id, phase: view.phase },
        };
      }
      if (ran.code === 3 && view) {
        return {
          ...text(`The child is still running (${id}); it did not answer in time.`),
          details: { id, phase: view.phase },
        };
      }
      // No child to report on, or one that ended without answering: what amx
      // said on stderr is the whole of the reason.
      const said = (ran.err || ran.out).trim().slice(0, SAID_CHARS);
      return {
        ...text(said || `the child did not finish (exit ${ran.code})`),
        details: { id, phase: view?.phase ?? "failed" },
      };
    },
  });
}
