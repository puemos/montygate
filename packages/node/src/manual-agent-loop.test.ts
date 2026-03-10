import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import {
  buildJudgePrompt,
  buildJsonOutput,
  computeMetricsStats,
  formatConversationForJudge,
  parseArgs,
  printComparisonTable,
  runScenario,
  scenarios,
  stdDev,
  TRADITIONAL_SYSTEM_PROMPT,
} from "./manual-agent-loop.js";

const thisDir = path.dirname(fileURLToPath(import.meta.url));
const benchmarkPath = path.resolve(thisDir, "../benchmark-results.json");
const siteDir = path.resolve(thisDir, "../../../site");

function makeRunResult(
  label: string,
  overrides?: Partial<{
    roundTrips: number;
    toolInvocations: number;
    executeCalls: number;
    inputTokens: number;
    outputTokens: number;
    costUsd: number;
    systemPromptTokens: number;
  }>,
) {
  const executeTrace = [
    {
      toolName: "lookup_order",
      args: { order_id: "ORD-7291" },
      output: { id: "ORD-7291", customer_id: "CUST-8842", total: 129.97 },
    },
    {
      toolName: "send_notification",
      args: { recipient: "maya.chen@example.com", channel: "email" },
      output: {
        notification_id: "NOTIF-00001",
        recipient: "maya.chen@example.com",
      },
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
      systemPromptTokens: overrides?.systemPromptTokens ?? 450,
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
    montygate: {
      key: "montygate" as const,
      label: "Montygate",
      run: makeRunResult("montygate", {
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
  it("defaults zero-arg runs to traditional vs Montygate with 3 runs", () => {
    const originalArgv = process.argv;

    try {
      process.argv = ["node", "manual-agent-loop.ts"];
      expect(parseArgs()).toMatchObject({
        runs: 3,
      });
    } finally {
      process.argv = originalArgv;
    }
  });

  it("rejects removed mode and state flags", () => {
    const originalArgv = process.argv;
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const exitSpy = vi
      .spyOn(process, "exit")
      .mockImplementation(((code?: number) => {
        throw new Error(`exit:${code ?? 0}`);
      }) as never);

    try {
      process.argv = [
        "node",
        "manual-agent-loop.ts",
        "--mode=traditional",
        "--no-state-injection",
      ];
      expect(() => parseArgs()).toThrow("exit:1");
      expect(errorSpy).toHaveBeenCalled();
    } finally {
      process.argv = originalArgv;
      errorSpy.mockRestore();
      exitSpy.mockRestore();
    }
  });

  it("runs traditional and montygate and prints one comparison table", async () => {
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
      montygate: vi.fn(async () => {
        callOrder.push("montygate");
        return makeRunResult("montygate", {
          roundTrips: 2,
          inputTokens: 1400,
          costUsd: 0.004,
        });
      }),
      judge: vi.fn(async (_conversation, expectations) =>
        makeEval(expectations.length),
      ),
    };

    const { result } = await runScenario(
      scenarios[0],
      {
        runs: 1,
      },
      runners,
    );

    expect(callOrder).toEqual(["traditional", "montygate"]);
    expect(result.montygate?.label).toBe("Montygate");

    const lines: string[] = [];
    const spy = vi
      .spyOn(console, "log")
      .mockImplementation((...args) => lines.push(args.join(" ")));

    try {
      printComparisonTable(result, result.montygate!);
    } finally {
      spy.mockRestore();
    }

    const output = lines.join("\n");
    expect(output).toContain("Refund Pipeline (5 tools) — Montygate");
    expect(output).not.toContain("Execute-Only");
    expect(output).not.toContain("Hybrid");
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

  it("judge prompt is mode-neutral and equally values both transcript formats", () => {
    const prompt = buildJudgePrompt(
      "Sample transcript",
      [{ description: "test", criterion: "test criterion" }],
    );

    // Should NOT prefer execute traces as "ACTUAL" or "primary evidence"
    expect(prompt).not.toMatch(/ACTUAL tool invocations/);
    expect(prompt).not.toMatch(/primary evidence/);

    // Should treat both formats as equivalent
    expect(prompt).toContain("[TOOL CALL]");
    expect(prompt).toContain("[EXECUTE TRACE]");
    expect(prompt).toContain("equivalent evidence");
    expect(prompt).toContain("Do not prefer one form");
  });

  it("TRADITIONAL_SYSTEM_PROMPT covers key strategic topics", () => {
    expect(TRADITIONAL_SYSTEM_PROMPT.length).toBeGreaterThan(300);
    expect(TRADITIONAL_SYSTEM_PROMPT).toContain("parallel");
    expect(TRADITIONAL_SYSTEM_PROMPT).toContain("round-trip");
    expect(TRADITIONAL_SYSTEM_PROMPT).toContain("Batch independent calls");
  });

  it("computes std dev correctly across multiple runs", () => {
    const runs = [
      {
        roundTrips: 2,
        totalToolInvocations: 5,
        executeCallCount: 1,
        inputTokens: 1200,
        outputTokens: 400,
        costUsd: 0.0031,
        systemPromptTokens: 450,
      },
      {
        roundTrips: 3,
        totalToolInvocations: 5,
        executeCallCount: 1,
        inputTokens: 1400,
        outputTokens: 420,
        costUsd: 0.0035,
        systemPromptTokens: 450,
      },
      {
        roundTrips: 2,
        totalToolInvocations: 5,
        executeCallCount: 1,
        inputTokens: 1100,
        outputTokens: 380,
        costUsd: 0.0029,
        systemPromptTokens: 450,
      },
    ];

    const stats = computeMetricsStats(runs);
    expect(stats.stdDev.roundTrips).toBeGreaterThan(0);
    expect(stats.stdDev.inputTokens).toBeGreaterThan(0);
    expect(stats.mean.roundTrips).toBeCloseTo(7 / 3, 1);
    expect(stats.mean.inputTokens).toBeCloseTo(3700 / 3, 0);
  });

  it("stdDev returns 0 for single value", () => {
    expect(stdDev([42])).toBe(0);
  });

  it("stdDev returns 0 for empty array", () => {
    expect(stdDev([])).toBe(0);
  });

  it("serializes the benchmark artifact with one Montygate variant", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-10T16:06:55.920Z"));

    try {
      const json = buildJsonOutput([makeScenarioResult()], { runs: 1 });

      expect({
        cost_scope: json.cost_scope,
        scenario_keys: Object.keys(json.scenarios[0] ?? {}),
        traditional_mode: json.scenarios[0]?.traditional?.mode,
        montygate_mode: json.scenarios[0]?.montygate?.mode,
        montygate_trace_tools:
          json.scenarios[0]?.montygate?.conversation[2]?.toolResults?.[0]?.trace?.map(
            (trace) => trace.toolName,
          ) ?? [],
        montygate_savings_keys: Object.keys(
          json.scenarios[0]?.montygate?.savings_vs_traditional ?? {},
        ),
      }).toMatchInlineSnapshot(`
        {
          "cost_scope": "agent_only_excludes_judge",
          "montygate_mode": "montygate",
          "montygate_savings_keys": [
            "round_trips_pct",
            "tool_invocations_pct",
            "input_tokens_pct",
            "output_tokens_pct",
            "cost_pct",
          ],
          "montygate_trace_tools": [
            "lookup_order",
            "send_notification",
          ],
          "scenario_keys": [
            "name",
            "prompt",
            "tools",
            "traditional",
            "montygate",
          ],
          "traditional_mode": "traditional",
        }
      `);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe.sequential("site build regression", () => {
  it("builds the Astro site with the benchmark artifact shape", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-10T16:06:55.920Z"));

    const existing = fs.existsSync(benchmarkPath)
      ? fs.readFileSync(benchmarkPath, "utf8")
      : null;

    try {
      const json = buildJsonOutput([makeScenarioResult()], { runs: 1 });
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
