import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import {
  buildJsonOutput,
  formatConversationForJudge,
  parseArgs,
  printComparisonTable,
  runScenario,
  scenarios,
} from "./manual-agent-loop.js";

const thisDir = path.dirname(fileURLToPath(import.meta.url));
const benchmarkPath = path.resolve(thisDir, "../benchmark-results.json");
const siteDir = path.resolve(thisDir, "../../../site");

function makeRunResult(label: string, overrides?: Partial<{
  roundTrips: number;
  toolInvocations: number;
  executeCalls: number;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
}>) {
  const executeTrace = [
    {
      toolName: "lookup_order",
      args: { order_id: "ORD-7291" },
      output: { id: "ORD-7291", customer_id: "CUST-8842", total: 129.97 },
    },
    {
      toolName: "send_notification",
      args: { recipient: "maya.chen@example.com", channel: "email" },
      output: { notification_id: "NOTIF-00001", recipient: "maya.chen@example.com" },
    },
  ];

  return {
    metrics: {
      roundTrips: overrides?.roundTrips ?? 2,
      totalToolInvocations: overrides?.toolInvocations ?? executeTrace.length,
      executeCallCount: overrides?.executeCalls ?? 1,
      inputTokens: overrides?.inputTokens ?? 1200,
      outputTokens: overrides?.outputTokens ?? 400,
      costUsd: overrides?.costUsd ?? 0.0031,
    },
    conversation: [
      { role: "user" as const, content: `${label} prompt` },
      {
        role: "assistant" as const,
        content: `${label} assistant`,
        toolCalls: [
          {
            name: "execute",
            input: {
              code: "order = tool('lookup_order', order_id='ORD-7291')\nnotification = tool('send_notification', recipient='maya.chen@example.com', channel='email', subject='ok', body='ok')\n{'done': True}",
            },
          },
        ],
      },
      {
        role: "tool" as const,
        toolResults: [
          {
            name: "execute",
            output: {
              label,
              recipient: "maya.chen@example.com",
            },
            trace: executeTrace,
          },
        ],
      },
    ],
    toolCallRecords: executeTrace,
  };
}

function makeEval(total: number, passed = total) {
  return {
    passed,
    failed: total - passed,
    total,
    score: total === 0 ? 0 : passed / total,
    results: Array.from({ length: total }, (_, index) => ({
      description: `criterion ${index + 1}`,
      criterion: `criterion ${index + 1}`,
      passed: index < passed,
      evidence: `evidence ${index + 1}`,
    })),
  };
}

function makeScenarioResult() {
  return {
    name: "Refund Pipeline",
    toolCount: 5,
    prompt: "Refund prompt",
    traditional: {
      key: "traditional" as const,
      label: "Traditional",
      run: makeRunResult("traditional", {
        roundTrips: 5,
        toolInvocations: 5,
        executeCalls: 0,
        inputTokens: 3200,
        outputTokens: 700,
        costUsd: 0.0092,
      }),
      eval: makeEval(5, 5),
      savingsVsTraditional: null,
    },
    executeOnly: {
      key: "execute-only" as const,
      label: "Execute-Only",
      run: makeRunResult("execute-only", {
        roundTrips: 2,
        toolInvocations: 5,
        executeCalls: 1,
        inputTokens: 1800,
        outputTokens: 500,
        costUsd: 0.0052,
      }),
      eval: makeEval(5, 4),
      savingsVsTraditional: {
        roundTripsPct: 60,
        toolInvocationsPct: 0,
        inputTokensPct: 44,
        outputTokensPct: 29,
        costPct: 43,
      },
    },
    hybrid: {
      key: "hybrid" as const,
      label: "Hybrid",
      run: makeRunResult("hybrid", {
        roundTrips: 2,
        toolInvocations: 5,
        executeCalls: 1,
        inputTokens: 1500,
        outputTokens: 450,
        costUsd: 0.0047,
      }),
      eval: makeEval(5, 5),
      savingsVsTraditional: {
        roundTripsPct: 60,
        toolInvocationsPct: 0,
        inputTokensPct: 53,
        outputTokensPct: 36,
        costPct: 49,
      },
    },
  };
}

describe("manual-agent-loop benchmark harness", () => {
  it("defaults zero-arg runs to all benchmark modes", () => {
    const originalArgv = process.argv;

    try {
      process.argv = ["node", "manual-agent-loop.ts"];
      expect(parseArgs()).toMatchObject({
        modes: ["traditional", "execute-only", "hybrid"],
        runs: 1,
        stateInjection: true,
      });
    } finally {
      process.argv = originalArgv;
    }
  });

  it("runs execute-only and hybrid independently and prints them separately", async () => {
    const callOrder: string[] = [];
    const runners = {
      traditional: vi.fn(async () => {
        callOrder.push("traditional");
        return makeRunResult("traditional", {
          roundTrips: 5,
        toolInvocations: 5,
        executeCalls: 0,
        inputTokens: 3000,
        outputTokens: 800,
        costUsd: 0.01,
      });
      }),
      montygate: vi.fn(async (_scenario, mode: "execute-only" | "hybrid") => {
        callOrder.push(mode);
        return makeRunResult(mode, {
          roundTrips: mode === "execute-only" ? 2 : 1,
          inputTokens: mode === "execute-only" ? 1800 : 1400,
          costUsd: mode === "execute-only" ? 0.005 : 0.004,
        });
      }),
      judge: vi.fn(async (_conversation, expectations) =>
        makeEval(expectations.length),
      ),
    };

    const { result } = await runScenario(
      scenarios[0],
      {
        modes: ["traditional", "execute-only", "hybrid"],
        runs: 1,
        stateInjection: true,
      },
      runners,
    );

    expect(callOrder).toEqual(["traditional", "execute-only", "hybrid"]);
    expect(result.executeOnly?.label).toBe("Execute-Only");
    expect(result.hybrid?.label).toBe("Hybrid");

    const lines: string[] = [];
    const spy = vi
      .spyOn(console, "log")
      .mockImplementation((...args) => lines.push(args.join(" ")));

    try {
      printComparisonTable(result, result.executeOnly!);
      printComparisonTable(result, result.hybrid!);
    } finally {
      spy.mockRestore();
    }

    const output = lines.join("\n");
    expect(output).toContain("Refund Pipeline (5 tools) — Execute-Only");
    expect(output).toContain("Refund Pipeline (5 tools) — Hybrid");
  });

  it("formats execute traces for the judge transcript", () => {
    const transcript = formatConversationForJudge([
      {
        role: "assistant",
        content: "Running execute",
        toolCalls: [
          {
            name: "execute",
            input: {
              code: "customer = tool('get_customer', customer_id='CUST-8842')\nnotification = tool('send_notification', recipient=customer['email'], channel='email', subject='ok', body='ok')\n{'done': True}",
            },
          },
        ],
      },
      {
        role: "tool",
        toolResults: [
          {
            name: "execute",
            output: { done: true },
            trace: [
              {
                toolName: "get_customer",
                args: { customer_id: "CUST-8842" },
                output: { id: "CUST-8842", email: "maya.chen@example.com" },
              },
              {
                toolName: "send_notification",
                args: { recipient: "maya.chen@example.com", channel: "email" },
                output: {
                  notification_id: "NOTIF-00001",
                  recipient: "maya.chen@example.com",
                },
              },
            ],
          },
        ],
      },
    ]);

    expect(transcript).toContain("[EXECUTE TRACE] get_customer");
    expect(transcript).toContain("[EXECUTE TRACE] send_notification");
    expect(transcript).toContain("maya.chen@example.com");
    expect(transcript).toContain("[EXECUTE RESULT - final script return value]");
  });

  it("serializes the new benchmark artifact shape", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-10T16:06:55.920Z"));

    try {
      const json = buildJsonOutput(
        [makeScenarioResult()],
        {
          modes: ["traditional", "execute-only", "hybrid"],
          runs: 1,
          stateInjection: true,
        },
      );

      expect({
        cost_scope: json.cost_scope,
        modes: json.modes,
        state_injection: json.state_injection,
        scenario_keys: Object.keys(json.scenarios[0] ?? {}),
        traditional_mode: json.scenarios[0]?.traditional?.mode,
        execute_only_mode: json.scenarios[0]?.execute_only?.mode,
        hybrid_mode: json.scenarios[0]?.hybrid?.mode,
        execute_only_trace_tools:
          json.scenarios[0]?.execute_only?.conversation[2]?.toolResults?.[0]?.trace?.map(
            (trace) => trace.toolName,
          ) ?? [],
        hybrid_trace_tools:
          json.scenarios[0]?.hybrid?.conversation[2]?.toolResults?.[0]?.trace?.map(
            (trace) => trace.toolName,
          ) ?? [],
        traditional_trace_tools:
          json.scenarios[0]?.traditional?.conversation[2]?.toolResults?.[0]?.trace?.map(
            (trace) => trace.toolName,
          ) ?? [],
        execute_only_savings_keys: Object.keys(
          json.scenarios[0]?.execute_only?.savings_vs_traditional ?? {},
        ),
      }).toMatchInlineSnapshot(`
        {
          "cost_scope": "agent_only_excludes_judge",
          "execute_only_mode": "execute-only",
          "execute_only_savings_keys": [
            "round_trips_pct",
            "tool_invocations_pct",
            "input_tokens_pct",
            "output_tokens_pct",
            "cost_pct",
          ],
          "execute_only_trace_tools": [
            "lookup_order",
            "send_notification",
          ],
          "hybrid_mode": "hybrid",
          "hybrid_trace_tools": [
            "lookup_order",
            "send_notification",
          ],
          "modes": [
            "traditional",
            "execute-only",
            "hybrid",
          ],
          "scenario_keys": [
            "name",
            "prompt",
            "tools",
            "traditional",
            "execute_only",
            "hybrid",
          ],
          "state_injection": true,
          "traditional_mode": "traditional",
          "traditional_trace_tools": [
            "lookup_order",
            "send_notification",
          ],
        }
      `);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe.sequential("site build regression", () => {
  it("builds the Astro site with the new benchmark artifact shape", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-10T16:06:55.920Z"));

    const existing = fs.existsSync(benchmarkPath)
      ? fs.readFileSync(benchmarkPath, "utf8")
      : null;

    try {
      const json = buildJsonOutput(
        [makeScenarioResult()],
        {
          modes: ["traditional", "execute-only", "hybrid"],
          runs: 1,
          stateInjection: true,
        },
      );
      fs.writeFileSync(benchmarkPath, `${JSON.stringify(json, null, 2)}\n`);
      execFileSync("pnpm", ["build"], {
        cwd: siteDir,
        env: { ...process.env, CI: "1" },
        stdio: "pipe",
      });
    } finally {
      if (existing === null) {
        fs.rmSync(benchmarkPath, { force: true });
      } else {
        fs.writeFileSync(benchmarkPath, existing);
      }
      vi.useRealTimers();
    }
  });
});
