// installed by amx
//
// amx's opt-in `subagent` tool for pi. amx writes this file, `amx uninstall`
// removes it and `amx doctor --fix` restores it, so local edits do not last.
//
// The tool hands a task to `amx sub` and returns the child's answer. It is
// separate from amx.ts because a tool is a capability a person opts into,
// and someone with their own `subagent` tool should not get a second one.
//
// It registers only in a pane amx started ($AMX_ID set) and runs the child
// through $AMX_BIN, the amx that started this pane. Nothing here throws: a
// child that could not start comes back as the tool's answer.
// @ts-nocheck
import { spawn } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

// Longest stderr or stdout a failure result carries.
const SAID_CHARS = 2000;

// Exit code and output of one amx run.
interface Ran {
  code: number;
  out: string;
  err: string;
}

// Run amx with `args` and collect its exit code and output. Aborting the tool
// call kills the child.
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
        // Already gone.
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

// The object `amx sub --json` prints, or undefined when there is none, as
// when amx refused before creating a child.
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
      role: Type.Optional(
        Type.String({
          description:
            "A role to spawn with, by name: its file supplies the dials and a brief, and anything passed here stands over them.",
        }),
      ),
      model: Type.Optional(
        Type.String({
          description:
            "The model for the child, when it runs this one's agent: the provider/id its own list prints, or the bare id alone. Naming a model also picks the vendor, so name the agent beside it to keep this one's.",
        }),
      ),
      effort: Type.Optional(
        Type.String({ description: "How much reasoning effort the child spends." }),
      ),
    }),
    async execute(_toolCallId, params, signal) {
      const args = ["sub", "--json", params.task];
      if (params.agent) args.push("--agent", params.agent);
      if (params.role) args.push("--role", params.role);
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
        // The child stopped on a question and is still running.
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
      // No child, or one that ended without an answer: amx's stderr says why.
      const said = (ran.err || ran.out).trim().slice(0, SAID_CHARS);
      return {
        ...text(said || `the child did not finish (exit ${ran.code})`),
        details: { id, phase: view?.phase ?? "failed" },
      };
    },
  });
}
