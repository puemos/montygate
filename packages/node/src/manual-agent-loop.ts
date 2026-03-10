#!/usr/bin/env npx tsx

/**
 * Benchmark: Montygate vs Traditional tool use with a real LLM.
 *
 * Runs scenarios designed to highlight sequential tool dependencies,
 * conditional branching, and dynamic fan-out — patterns where Montygate
 * reduces LLM round-trips by orchestrating tool chains in a single
 * Python sandbox execution.
 *
 * FAIRNESS GUARANTEES:
 * This benchmark controls for structural biases to isolate the architectural
 * advantage of Montygate (sandbox) vs Traditional (multi-turn). Specific controls:
 *
 * 1. Unified token budget: both modes get MAX_TOKENS output budget (line 31)
 * 2. Equal system prompts: Traditional receives strategic guidance matching Montygate (line 35)
 * 3. System prompt overhead tracked: recorded in systemPromptTokens, subtracted from totals (RunMetrics)
 * 4. Judge prompt is mode-neutral: no preference for execute traces vs direct calls (buildJudgePrompt)
 * 5. Scenario balance: 11 scenarios mixing sequential-chain, parallel, and single-tool patterns
 * 6. Variance reporting: 3 default runs with mean ± σ (see parseArgs, computeMetricsStats)
 *
 * Outputs:
 *   - Terminal: side-by-side conversation logs + comparison tables with variance
 *   - JSON:    benchmark-results.json with full conversations for landing page
 *
 * Run from packages/node/:
 *   ANTHROPIC_API_KEY=sk-... npx tsx src/manual-agent-loop.ts
 *   ANTHROPIC_API_KEY=sk-... npx tsx src/manual-agent-loop.ts --runs=3  (with variance)
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import Anthropic from "@anthropic-ai/sdk";
import { z } from "zod";
import { Montygate } from "./index.js";

// ── Config ──────────────────────────────────────────────────────────────────

const DEFAULT_MODEL = "claude-haiku-4-5-20251001";
const JUDGE_MODEL = "claude-haiku-4-5-20251001";
let MODEL = DEFAULT_MODEL;

// FAIRNESS: Both modes use the same output token budget.
// Montygate's system prompt + tool schema adds ~600-700 input tokens;
// giving Traditional only 2048 would silently truncate its final answer on complex tasks.
const MAX_TOKENS = 4096;
const MAX_TURNS = 15;

// Simulate realistic API/DB latency per tool call (ms)
const SIMULATED_TOOL_LATENCY_MS = 80;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// Pricing per million tokens by model
const MODEL_PRICING: Record<string, { input: number; output: number }> = {
  "claude-haiku-4-5-20251001": { input: 0.8, output: 4.0 },
  "claude-sonnet-4-6": { input: 3.0, output: 15.0 },
};

// FAIRNESS: Traditional receives equivalent strategic guidance to Montygate.
// The ONLY intended difference between modes is architectural.
export const TRADITIONAL_SYSTEM_PROMPT = `You are an expert assistant that calls tools efficiently.

Strategic guidance:
- Gather ALL information you need in the fewest possible round-trips.
- When tool calls are INDEPENDENT of each other, issue them in the SAME response (parallel tool calls). Never wait for a result you don't need yet.
- Think ahead: read the full task, identify every tool call needed, and issue all independent calls in the first response.
- Chain results: after each round-trip, immediately issue the next batch of dependent calls. Never return to the user before all required actions are complete.
- Fan-out: for repeated operations over a list, issue ALL calls in one response.
- Conditional logic: read the result, decide in one reasoning step, then issue all consequent calls.
- Complete the ENTIRE task end-to-end before answering. Never stop mid-task.

Efficiency rules:
- Never call a tool you already have the result for.
- Never call a tool irrelevant to the task.
- Batch independent calls — 3 in one response is always better than 3 round-trips.
- Complete both data-gathering and action steps in the same run.`;

function estimateCost(inputTokens: number, outputTokens: number): number {
  const pricing = MODEL_PRICING[MODEL] ?? MODEL_PRICING[DEFAULT_MODEL];
  return (
    (inputTokens / 1_000_000) * pricing.input +
    (outputTokens / 1_000_000) * pricing.output
  );
}

// ── Types ───────────────────────────────────────────────────────────────────

interface RunMetrics {
  roundTrips: number;
  totalToolInvocations: number;
  executeCallCount: number;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  // FAIRNESS: input tokens attributable to the system prompt (first-turn proxy).
  // Subtract to get "net agent reasoning tokens."
  systemPromptTokens: number;
}

interface ConversationEntry {
  role: "user" | "assistant" | "tool";
  content?: string;
  toolCalls?: Array<{ name: string; input: unknown }>;
  toolResults?: Array<{
    name: string;
    output: unknown;
    isError?: boolean;
    trace?: ToolCallRecord[];
  }>;
}

interface AgentRunResult {
  metrics: RunMetrics;
  conversation: ConversationEntry[];
  toolCallRecords: ToolCallRecord[];
}

interface ToolCallRecord {
  toolName: string;
  args: Record<string, unknown>;
  output?: unknown;
  error?: string;
}

type BenchmarkMode = "traditional" | "montygate";

interface SavingsSummary {
  roundTripsPct: number;
  toolInvocationsPct: number;
  inputTokensPct: number;
  outputTokensPct: number;
  costPct: number;
}

interface MetricsStats {
  mean: {
    roundTrips: number;
    totalToolInvocations: number;
    executeCallCount: number;
    inputTokens: number;
    outputTokens: number;
    systemPromptTokens: number;
    costUsd: number;
    netReasoningTokens: number;
    netReasoningCostUsd: number;
  };
  stdDev: {
    roundTrips: number;
    totalToolInvocations: number;
    executeCallCount: number;
    inputTokens: number;
    outputTokens: number;
    systemPromptTokens: number;
    costUsd: number;
    netReasoningTokens: number;
    netReasoningCostUsd: number;
  };
}

interface ScenarioModeResult {
  key: BenchmarkMode;
  label: string;
  run: AgentRunResult;
  eval: EvalResult;
  savingsVsTraditional: SavingsSummary | null;
  metricsStats?: MetricsStats;
}

interface ScenarioResult {
  name: string;
  toolCount: number;
  prompt: string;
  traditional: ScenarioModeResult | null;
  montygate: ScenarioModeResult | null;
}

interface OutcomeExpectation {
  description: string;
  criterion: string;
}

interface OutcomeResult {
  description: string;
  criterion: string;
  passed: boolean;
  evidence: string;
}

interface EvalResult {
  passed: number;
  failed: number;
  total: number;
  score: number;
  results: OutcomeResult[];
}

interface RunBenchmarkOptions {
  runs: number;
}

interface BenchmarkArgs extends RunBenchmarkOptions {}

interface ScenarioRunners {
  traditional: (
    scenario: { prompt: string; tools: ToolDef[] },
  ) => Promise<AgentRunResult>;
  montygate: (
    scenario: { prompt: string; tools: ToolDef[] },
  ) => Promise<AgentRunResult>;
  judge: (
    conversation: ConversationEntry[],
    expectations: OutcomeExpectation[],
  ) => Promise<EvalResult>;
}

interface ToolDef {
  name: string;
  description: string;
  params: z.ZodObject<z.ZodRawShape>;
  returns?: z.ZodType;
  run: (args: Record<string, unknown>) => Promise<unknown>;
}

interface ScenarioDef {
  name: string;
  prompt: string;
  toolNames: string[];
  expectations: OutcomeExpectation[];
}

const orderFixtures = {
  "ORD-7291": {
    id: "ORD-7291",
    status: "shipped",
    customer_id: "CUST-8842",
    items: [
      { sku: "SKU-W100", name: "Wireless Headphones", price: 79.99 },
      { sku: "SKU-C200", name: "USB-C Cable Pack", price: 14.99 },
      { sku: "SKU-P300", name: "Phone Stand Pro", price: 34.99 },
    ],
    total: 129.97,
    tracking_id: "TRK-55821",
    order_date: "2026-02-15",
  },
  "ORD-7104": {
    id: "ORD-7104",
    status: "delivered",
    customer_id: "CUST-8842",
    items: [
      { sku: "SKU-B110", name: "Portable Charger", price: 39.5 },
      { sku: "SKU-C210", name: "USB-C Cable Twin Pack", price: 25.0 },
    ],
    total: 64.5,
    tracking_id: "TRK-55104",
    order_date: "2026-01-28",
  },
  "ORD-6980": {
    id: "ORD-6980",
    status: "delivered",
    customer_id: "CUST-8842",
    items: [
      { sku: "SKU-E500", name: "Noise-Cancelling Earbuds", price: 159.0 },
      { sku: "SKU-CC90", name: "Travel Case", price: 60.0 },
    ],
    total: 219.0,
    tracking_id: "TRK-56980",
    order_date: "2026-01-10",
  },
  "ORD-6755": {
    id: "ORD-6755",
    status: "returned",
    customer_id: "CUST-8842",
    items: [
      { sku: "SKU-S010", name: "Screen Cleaner Kit", price: 12.0 },
      { sku: "SKU-CO15", name: "Cable Organizer", price: 15.0 },
      { sku: "SKU-PG15", name: "Phone Grip", price: 15.0 },
    ],
    total: 42.0,
    tracking_id: "TRK-56755",
    order_date: "2025-12-05",
  },
  // ── Second customer orders ──
  "ORD-9010": {
    id: "ORD-9010",
    status: "delivered",
    customer_id: "CUST-2244",
    items: [
      { sku: "SKU-L700", name: "Laptop Stand Deluxe", price: 89.99 },
      { sku: "SKU-K800", name: "Mechanical Keyboard", price: 149.0 },
      { sku: "SKU-M900", name: "Ergonomic Mouse", price: 69.99 },
    ],
    total: 308.98,
    tracking_id: "TRK-60010",
    order_date: "2026-02-20",
  },
  "ORD-9011": {
    id: "ORD-9011",
    status: "shipped",
    customer_id: "CUST-2244",
    items: [
      { sku: "SKU-H400", name: "USB Hub 7-Port", price: 45.0 },
      { sku: "SKU-D500", name: "Docking Station", price: 199.0 },
    ],
    total: 244.0,
    tracking_id: "TRK-60011",
    order_date: "2026-03-01",
  },
  "ORD-9012": {
    id: "ORD-9012",
    status: "processing",
    customer_id: "CUST-2244",
    items: [
      { sku: "SKU-W100", name: "Wireless Headphones", price: 79.99 },
      { sku: "SKU-E500", name: "Noise-Cancelling Earbuds", price: 159.0 },
      { sku: "SKU-C200", name: "USB-C Cable Pack", price: 14.99 },
    ],
    total: 253.98,
    tracking_id: "TRK-60012",
    order_date: "2026-03-05",
  },
  "ORD-9013": {
    id: "ORD-9013",
    status: "returned",
    customer_id: "CUST-2244",
    items: [
      { sku: "SKU-M900", name: "Ergonomic Mouse", price: 69.99 },
      { sku: "SKU-P300", name: "Phone Stand Pro", price: 34.99 },
    ],
    total: 104.98,
    tracking_id: "TRK-60013",
    order_date: "2026-01-15",
  },
  // ── Third customer orders ──
  "ORD-5501": {
    id: "ORD-5501",
    status: "delivered",
    customer_id: "CUST-5501",
    items: [
      { sku: "SKU-K800", name: "Mechanical Keyboard", price: 149.0 },
      { sku: "SKU-W100", name: "Wireless Headphones", price: 79.99 },
    ],
    total: 228.99,
    tracking_id: "TRK-70501",
    order_date: "2026-02-10",
  },
  "ORD-5502": {
    id: "ORD-5502",
    status: "shipped",
    customer_id: "CUST-5501",
    items: [
      { sku: "SKU-D500", name: "Docking Station", price: 199.0 },
      { sku: "SKU-H400", name: "USB Hub 7-Port", price: 45.0 },
      { sku: "SKU-C200", name: "USB-C Cable Pack", price: 14.99 },
    ],
    total: 258.99,
    tracking_id: "TRK-70502",
    order_date: "2026-02-28",
  },
} as const;

const customerOrderIds: Record<string, string[]> = {
  "CUST-8842": ["ORD-7291", "ORD-7104", "ORD-6980", "ORD-6755"],
  "CUST-2244": ["ORD-9010", "ORD-9011", "ORD-9012", "ORD-9013"],
  "CUST-5501": ["ORD-5501", "ORD-5502"],
};

const customerFixtures = {
  "CUST-8842": {
    id: "CUST-8842",
    name: "Maya Chen",
    email: "maya.chen@example.com",
    tier: "gold",
    loyalty_points: 4820,
    account_status: "active",
    member_since: "2024-03-10",
  },
  "CUST-2244": {
    id: "CUST-2244",
    name: "James Rivera",
    email: "j.rivera@example.com",
    tier: "silver",
    loyalty_points: 1250,
    account_status: "active",
    member_since: "2025-01-22",
  },
  "CUST-5501": {
    id: "CUST-5501",
    name: "Priya Sharma",
    email: "priya.s@example.com",
    tier: "bronze",
    loyalty_points: 380,
    account_status: "active",
    member_since: "2025-11-05",
  },
} as const;

const inventoryFixtures = {
  "SKU-W100": {
    sku: "SKU-W100",
    available: 42,
    warehouse: "WH-EAST",
    restock_date: null,
  },
  "SKU-C200": {
    sku: "SKU-C200",
    available: 0,
    warehouse: "WH-WEST",
    restock_date: "2026-03-20",
  },
  "SKU-P300": {
    sku: "SKU-P300",
    available: 15,
    warehouse: "WH-WEST",
    restock_date: null,
  },
  "SKU-L700": {
    sku: "SKU-L700",
    available: 8,
    warehouse: "WH-EAST",
    restock_date: null,
  },
  "SKU-K800": {
    sku: "SKU-K800",
    available: 0,
    warehouse: "WH-EAST",
    restock_date: "2026-03-25",
  },
  "SKU-M900": {
    sku: "SKU-M900",
    available: 23,
    warehouse: "WH-WEST",
    restock_date: null,
  },
  "SKU-H400": {
    sku: "SKU-H400",
    available: 0,
    warehouse: "WH-WEST",
    restock_date: "2026-04-01",
  },
  "SKU-D500": {
    sku: "SKU-D500",
    available: 3,
    warehouse: "WH-EAST",
    restock_date: null,
  },
} as const;

interface IdCounters {
  refund: number;
  case: number;
  escalation: number;
  notification: number;
  credit: number;
  callback: number;
}

function createIdCounters(): IdCounters {
  return { refund: 0, case: 0, escalation: 0, notification: 0, credit: 0, callback: 0 };
}

function cloneOrder(orderId: string) {
  const order = orderFixtures[orderId as keyof typeof orderFixtures];
  if (!order) {
    throw new Error(`Unknown order_id: ${orderId}`);
  }
  return {
    ...order,
    items: order.items.map((item) => ({ ...item })),
  };
}

function cloneCustomer(customerId: string) {
  const customer =
    customerFixtures[customerId as keyof typeof customerFixtures];
  if (!customer) {
    throw new Error(`Unknown customer_id: ${customerId}`);
  }
  return { ...customer };
}

function buildOrderHistory(customerId: string) {
  const orderIds = customerOrderIds[customerId] ?? [];
  return {
    customer_id: customerId,
    orders: orderIds.map((orderId) => {
      const order = orderFixtures[orderId as keyof typeof orderFixtures];
      return {
        id: order.id,
        date: order.order_date,
        total: order.total,
        status: order.status,
      };
    }),
  };
}

function cloneInventoryItem(sku: string) {
  const item = inventoryFixtures[sku as keyof typeof inventoryFixtures];
  if (!item) {
    return {
      sku,
      available: 0,
      warehouse: "WH-UNKNOWN",
      restock_date: null,
    };
  }
  return { ...item };
}

function nextId(counters: IdCounters, kind: keyof IdCounters, prefix: string): string {
  counters[kind] += 1;
  return `${prefix}-${String(counters[kind]).padStart(5, "0")}`;
}

// ── Eval Framework (LLM-as-Judge) ──────────────────────────────────────────

function formatConversationForJudge(conversation: ConversationEntry[]): string {
  const parts: string[] = [];
  for (const entry of conversation) {
    if (entry.role === "user") {
      parts.push(`[USER]\n${entry.content ?? ""}`);
    } else if (entry.role === "assistant") {
      let text = entry.content ?? "";
      if (entry.toolCalls) {
        for (const tc of entry.toolCalls) {
          if (tc.name === "execute") {
            const code = (tc.input as Record<string, unknown>)?.code ?? "";
            text += `\n[EXECUTE SCRIPT - contains tool() calls inside sandbox]\n${code}`;
          } else {
            text += `\n[TOOL CALL] ${tc.name}(${JSON.stringify(tc.input)})`;
          }
        }
      }
      parts.push(`[ASSISTANT]\n${text}`);
    } else if (entry.role === "tool") {
      if (entry.toolResults) {
        for (const tr of entry.toolResults) {
          if (tr.name === "execute") {
            const prefix = tr.isError
              ? "[EXECUTE ERROR]"
              : "[EXECUTE RESULT - final script return value]";
            parts.push(`${prefix} => ${JSON.stringify(tr.output)}`);
            if (tr.trace) {
              for (const trace of tr.trace) {
                const tracePrefix = trace.error
                  ? "[EXECUTE TRACE ERROR]"
                  : "[EXECUTE TRACE]";
                const traceOutput = trace.error ?? trace.output;
                parts.push(
                  `${tracePrefix} ${trace.toolName}(${JSON.stringify(trace.args)}) => ${JSON.stringify(traceOutput)}`,
                );
              }
            }
          } else {
            const prefix = tr.isError ? "[TOOL ERROR]" : "[TOOL RESULT]";
            parts.push(`${prefix} ${tr.name} => ${JSON.stringify(tr.output)}`);
          }
        }
      }
    }
  }
  return parts.join("\n\n");
}

function buildJudgePrompt(
  transcript: string,
  expectations: OutcomeExpectation[],
): string {
  const criteriaList = expectations
    .map((e, i) => `${i + 1}. ${e.criterion}`)
    .join("\n");

  return `You are evaluating whether an AI agent completed a task correctly.

Here is the full conversation transcript:
${transcript}

FAIRNESS: judge prompt must give equal evidential weight to both transcript formats.
Reading the transcript:
- [TOOL CALL] name(args) and [TOOL RESULT] name => output — direct tool invocations.
- [EXECUTE SCRIPT] — a Python script submitted to a sandbox executor.
- [EXECUTE TRACE] name(args) => output — a tool invoked from inside the sandbox script.

Treat [TOOL CALL]/[TOOL RESULT] and [EXECUTE TRACE] entries as equivalent evidence.
Do not prefer one form over the other. Both formats represent actual tool invocations.

Evaluation rules:
- Evaluate each criterion INDEPENDENTLY based on concrete evidence in the transcript.
- For criteria about actions taken: look for [TOOL CALL], [TOOL RESULT], or [EXECUTE TRACE] evidence.
- For criteria about actions NOT taken (e.g. "no case created"): verify there is NO matching tool call or trace entry. Pass only if there is clear absence of the action.
- For numeric criteria (amounts, counts): check the actual values in tool call arguments and results.
${criteriaList}

Return ONLY a JSON array (no other text, no markdown fences). Keep evidence strings simple without nested JSON:
[
  {"index": 0, "passed": true, "evidence": "Action completed successfully"},
  {"index": 1, "passed": false, "evidence": "Condition not met"}
]
Each entry must have "index" (0-based number), "passed" (boolean true/false), and "evidence" (string). Avoid quotes inside evidence strings.`;
}

async function judgeOutcomes(
  conversation: ConversationEntry[],
  expectations: OutcomeExpectation[],
): Promise<EvalResult> {
  const transcript = formatConversationForJudge(conversation);

  const response = await client.messages.create({
    model: JUDGE_MODEL,
    max_tokens: 2048,
    temperature: 0,
    messages: [
      {
        role: "user",
        content: buildJudgePrompt(transcript, expectations),
      },
    ],
  });

  const text = extractText(response.content);

  let judgeResults: Array<{
    index: number;
    passed: boolean;
    evidence: string;
  }> = [];

  try {
    const jsonMatch = text.match(/\[[\s\S]*\]/);
    if (jsonMatch) {
      judgeResults = JSON.parse(jsonMatch[0]);
    }
  } catch (e) {
    console.error("  [judge] Failed to parse JSON:", e);
  }

  const results: OutcomeResult[] = expectations.map((e, i) => {
    const jr = judgeResults.find((r) => r.index === i);
    return {
      description: e.description,
      criterion: e.criterion,
      passed: jr?.passed ?? false,
      evidence: jr?.evidence ?? "No evidence provided",
    };
  });

  const passed = results.filter((r) => r.passed).length;
  const failed = results.filter((r) => !r.passed).length;
  return {
    passed,
    failed,
    total: results.length,
    score: results.length > 0 ? passed / results.length : 1,
    results,
  };
}

function extractTraditionalToolCalls(
  conversation: ConversationEntry[],
): ToolCallRecord[] {
  const records: ToolCallRecord[] = [];
  for (let i = 0; i < conversation.length; i++) {
    const entry = conversation[i];
    if (entry.role === "assistant" && entry.toolCalls) {
      const nextEntry = conversation[i + 1];
      const toolResults =
        nextEntry?.role === "tool" ? nextEntry.toolResults : undefined;
      for (let j = 0; j < entry.toolCalls.length; j++) {
        const tc = entry.toolCalls[j];
        const tr = toolResults?.[j];
        records.push({
          toolName: tc.name,
          args: (tc.input as Record<string, unknown>) ?? {},
          output: tr?.output,
          error: tr?.isError ? String(tr.output) : undefined,
        });
      }
    }
  }
  return records;
}

// ── Tool Definitions ────────────────────────────────────────────────────────
//
// Designed so that each tool's OUTPUT contains fields required as INPUT
// by downstream tools. This forces sequential LLM round-trips in the
// traditional approach (the LLM can't call tool B until tool A returns).

function createAllTools(counters: IdCounters): Record<string, ToolDef> {
  return {
    lookup_order: {
      name: "lookup_order",
      description:
        "Look up order details by order ID. Returns order status, associated customer ID, line items with SKUs and prices, total amount, and tracking info.",
      params: z.object({
        order_id: z.string().describe("The order ID, e.g. ORD-1234"),
      }),
      returns: z.object({
        id: z.string(),
        status: z.string(),
        customer_id: z.string(),
        items: z.array(
          z.object({
            sku: z.string(),
            name: z.string(),
            price: z.number(),
          }),
        ),
        total: z.number(),
        tracking_id: z.string(),
        order_date: z.string(),
      }),
      run: async ({ order_id }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return cloneOrder(order_id as string);
      },
    },

    get_customer: {
      name: "get_customer",
      description:
        "Get customer profile by customer ID. Returns name, email, membership tier, loyalty points, and account status.",
      params: z.object({
        customer_id: z
          .string()
          .describe("Customer ID returned from order lookup, e.g. CUST-1001"),
      }),
      returns: z.object({
        id: z.string(),
        name: z.string(),
        email: z.string(),
        tier: z.string(),
        loyalty_points: z.number(),
        account_status: z.string(),
        member_since: z.string(),
      }),
      run: async ({ customer_id }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return cloneCustomer(customer_id as string);
      },
    },

    get_order_history: {
      name: "get_order_history",
      description:
        "Get a customer's order history. Returns a list of recent orders with IDs, dates, totals, and statuses.",
      params: z.object({
        customer_id: z.string().describe("Customer ID"),
      }),
      returns: z.object({
        customer_id: z.string(),
        orders: z.array(
          z.object({
            id: z.string(),
            date: z.string(),
            total: z.number(),
            status: z.string(),
          }),
        ),
      }),
      run: async ({ customer_id }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return buildOrderHistory(customer_id as string);
      },
    },

    check_refund_eligibility: {
      name: "check_refund_eligibility",
      description:
        "Check whether an order qualifies for a refund based on the customer's membership tier. Returns eligibility status, maximum refund percentage, and the policy ID to use when processing.",
      params: z.object({
        order_id: z.string().describe("Order ID to check"),
        customer_tier: z
          .string()
          .describe("Customer membership tier (e.g. 'gold', 'silver', 'bronze')"),
      }),
      returns: z.object({
        eligible: z.boolean(),
        max_refund_pct: z.number(),
        policy_id: z.string(),
        reason: z.string(),
      }),
      run: async ({ order_id: _order_id, customer_tier }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        const pct =
          customer_tier === "gold" ? 100 : customer_tier === "silver" ? 80 : 60;
        return {
          eligible: true,
          max_refund_pct: pct,
          policy_id: `POL-${pct}-2026`,
          reason:
            pct === 100
              ? "Gold members qualify for full refund within 60 days"
              : `${customer_tier} tier qualifies for ${pct}% refund within 30 days`,
        };
      },
    },

    process_refund: {
      name: "process_refund",
      description:
        "Process a refund for an order. Requires the order ID, dollar amount, and policy ID from the eligibility check.",
      params: z.object({
        order_id: z.string().describe("Order ID to refund"),
        amount: z.number().describe("Refund amount in USD"),
        policy_id: z
          .string()
          .describe("Policy ID from check_refund_eligibility result"),
      }),
      returns: z.object({
        refund_id: z.string(),
        order_id: z.string(),
        amount: z.number(),
        status: z.string(),
        estimated_credit_date: z.string(),
      }),
      run: async ({ order_id, amount, policy_id }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return {
          refund_id: nextId(counters, "refund", "REF"),
          order_id: order_id as string,
          amount: amount as number,
          policy_id: policy_id as string,
          status: "processed",
          estimated_credit_date: "2026-03-14",
        };
      },
    },

    assess_risk: {
      name: "assess_risk",
      description:
        "Assess fraud risk for a customer transaction. Takes the customer ID, order total, and loyalty points to compute a risk score and level.",
      params: z.object({
        customer_id: z.string().describe("Customer ID"),
        order_total: z.number().describe("Total order amount in USD"),
        loyalty_points: z
          .number()
          .describe("Customer's loyalty points from their profile"),
      }),
      returns: z.object({
        risk_level: z.string(),
        score: z.number(),
        recommended_action: z.string(),
      }),
      run: async ({ customer_id: _customer_id, order_total, loyalty_points }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        const pts = loyalty_points as number;
        const total = order_total as number;
        const score = Math.max(
          0,
          Math.min(100, Math.round(total / 2 - pts / 100)),
        );
        const level = score > 70 ? "high" : score > 40 ? "medium" : "low";
        return {
          risk_level: level,
          score,
          recommended_action:
            level === "high"
              ? "Immediate review required — escalate to fraud team"
              : level === "medium"
                ? "Flag for periodic review"
                : "No action needed",
        };
      },
    },

    create_case: {
      name: "create_case",
      description:
        "Create a support case. Returns the case ID, assigned team, and SLA hours.",
      params: z.object({
        customer_id: z.string().describe("Customer ID"),
        order_id: z.string().describe("Related order ID"),
        category: z
          .string()
          .describe("Case category, e.g. 'refund', 'fraud', 'complaint'"),
        priority: z.enum(["low", "medium", "high"]).describe("Case priority"),
        summary: z.string().describe("Summary of the case"),
      }),
      returns: z.object({
        case_id: z.string(),
        assigned_to: z.string(),
        sla_hours: z.number(),
        status: z.string(),
      }),
      run: async ({ priority }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return {
          case_id: nextId(counters, "case", "CASE"),
          assigned_to: priority === "high" ? "fraud-team-alpha" : "support-team-3",
          sla_hours: priority === "high" ? 4 : priority === "medium" ? 24 : 72,
          status: "open",
          created_at: new Date().toISOString(),
        };
      },
    },

    escalate_case: {
      name: "escalate_case",
      description:
        "Escalate an existing support case to a senior team. Requires the case ID from create_case.",
      params: z.object({
        case_id: z.string().describe("Case ID from create_case result"),
        reason: z.string().describe("Reason for escalation"),
        urgency: z
          .enum(["normal", "urgent", "critical"])
          .describe("Urgency level"),
      }),
      returns: z.object({
        escalation_id: z.string(),
        escalated_to: z.string(),
        response_eta: z.string(),
      }),
      run: async ({ case_id, urgency }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return {
          escalation_id: nextId(counters, "escalation", "ESC"),
          case_id: case_id as string,
          escalated_to:
            urgency === "critical" ? "vp-operations" : "senior-support-lead",
          response_eta: urgency === "critical" ? "30 minutes" : "2 hours",
          status: "escalated",
        };
      },
    },

    check_inventory: {
      name: "check_inventory",
      description:
        "Check inventory levels for product SKUs. Returns stock availability per warehouse.",
      params: z.object({
        sku_list: z
          .array(z.string())
          .describe("List of SKU IDs to check, e.g. ['SKU-A100', 'SKU-B200']"),
      }),
      returns: z.object({
        items: z.array(
          z.object({
            sku: z.string(),
            available: z.number(),
            warehouse: z.string(),
            restock_date: z.string().nullable(),
          }),
        ),
      }),
      run: async ({ sku_list }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return {
          items: (sku_list as string[]).map((sku) => cloneInventoryItem(sku)),
        };
      },
    },

    send_notification: {
      name: "send_notification",
      description:
        "Send a notification via email or SMS. Returns delivery confirmation.",
      params: z.object({
        channel: z.enum(["email", "sms"]).describe("Notification channel"),
        recipient: z.string().describe("Email address or phone number"),
        subject: z.string().describe("Notification subject"),
        body: z.string().describe("Notification body text"),
      }),
      returns: z.object({
        notification_id: z.string(),
        channel: z.string(),
        sent_at: z.string(),
        status: z.string(),
      }),
      run: async ({ channel, recipient }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return {
          notification_id: nextId(counters, "notification", "NOTIF"),
          channel: channel as string,
          recipient: recipient as string,
          sent_at: new Date().toISOString(),
          status: "delivered",
        };
      },
    },

    apply_store_credit: {
      name: "apply_store_credit",
      description:
        "Apply store credit to a customer's account. Returns the credit ID, new balance, and expiration date.",
      params: z.object({
        customer_id: z.string().describe("Customer ID to credit"),
        amount: z.number().describe("Credit amount in USD"),
        reason: z.string().describe("Reason for the credit"),
      }),
      returns: z.object({
        credit_id: z.string(),
        customer_id: z.string(),
        amount: z.number(),
        new_balance: z.number(),
        expires_at: z.string(),
      }),
      run: async ({ customer_id, amount }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return {
          credit_id: nextId(counters, "credit", "CRED"),
          customer_id: customer_id as string,
          amount: amount as number,
          new_balance: (amount as number) + 25.0,
          expires_at: "2027-03-08",
        };
      },
    },

    schedule_callback: {
      name: "schedule_callback",
      description:
        "Schedule a callback from a support agent to the customer. Returns the callback ID and scheduled time.",
      params: z.object({
        customer_id: z.string().describe("Customer ID"),
        phone: z.string().describe("Phone number to call"),
        topic: z.string().describe("Topic of the callback"),
        priority: z
          .enum(["normal", "high", "urgent"])
          .describe("Callback priority"),
      }),
      returns: z.object({
        callback_id: z.string(),
        scheduled_for: z.string(),
        agent_team: z.string(),
      }),
      run: async ({ customer_id, priority }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        return {
          callback_id: nextId(counters, "callback", "CB"),
          customer_id: customer_id as string,
          scheduled_for:
            priority === "urgent"
              ? "2026-03-08T10:00:00Z"
              : priority === "high"
                ? "2026-03-08T14:00:00Z"
                : "2026-03-09T10:00:00Z",
          agent_team:
            priority === "urgent" ? "senior-resolution" : "general-support",
        };
      },
    },

    days_since: {
      name: "days_since",
      description:
        "Calculate the number of days between a given date (YYYY-MM-DD) and today. Returns {days: number}. Use this for any date comparisons.",
      params: z.object({
        date: z.string().describe("Date in YYYY-MM-DD format"),
      }),
      returns: z.object({
        days: z.number().describe("Number of days since the given date"),
      }),
      run: async ({ date }) => {
        await sleep(SIMULATED_TOOL_LATENCY_MS);
        const then = new Date(date as string);
        const now = new Date();
        return { days: Math.floor((now.getTime() - then.getTime()) / 86400000) };
      },
    },
  };
}

// ── Scenarios ───────────────────────────────────────────────────────────────

const scenarios: ScenarioDef[] = [
  {
    name: "Refund Pipeline",
    prompt:
      "I need a refund for order ORD-7291. " +
      "Give me the max I'm eligible for and email me the confirmation.",
    toolNames: [
      "lookup_order",
      "get_customer",
      "check_refund_eligibility",
      "process_refund",
      "send_notification",
    ],
    expectations: [
      {
        description: "Order ORD-7291 looked up",
        criterion: "Order ORD-7291 was looked up and its details (items, total, customer ID) were used in the process",
      },
      {
        description: "Customer profile retrieved",
        criterion: "The customer profile for the order's owner (CUST-8842 / Maya Chen) was retrieved",
      },
      {
        description: "Gold tier eligibility used",
        criterion: "The refund eligibility check used the customer's gold membership tier, resulting in 100% refund eligibility",
      },
      {
        description: "Refund of $129.97 processed",
        criterion: "A refund of $129.97 (the full order total) was processed for order ORD-7291",
      },
      {
        description: "Confirmation email sent",
        criterion: "A confirmation email was sent to maya.chen@example.com about the refund",
      },
    ],
  },
  {
    name: "Batch Order Review",
    prompt:
      "CUST-8842 is asking for refunds on all their orders over $50. " +
      "Process whatever qualifies and send them a summary email.",
    toolNames: [
      "get_customer",
      "get_order_history",
      "lookup_order",
      "check_refund_eligibility",
      "process_refund",
      "send_notification",
    ],
    expectations: [
      {
        description: "Customer CUST-8842 identified",
        criterion: "Customer CUST-8842 (Maya Chen) was looked up and their gold membership tier was identified",
      },
      {
        description: "Order history retrieved",
        criterion: "The customer's order history was retrieved, showing orders ORD-7291, ORD-7104, ORD-6980, and ORD-6755",
      },
      {
        description: "Refund for ORD-7291 ($129.97)",
        criterion: "A refund was processed for order ORD-7291 with amount $129.97 (over $50, qualifies)",
      },
      {
        description: "Refund for ORD-7104 ($64.50)",
        criterion: "A refund was processed for order ORD-7104 with amount $64.50 (over $50, qualifies)",
      },
      {
        description: "Refund for ORD-6980 ($219.00)",
        criterion: "A refund was processed for order ORD-6980 with amount $219.00 (over $50, qualifies)",
      },
      {
        description: "ORD-6755 excluded (under $50)",
        criterion: "Order ORD-6755 ($42.00) was NOT refunded because its total is under $50. Exactly 3 refunds were processed total.",
      },
      {
        description: "Summary email sent",
        criterion: "A summary notification/email was sent to the customer about the refund results",
      },
    ],
  },
  {
    name: "Cross-Customer Fraud Investigation",
    prompt:
      "Orders ORD-7291, ORD-9010, and ORD-5501 might be linked to a fraud ring. " +
      "Look into all three — pull customer info, run risk assessment, " +
      "open fraud cases for each, and escalate anything that scores high. " +
      "Email a summary to fraud-team@example.com.",
    toolNames: [
      "lookup_order",
      "get_customer",
      "assess_risk",
      "create_case",
      "escalate_case",
      "check_inventory",
      "send_notification",
    ],
    expectations: [
      {
        description: "All 3 orders investigated",
        criterion: "All three orders (ORD-7291, ORD-9010, ORD-5501) were looked up and their details retrieved",
      },
      {
        description: "All 3 customers identified",
        criterion: "Customer profiles were retrieved for all three order owners: CUST-8842 (Maya Chen), CUST-2244 (James Rivera), and CUST-5501 (Priya Sharma)",
      },
      {
        description: "Risk assessed for all 3",
        criterion: "Risk assessment was performed for all three customers/orders",
      },
      {
        description: "Fraud cases created for all 3",
        criterion: "Fraud/support cases were created for all three orders",
      },
      {
        description: "High-risk cases escalated",
        criterion: "Cases with high risk scores were escalated. CUST-8842 has low risk (high loyalty, moderate order) and should NOT be escalated, while higher-risk cases should be escalated.",
      },
      {
        description: "Summary emailed to fraud team",
        criterion: "A summary notification/email was sent to fraud-team@example.com with the investigation results",
      },
    ],
  },
  {
    name: "Proactive Churn Prevention",
    prompt:
      "CUST-2244 just returned something and I'm worried they're about to churn. " +
      "Look at their history, find the returned order, and give them 15% store credit on that order's total. " +
      "Also check if the returned items are back in stock. " +
      "Open a retention case, schedule a callback at +1-555-0142, " +
      "and send a win-back email.",
    toolNames: [
      "get_customer",
      "get_order_history",
      "lookup_order",
      "assess_risk",
      "apply_store_credit",
      "check_inventory",
      "create_case",
      "schedule_callback",
      "send_notification",
    ],
    expectations: [
      {
        description: "Customer CUST-2244 profile retrieved",
        criterion: "Customer CUST-2244 (James Rivera) profile was retrieved",
      },
      {
        description: "Order history reviewed",
        criterion: "The customer's order history was retrieved, including the returned order ORD-9013",
      },
      {
        description: "Store credit ~$15.75 applied",
        criterion: "Store credit of approximately $15.75 (15% of the returned order total $104.98) was applied to the customer's account. The amount should be between $15.00 and $16.00.",
      },
      {
        description: "Inventory checked for returned items",
        criterion: "Inventory was checked for the returned items' SKUs (SKU-M900 Ergonomic Mouse and SKU-P300 Phone Stand Pro)",
      },
      {
        description: "Retention case created",
        criterion: "A support/retention case was created for the customer",
      },
      {
        description: "Callback scheduled at +1-555-0142",
        criterion: "A callback was scheduled to the phone number +1-555-0142 or containing 555-0142",
      },
      {
        description: "Win-back email sent",
        criterion: "A win-back or retention email/notification was sent to the customer",
      },
    ],
  },
  {
    name: "Conditional Escalation Chain",
    prompt:
      "Check order ORD-7291 — if it's over $100, assess risk. " +
      "If risk is high, open a case AND escalate it. " +
      "If risk is low, just send the customer a thank-you email.",
    toolNames: [
      "lookup_order",
      "get_customer",
      "assess_risk",
      "create_case",
      "escalate_case",
      "send_notification",
    ],
    expectations: [
      {
        description: "Order ORD-7291 looked up",
        criterion: "Order ORD-7291 was looked up and confirmed to be over $100 (total is $129.97)",
      },
      {
        description: "Customer retrieved",
        criterion: "The customer profile (CUST-8842 / Maya Chen) was retrieved to get loyalty points for risk assessment",
      },
      {
        description: "Risk assessed",
        criterion: "A risk assessment was performed for the order/customer and returned a low risk score",
      },
      {
        description: "No case created (low risk)",
        criterion: "No support case was created because the risk level was low",
      },
      {
        description: "No escalation (low risk)",
        criterion: "No case escalation was performed because the risk level was low",
      },
      {
        description: "Thank-you email sent",
        criterion: "A thank-you email/notification was sent to maya.chen@example.com",
      },
    ],
  },
  {
    name: "Find-and-Fix Loop",
    prompt:
      "Go through CUST-8842's orders. For each one over $50 that's still 'shipped', " +
      "use the days_since tool to check if it's been more than 14 days since the order date. " +
      "If so, open a support case. Give me a summary of what you found.",
    toolNames: [
      "get_customer",
      "get_order_history",
      "lookup_order",
      "days_since",
      "create_case",
      "send_notification",
    ],
    expectations: [
      {
        description: "Order history retrieved",
        criterion: "The order history for CUST-8842 was retrieved, showing all their orders",
      },
      {
        description: "Shipped orders identified",
        criterion: "The agent identified that ORD-7291 is the only order with 'shipped' status and total over $50",
      },
      {
        description: "Case created for ORD-7291",
        criterion: "A support case was created for order ORD-7291 because it has been shipped for more than 14 days (ordered 2026-02-15, which is more than 14 days ago)",
      },
      {
        description: "Only 1 case created",
        criterion: "Exactly 1 support case was created total — only ORD-7291 qualifies (shipped, over $50, more than 14 days old). No other orders should have cases.",
      },
    ],
  },
  {
    name: "Cross-Customer Order Audit",
    prompt:
      "Audit ALL 3 customers (CUST-8842, CUST-2244, CUST-5501): " +
      "get each profile, get each order history, look up every non-returned order's details, " +
      "check inventory for every item across all those orders, flag any out-of-stock items, " +
      "and send a per-customer summary email with their audit results.",
    toolNames: [
      "get_customer",
      "get_order_history",
      "lookup_order",
      "check_inventory",
      "send_notification",
    ],
    expectations: [
      {
        description: "All 3 customer profiles retrieved",
        criterion: "Customer profiles were retrieved for CUST-8842 (Maya Chen), CUST-2244 (James Rivera), and CUST-5501 (Priya Sharma)",
      },
      {
        description: "All order histories retrieved",
        criterion: "Order histories were retrieved for all 3 customers",
      },
      {
        description: "Non-returned orders looked up",
        criterion: "Order details were looked up for non-returned orders. Returned orders ORD-6755 and ORD-9013 should be excluded from detail lookups.",
      },
      {
        description: "Inventory checked for all SKUs",
        criterion: "Inventory was checked for the SKUs from the non-returned orders",
      },
      {
        description: "Out-of-stock items flagged",
        criterion: "Out-of-stock items were identified — at minimum SKU-C200 (available=0), SKU-K800 (available=0), and SKU-H400 (available=0) should be flagged",
      },
      {
        description: "3 summary emails sent",
        criterion: "Three summary emails/notifications were sent, one per customer (to maya.chen@example.com, j.rivera@example.com, priya.s@example.com)",
      },
    ],
  },
  {
    name: "Tiered Loyalty Rewards",
    prompt:
      "For CUST-8842: get their profile and full order history, look up each order to compute their total spend, " +
      "then calculate a loyalty reward of 15% store credit on total spend above $200. " +
      "Apply the credit, assess risk on the credit amount, create a 'loyalty review' case, " +
      "and send a congratulations email.",
    toolNames: [
      "get_customer",
      "get_order_history",
      "lookup_order",
      "apply_store_credit",
      "assess_risk",
      "create_case",
      "send_notification",
    ],
    expectations: [
      {
        description: "Customer profile retrieved",
        criterion: "Customer CUST-8842 (Maya Chen) profile was retrieved",
      },
      {
        description: "Full order history retrieved",
        criterion: "The customer's order history was retrieved, showing all 4 orders",
      },
      {
        description: "Each order looked up for totals",
        criterion: "Individual order details were looked up to compute total spend",
      },
      {
        description: "Total spend calculated correctly",
        criterion: "Total spend was calculated across all orders. The 4 orders total approximately $455.47 ($129.97 + $64.50 + $219.00 + $42.00). The exact total may vary slightly.",
      },
      {
        description: "Store credit ~$38.32 applied",
        criterion: "Store credit of approximately $38.32 was applied. The correct calculation is 15% of (total spend ABOVE $200), i.e. 15% of ~$255.47 = ~$38.32. The amount MUST be between $35 and $42. An amount like $68 would be WRONG (that's 15% of total spend, not 15% of spend above $200).",
      },
      {
        description: "Risk assessed on credit",
        criterion: "Risk assessment was performed related to the credit or customer transaction",
      },
      {
        description: "Loyalty review case created",
        criterion: "A case was created with a category related to loyalty or review",
      },
      {
        description: "Congratulations email sent",
        criterion: "A congratulations or loyalty reward email was sent to maya.chen@example.com",
      },
    ],
  },

  // ── Scenario 9: Single-Tool Lookup ──────────────────────────────────────
  {
    name: "Single Order Lookup",
    prompt: "What is the current status and total for order ORD-7291?",
    toolNames: ["lookup_order"],
    expectations: [
      {
        description: "Order ORD-7291 looked up",
        criterion:
          "Order ORD-7291 was looked up and its status ('shipped') and total ($129.97) were reported",
      },
      {
        description: "Status and total reported",
        criterion:
          "Both the order status and total amount were included in the final response",
      },
    ],
  },

  // ── Scenario 10: Parallel Independent Lookups ───────────────────────────
  {
    name: "Parallel Customer Profiles",
    prompt:
      "Get the profiles for CUST-8842, CUST-2244, and CUST-5501. Give me each customer's name, tier, and loyalty points.",
    toolNames: ["get_customer"],
    expectations: [
      {
        description: "CUST-8842 profile retrieved",
        criterion:
          "Profile for CUST-8842 (Maya Chen, gold, 4820 points) was retrieved",
      },
      {
        description: "CUST-2244 profile retrieved",
        criterion:
          "Profile for CUST-2244 (James Rivera, silver, 1250 points) was retrieved",
      },
      {
        description: "CUST-5501 profile retrieved",
        criterion:
          "Profile for CUST-5501 (Priya Sharma, bronze, 380 points) was retrieved",
      },
      {
        description: "All info reported",
        criterion:
          "Names, tiers, and loyalty points for all three customers are included",
      },
    ],
  },

  // ── Scenario 11: Deep Fan-Out Audit ────────────────────────────────────
  {
    name: "Two-Customer Inventory Audit",
    prompt:
      "For CUST-8842 and CUST-2244: get their order histories, look up each non-returned order, check inventory for every item SKU, and report which SKUs are out of stock.",
    toolNames: [
      "get_customer",
      "get_order_history",
      "lookup_order",
      "check_inventory",
    ],
    expectations: [
      {
        description: "Both order histories retrieved",
        criterion:
          "Order histories retrieved for both CUST-8842 and CUST-2244",
      },
      {
        description: "Non-returned orders looked up",
        criterion:
          "Details retrieved for non-returned orders; ORD-6755 and ORD-9013 excluded",
      },
      {
        description: "Inventory checked",
        criterion:
          "Inventory checked for SKUs from non-returned orders",
      },
      {
        description: "Out-of-stock items reported",
        criterion:
          "SKU-C200, SKU-K800, and SKU-H400 (all available=0) identified as out of stock",
      },
    ],
  },
];

// ── Anthropic Client ────────────────────────────────────────────────────────

const client = new Anthropic();

// ── Helpers ─────────────────────────────────────────────────────────────────

/** Extract text content from an Anthropic response */
function extractText(content: Anthropic.Messages.ContentBlock[]): string {
  return content
    .filter((b): b is Anthropic.Messages.TextBlock => b.type === "text")
    .map((b) => b.text)
    .join("\n")
    .trim();
}

/** Extract tool calls from an Anthropic response */
function extractToolCalls(
  content: Anthropic.Messages.ContentBlock[],
): Array<{ name: string; input: unknown }> {
  return content
    .filter((b): b is Anthropic.Messages.ToolUseBlock => b.type === "tool_use")
    .map((b) => ({ name: b.name, input: b.input }));
}

/** Truncate a JSON string for terminal display */
function truncJson(value: unknown, maxLen = 120): string {
  const s = JSON.stringify(value);
  if (s.length <= maxLen) return s;
  return `${s.slice(0, maxLen - 3)}...`;
}

// ── Traditional Agent Loop ──────────────────────────────────────────────────

function buildTraditionalTools(toolDefs: ToolDef[]): Anthropic.Messages.Tool[] {
  return toolDefs.map((t) => {
    const shape = t.params.shape;
    const properties: Record<string, unknown> = {};
    const required: string[] = [];

    for (const [key, val] of Object.entries(shape)) {
      const zodField = val as z.ZodType;
      const isOptional = zodField.isOptional();

      let fieldSchema: Record<string, unknown> = { type: "string" };
      const desc = zodField.description;

      if (zodField instanceof z.ZodNumber) {
        fieldSchema = { type: "number" };
      } else if (zodField instanceof z.ZodArray) {
        fieldSchema = { type: "array", items: { type: "string" } };
      } else if (zodField instanceof z.ZodEnum) {
        fieldSchema = { type: "string", enum: zodField.options };
      } else if (zodField instanceof z.ZodRecord) {
        fieldSchema = { type: "object" };
      } else if (zodField instanceof z.ZodOptional) {
        const inner = zodField._def.innerType;
        if (inner instanceof z.ZodNumber) {
          fieldSchema = { type: "number" };
        } else if (inner instanceof z.ZodString) {
          fieldSchema = { type: "string" };
        }
      }

      if (desc) fieldSchema.description = desc;
      properties[key] = fieldSchema;
      if (!isOptional) required.push(key);
    }

    return {
      name: t.name,
      description: t.description,
      input_schema: {
        type: "object" as const,
        properties,
        required,
      },
    };
  });
}

async function traditionalAgentLoop(
  scenario: { prompt: string; tools: ToolDef[] },
): Promise<AgentRunResult> {
  const tools = buildTraditionalTools(scenario.tools);
  const handlerMap = new Map(scenario.tools.map((t) => [t.name, t.run]));

  const today = new Date().toISOString().split("T")[0];
  const prompt = `${scenario.prompt}\nToday's date is ${today}.`;

  const messages: Anthropic.Messages.MessageParam[] = [
    { role: "user", content: prompt },
  ];

  const conversation: ConversationEntry[] = [
    { role: "user", content: prompt },
  ];

  let totalInputTokens = 0;
  let totalOutputTokens = 0;
  let totalToolInvocations = 0;
  let roundTrips = 0;
  let systemPromptTokens = 0;
  const allToolCallRecords: ToolCallRecord[] = [];

  let turns = 0;
  while (turns < MAX_TURNS) {
    turns++;
    roundTrips++;

    const response = await client.messages.create({
      model: MODEL,
      max_tokens: MAX_TOKENS,
      temperature: 0,
      system: TRADITIONAL_SYSTEM_PROMPT,
      tools,
      messages,
    });

    // FAIRNESS: capture system prompt overhead on first turn
    if (turns === 1) {
      systemPromptTokens = response.usage.input_tokens;
    }

    totalInputTokens += response.usage.input_tokens;
    totalOutputTokens += response.usage.output_tokens;

    const text = extractText(response.content);
    const toolCalls = extractToolCalls(response.content);

    if (toolCalls.length === 0) {
      // Final response — no tool calls
      conversation.push({ role: "assistant", content: text });
      break;
    }

    // Assistant decided to call tools
    conversation.push({
      role: "assistant",
      content: text || undefined,
      toolCalls,
    });

    messages.push({ role: "assistant", content: response.content });

    const toolResultBlocks: Anthropic.Messages.ToolResultBlockParam[] = [];
    const toolResults: ConversationEntry["toolResults"] = [];

    const toolUseBlocks = response.content.filter(
      (b): b is Anthropic.Messages.ToolUseBlock => b.type === "tool_use",
    );

    for (const block of toolUseBlocks) {
      totalToolInvocations++;
      const handler = handlerMap.get(block.name);
      if (!handler) {
        toolResultBlocks.push({
          type: "tool_result",
          tool_use_id: block.id,
          content: `Unknown tool: ${block.name}`,
          is_error: true,
        });
        toolResults.push({
          name: block.name,
          output: `Unknown tool: ${block.name}`,
          isError: true,
        });
        allToolCallRecords.push({
          toolName: block.name,
          args: (block.input as Record<string, unknown>) ?? {},
          error: `Unknown tool: ${block.name}`,
        });
        continue;
      }

      try {
        const result = await handler(block.input as Record<string, unknown>);
        toolResultBlocks.push({
          type: "tool_result",
          tool_use_id: block.id,
          content: JSON.stringify(result),
        });
        toolResults.push({ name: block.name, output: result });
        allToolCallRecords.push({
          toolName: block.name,
          args: (block.input as Record<string, unknown>) ?? {},
          output: result,
        });
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        toolResultBlocks.push({
          type: "tool_result",
          tool_use_id: block.id,
          content: msg,
          is_error: true,
        });
        toolResults.push({ name: block.name, output: msg, isError: true });
        allToolCallRecords.push({
          toolName: block.name,
          args: (block.input as Record<string, unknown>) ?? {},
          error: msg,
        });
      }
    }

    conversation.push({ role: "tool", toolResults });
    messages.push({ role: "user", content: toolResultBlocks });
  }

  return {
    metrics: {
      roundTrips,
      totalToolInvocations,
      executeCallCount: 0,
      inputTokens: totalInputTokens,
      outputTokens: totalOutputTokens,
      costUsd: estimateCost(totalInputTokens, totalOutputTokens),
      systemPromptTokens,
    },
    conversation,
    toolCallRecords: allToolCallRecords,
  };
}

// ── Montygate Agent Loop ────────────────────────────────────────────────────

async function montygateAgentLoop(
  scenario: { prompt: string; tools: ToolDef[] },
): Promise<AgentRunResult> {
  const gate = new Montygate({
    limits: { maxConcurrent: 10, timeoutMs: 30_000 },
  });

  for (const t of scenario.tools) {
    gate.tool(t.name, {
      description: t.description,
      params: t.params,
      returns: t.returns,
      run: t.run as (args: z.infer<typeof t.params>) => Promise<unknown>,
    });
  }

  const tools = gate.anthropic() as Anthropic.Messages.Tool[];

  const systemPrompt = gate.systemPrompt();
  const today = new Date().toISOString().split("T")[0];
  const prompt = `${scenario.prompt}\nToday's date is ${today}.`;

  const messages: Anthropic.Messages.MessageParam[] = [
    { role: "user", content: prompt },
  ];

  const conversation: ConversationEntry[] = [
    { role: "user", content: prompt },
  ];

  let totalInputTokens = 0;
  let totalOutputTokens = 0;
  let roundTrips = 0;
  let executeCallCount = 0;
  let systemPromptTokens = 0;
  const allToolCallRecords: ToolCallRecord[] = [];

  let turns = 0;
  while (turns < MAX_TURNS) {
    turns++;
    roundTrips++;

    const response = await client.messages.create({
      model: MODEL,
      max_tokens: MAX_TOKENS,
      temperature: 0,
      system: systemPrompt,
      tools,
      messages,
    });

    totalInputTokens += response.usage.input_tokens;
    totalOutputTokens += response.usage.output_tokens;

    // FAIRNESS: capture system prompt overhead on first turn
    if (turns === 1) {
      systemPromptTokens = response.usage.input_tokens;
    }

    const text = extractText(response.content);
    const toolCalls = extractToolCalls(response.content);

    if (toolCalls.length === 0) {
      conversation.push({ role: "assistant", content: text });
      break;
    }

    conversation.push({
      role: "assistant",
      content: text || undefined,
      toolCalls,
    });

    messages.push({ role: "assistant", content: response.content });

    const toolResultBlocks: Anthropic.Messages.ToolResultBlockParam[] = [];
    const toolResults: ConversationEntry["toolResults"] = [];

    const toolUseBlocks = response.content.filter(
      (b): b is Anthropic.Messages.ToolUseBlock => b.type === "tool_use",
    );

    for (const block of toolUseBlocks) {
      try {
        const result = await gate.handleToolCall(
          block.name,
          block.input as Record<string, unknown>,
        );
        let executeTrace: ToolCallRecord[] | undefined;
        if (block.name === "execute") {
          executeCallCount++;
          executeTrace = gate.getTraces().map((t) => ({
            toolName: t.toolName,
            args: (t.input as Record<string, unknown>) ?? {},
            output: t.output,
            error: t.error,
          }));
          allToolCallRecords.push(...executeTrace);
          gate.clearTraces();
        }
        toolResultBlocks.push({
          type: "tool_result",
          tool_use_id: block.id,
          content: JSON.stringify(result),
        });
        toolResults.push({
          name: block.name,
          output: result,
          trace: executeTrace,
        });

        if (block.name !== "execute" && block.name !== "search") {
          allToolCallRecords.push({
            toolName: block.name,
            args: (block.input as Record<string, unknown>) ?? {},
            output: result,
          });
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        let executeTrace: ToolCallRecord[] | undefined;
        if (block.name === "execute") {
          executeCallCount++;
          executeTrace = gate.getTraces().map((t) => ({
            toolName: t.toolName,
            args: (t.input as Record<string, unknown>) ?? {},
            output: t.output,
            error: t.error,
          }));
          allToolCallRecords.push(...executeTrace);
          gate.clearTraces();
        }
        toolResultBlocks.push({
          type: "tool_result",
          tool_use_id: block.id,
          content: msg,
          is_error: true,
        });
        toolResults.push({
          name: block.name,
          output: msg,
          isError: true,
          trace: executeTrace,
        });
        if (block.name !== "execute" && block.name !== "search") {
          allToolCallRecords.push({
            toolName: block.name,
            args: (block.input as Record<string, unknown>) ?? {},
            error: msg,
          });
        }
      }
    }

    conversation.push({ role: "tool", toolResults });
    messages.push({ role: "user", content: toolResultBlocks });
  }

  return {
    metrics: {
      roundTrips,
      totalToolInvocations: allToolCallRecords.length,
      executeCallCount,
      inputTokens: totalInputTokens,
      outputTokens: totalOutputTokens,
      costUsd: estimateCost(totalInputTokens, totalOutputTokens),
      systemPromptTokens,
    },
    conversation,
    toolCallRecords: allToolCallRecords,
  };
}

// ── Conversation Rendering ──────────────────────────────────────────────────

function printConversation(
  scenarioName: string,
  mode: string,
  conversation: ConversationEntry[],
  metrics: RunMetrics,
): void {
  const header = `${scenarioName} | ${mode} (${metrics.roundTrips} round-trips)`;
  console.log();
  console.log(`  ${"─".repeat(header.length + 4)}`);
  console.log(`  | ${header} |`);
  console.log(`  ${"─".repeat(header.length + 4)}`);

  let rt = 0;

  for (const entry of conversation) {
    if (entry.role === "user") {
      console.log();
      console.log("  USER");
      const prompt = entry.content ?? "";
      // Word-wrap at 90 chars with indent
      const words = prompt.split(" ");
      let line = "    ";
      for (const word of words) {
        if (line.length + word.length > 94) {
          console.log(line);
          line = `    ${word}`;
        } else {
          line += (line.length > 4 ? " " : "") + word;
        }
      }
      if (line.trim()) console.log(line);
    } else if (entry.role === "assistant") {
      rt++;
      console.log();
      console.log(`  ASSISTANT  (RT ${rt})`);

      if (entry.content) {
        // Word-wrap assistant text
        const lines = entry.content.split("\n");
        for (const rawLine of lines) {
          const words = rawLine.split(" ");
          let line = "    ";
          for (const word of words) {
            if (line.length + word.length > 94) {
              console.log(line);
              line = `    ${word}`;
            } else {
              line += (line.length > 4 ? " " : "") + word;
            }
          }
          if (line.trim()) console.log(line);
        }
      }

      if (entry.toolCalls) {
        for (const tc of entry.toolCalls) {
          if (tc.name === "execute") {
            // Show the Python code for Montygate execute calls
            const input = tc.input as Record<string, unknown>;
            console.log(`    >> execute()`);
            if (typeof input.code === "string") {
              const codeLines = input.code.split("\n");
              for (const cl of codeLines) {
                console.log(`    |  ${cl}`);
              }
            }
          } else {
            console.log(`    >> ${tc.name}(${truncJson(tc.input, 80)})`);
          }
        }
      }
    } else if (entry.role === "tool") {
      if (entry.toolResults) {
        for (const tr of entry.toolResults) {
          const prefix = tr.isError ? "  !! " : "  << ";
          if (tr.name === "execute") {
            console.log(`${prefix}execute result:`);
            const resultStr = JSON.stringify(tr.output, null, 2);
            for (const line of resultStr.split("\n").slice(0, 20)) {
              console.log(`       ${line}`);
            }
            if (tr.trace && tr.trace.length > 0) {
              console.log("       trace:");
              for (const trace of tr.trace) {
                const rendered = trace.error ?? truncJson(trace.output, 80);
                console.log(
                  `         ${trace.toolName}(${truncJson(trace.args, 60)}) => ${rendered}`,
                );
              }
            }
          } else {
            console.log(`${prefix}${tr.name} => ${truncJson(tr.output, 90)}`);
          }
        }
      }
    }
  }

  console.log();
}

// ── Metrics Table ───────────────────────────────────────────────────────────

function formatNumber(n: number): string {
  return n.toLocaleString("en-US");
}

function savingsPct(a: number, b: number): number {
  if (a === 0) return 0;
  return Math.round(((a - b) / a) * 100);
}

function buildSavingsSummary(
  traditional: RunMetrics,
  variant: RunMetrics,
): SavingsSummary {
  return {
    roundTripsPct: savingsPct(traditional.roundTrips, variant.roundTrips),
    toolInvocationsPct: savingsPct(
      traditional.totalToolInvocations,
      variant.totalToolInvocations,
    ),
    inputTokensPct: savingsPct(traditional.inputTokens, variant.inputTokens),
    outputTokensPct: savingsPct(traditional.outputTokens, variant.outputTokens),
    costPct: savingsPct(traditional.costUsd, variant.costUsd),
  };
}

function printComparisonTable(
  result: ScenarioResult,
  variant: ScenarioModeResult,
): void {
  const w1 = 22;
  const w2 = 16;
  const w3 = 12;
  const w4 = 13;
  const pad = (s: string, w: number, right = false) =>
    right ? s.padStart(w) : s.padEnd(w);

  const sep = `+-${"-".repeat(w1)}-+-${"-".repeat(w2)}-+-${"-".repeat(w3)}-+-${"-".repeat(w4)}-+`;

  const row = (m: string, tv: string, mv: string, sv: string) =>
    `| ${pad(m, w1)} | ${pad(tv, w2, true)} | ${pad(mv, w3, true)} | ${pad(sv, w4, true)} |`;

  if (!result.traditional || !variant.savingsVsTraditional) {
    return;
  }

  console.log();
  console.log(`  ${result.name} (${result.toolCount} tools) — ${variant.label}`);
  console.log(sep);
  console.log(row("Metric", "Traditional", variant.label, "Savings"));
  console.log(sep);
  console.log(
    row(
      "LLM Round Trips",
      String(result.traditional.run.metrics.roundTrips),
      String(variant.run.metrics.roundTrips),
      `${variant.savingsVsTraditional.roundTripsPct}%`,
    ),
  );
  console.log(
    row(
      "Tool Invocations",
      String(result.traditional.run.metrics.totalToolInvocations),
      String(variant.run.metrics.totalToolInvocations),
      `${variant.savingsVsTraditional.toolInvocationsPct}%`,
    ),
  );
  console.log(
    row(
      "Execute Calls",
      "n/a",
      String(variant.run.metrics.executeCallCount),
      "",
    ),
  );
  console.log(
    row(
      "Input Tokens",
      formatNumber(result.traditional.run.metrics.inputTokens),
      formatNumber(variant.run.metrics.inputTokens),
      `${variant.savingsVsTraditional.inputTokensPct}%`,
    ),
  );
  console.log(
    row(
      "Output Tokens",
      formatNumber(result.traditional.run.metrics.outputTokens),
      formatNumber(variant.run.metrics.outputTokens),
      `${variant.savingsVsTraditional.outputTokensPct}%`,
    ),
  );

  // FAIRNESS: Net reasoning tokens = input tokens - system prompt overhead
  const tradNetReasoningTokens =
    result.traditional.run.metrics.inputTokens -
    result.traditional.run.metrics.systemPromptTokens;
  const variantNetReasoningTokens =
    variant.run.metrics.inputTokens - variant.run.metrics.systemPromptTokens;
  const netReasoningTokensPct = savingsPct(tradNetReasoningTokens, variantNetReasoningTokens);
  console.log(
    row(
      "Net Reasoning Tokens",
      formatNumber(tradNetReasoningTokens),
      formatNumber(variantNetReasoningTokens),
      `${netReasoningTokensPct}%`,
    ),
  );

  console.log(
    row(
      "Agent-Run Cost",
      `$${result.traditional.run.metrics.costUsd.toFixed(4)}`,
      `$${variant.run.metrics.costUsd.toFixed(4)}`,
      `${variant.savingsVsTraditional.costPct}%`,
    ),
  );

  // FAIRNESS: Net reasoning cost = cost excluding system prompt overhead
  const tradNetReasoningCost = estimateCost(tradNetReasoningTokens, result.traditional.run.metrics.outputTokens);
  const variantNetReasoningCost = estimateCost(variantNetReasoningTokens, variant.run.metrics.outputTokens);
  const netReasoningCostPct = savingsPct(tradNetReasoningCost, variantNetReasoningCost);
  console.log(
    row(
      "Net Reasoning Cost",
      `$${tradNetReasoningCost.toFixed(4)}`,
      `$${variantNetReasoningCost.toFixed(4)}`,
      `${netReasoningCostPct}%`,
    ),
  );

  console.log(
    row(
      "Correctness",
      `${Math.round(result.traditional.eval.score * 100)}% (${result.traditional.eval.passed}/${result.traditional.eval.total})`,
      `${Math.round(variant.eval.score * 100)}% (${variant.eval.passed}/${variant.eval.total})`,
      "",
    ),
  );
  console.log(sep);
}

function printEvalResults(
  scenarioName: string,
  traditional: ScenarioModeResult,
  variant: ScenarioModeResult,
): void {
  const w1 = 45;
  const w2 = 14;
  const w3 = 14;
  const pad = (s: string, w: number) => s.padEnd(w);

  const sep = `+-${"-".repeat(w1)}-+-${"-".repeat(w2)}-+-${"-".repeat(w3)}-+`;
  const row = (desc: string, trad: string, mg: string) =>
    `| ${pad(desc, w1)} | ${pad(trad, w2)} | ${pad(mg, w3)} |`;

  console.log();
  console.log(`  ${scenarioName} — Correctness (LLM-as-Judge) — ${variant.label}`);
  console.log(sep);
  console.log(row("Outcome Criterion", "Traditional", variant.label));
  console.log(sep);

  for (let i = 0; i < traditional.eval.results.length; i++) {
    const tr = traditional.eval.results[i];
    const mr = variant.eval.results[i];
    const tradStatus = tr.passed ? "PASS" : "FAIL";
    const mgStatus = mr.passed ? "PASS" : "FAIL";
    console.log(row(tr.description, tradStatus, mgStatus));
  }

  console.log(sep);
  console.log(
    row(
      "TOTAL",
      `${traditional.eval.passed}/${traditional.eval.total} (${Math.round(traditional.eval.score * 100)}%)`,
      `${variant.eval.passed}/${variant.eval.total} (${Math.round(variant.eval.score * 100)}%)`,
    ),
  );
  console.log(sep);

  // Print evidence for any failed criteria
  const failures: Array<{ mode: string; description: string; evidence: string }> = [];
  for (const r of traditional.eval.results) {
    if (!r.passed) {
      failures.push({ mode: "Traditional", description: r.description, evidence: r.evidence });
    }
  }
  for (const r of variant.eval.results) {
    if (!r.passed) {
      failures.push({ mode: variant.label, description: r.description, evidence: r.evidence });
    }
  }
  if (failures.length > 0) {
    console.log();
    for (const f of failures) {
      console.log(`  FAILED [${f.mode}]: "${f.description}"`);
      console.log(`    Evidence: ${f.evidence}`);
    }
  }
}

// ── CLI ─────────────────────────────────────────────────────────────────────

function parseArgs(): BenchmarkArgs {
  const removedArgs = process.argv.filter(
    (arg) =>
      arg.startsWith("--mode=") ||
      arg === "--no-state-injection" ||
      arg === "--stress-no-state-injection",
  );
  if (removedArgs.length > 0) {
    console.error(
      `Unsupported option(s): ${removedArgs.join(", ")}. This benchmark now runs Traditional vs Montygate only.`,
    );
    process.exit(1);
  }

  const runsArg = process.argv.find((a) => a.startsWith("--runs="));
  // FAIRNESS: default to 3 runs for variance reporting
  const runs = runsArg ? parseInt(runsArg.split("=")[1], 10) : 3;
  if (isNaN(runs) || runs < 1) {
    console.error("--runs must be a positive integer");
    process.exit(1);
  }

  const modelArg = process.argv.find((a) => a.startsWith("--model="));
  if (modelArg) {
    MODEL = modelArg.split("=")[1];
  }

  return {
    runs,
  };
}

// ── Statistics Helpers ──────────────────────────────────────────────────────

export function stdDev(values: number[]): number {
  if (values.length === 0) return 0;
  if (values.length === 1) return 0;
  const mean = values.reduce((a, b) => a + b, 0) / values.length;
  const variance = values.reduce((s, v) => s + Math.pow(v - mean, 2), 0) / (values.length - 1);
  return Math.sqrt(variance);
}

export function computeMetricsStats(runs: RunMetrics[]): MetricsStats {
  const n = runs.length;
  const netReasoningTokens = runs.map((r) => r.inputTokens - r.systemPromptTokens);
  const netReasoningCosts = runs.map(
    (r) => estimateCost(r.inputTokens - r.systemPromptTokens, r.outputTokens),
  );

  return {
    mean: {
      roundTrips: runs.reduce((s, r) => s + r.roundTrips, 0) / n,
      totalToolInvocations: runs.reduce((s, r) => s + r.totalToolInvocations, 0) / n,
      executeCallCount: runs.reduce((s, r) => s + r.executeCallCount, 0) / n,
      inputTokens: runs.reduce((s, r) => s + r.inputTokens, 0) / n,
      outputTokens: runs.reduce((s, r) => s + r.outputTokens, 0) / n,
      systemPromptTokens: runs.reduce((s, r) => s + r.systemPromptTokens, 0) / n,
      costUsd: runs.reduce((s, r) => s + r.costUsd, 0) / n,
      netReasoningTokens: netReasoningTokens.reduce((a, b) => a + b, 0) / n,
      netReasoningCostUsd: netReasoningCosts.reduce((a, b) => a + b, 0) / n,
    },
    stdDev: {
      roundTrips: stdDev(runs.map((r) => r.roundTrips)),
      totalToolInvocations: stdDev(runs.map((r) => r.totalToolInvocations)),
      executeCallCount: stdDev(runs.map((r) => r.executeCallCount)),
      inputTokens: stdDev(runs.map((r) => r.inputTokens)),
      outputTokens: stdDev(runs.map((r) => r.outputTokens)),
      systemPromptTokens: stdDev(runs.map((r) => r.systemPromptTokens)),
      costUsd: stdDev(runs.map((r) => r.costUsd)),
      netReasoningTokens: stdDev(netReasoningTokens),
      netReasoningCostUsd: stdDev(netReasoningCosts),
    },
  };
}

// ── Main ────────────────────────────────────────────────────────────────────

function averageMetrics(runs: RunMetrics[]): RunMetrics {
  const n = runs.length;
  const stats = computeMetricsStats(runs);
  return {
    roundTrips: Math.round(stats.mean.roundTrips),
    totalToolInvocations: Math.round(stats.mean.totalToolInvocations),
    executeCallCount: Math.round(stats.mean.executeCallCount),
    inputTokens: Math.round(stats.mean.inputTokens),
    outputTokens: Math.round(stats.mean.outputTokens),
    costUsd: stats.mean.costUsd,
    systemPromptTokens: Math.round(stats.mean.systemPromptTokens),
  };
}

function averageEval(evals: EvalResult[]): EvalResult {
  const n = evals.length;
  const avgPassed = evals.reduce((s, e) => s + e.passed, 0) / n;
  const avgFailed = evals.reduce((s, e) => s + e.failed, 0) / n;
  const avgScore = evals.reduce((s, e) => s + e.score, 0) / n;
  // Use the last run's detailed results (most representative)
  return {
    passed: Math.round(avgPassed),
    failed: Math.round(avgFailed),
    total: evals[0]?.total ?? 0,
    score: avgScore,
    results: evals[evals.length - 1]?.results ?? [],
  };
}

function resolveTools(toolNames: string[], counters: IdCounters): ToolDef[] {
  const allTools = createAllTools(counters);
  return toolNames.map((name) => {
    const tool = allTools[name];
    if (!tool) throw new Error(`Unknown tool name: ${name}`);
    return tool;
  });
}

const DEFAULT_SCENARIO_RUNNERS: ScenarioRunners = {
  traditional: traditionalAgentLoop,
  montygate: montygateAgentLoop,
  judge: judgeOutcomes,
};

function createEmptyEvalResult(): EvalResult {
  return { passed: 0, failed: 0, total: 0, score: 0, results: [] };
}

function buildScenarioModeResult(
  key: BenchmarkMode,
  label: string,
  runs: AgentRunResult[],
  evals: EvalResult[],
  traditionalMetrics?: RunMetrics | null,
): ScenarioModeResult | null {
  if (runs.length === 0) {
    return null;
  }

  const metricsArray = runs.map((r) => r.metrics);
  const run = {
    ...runs[runs.length - 1],
    metrics: averageMetrics(metricsArray),
  };
  const evalResult = evals.length > 0 ? averageEval(evals) : createEmptyEvalResult();

  // FAIRNESS: compute variance statistics when multiple runs are available
  const metricsStats = metricsArray.length > 1 ? computeMetricsStats(metricsArray) : undefined;

  return {
    key,
    label,
    run,
    eval: evalResult,
    savingsVsTraditional: traditionalMetrics
      ? buildSavingsSummary(traditionalMetrics, run.metrics)
      : null,
    metricsStats,
  };
}

function serializeMetrics(metrics: RunMetrics) {
  return {
    round_trips: metrics.roundTrips,
    total_tool_invocations: metrics.totalToolInvocations,
    execute_call_count: metrics.executeCallCount,
    input_tokens: metrics.inputTokens,
    output_tokens: metrics.outputTokens,
    cost_usd: metrics.costUsd,
    system_prompt_tokens: metrics.systemPromptTokens,
  };
}

function serializeEval(evalResult: EvalResult) {
  return {
    passed: evalResult.passed,
    failed: evalResult.failed,
    total: evalResult.total,
    score: evalResult.score,
    results: evalResult.results.map((er) => ({
      description: er.description,
      passed: er.passed,
      evidence: er.evidence,
    })),
  };
}

function serializeSavings(savings: SavingsSummary | null) {
  if (!savings) {
    return null;
  }

  return {
    round_trips_pct: savings.roundTripsPct,
    tool_invocations_pct: savings.toolInvocationsPct,
    input_tokens_pct: savings.inputTokensPct,
    output_tokens_pct: savings.outputTokensPct,
    cost_pct: savings.costPct,
  };
}

function serializeScenarioMode(modeResult: ScenarioModeResult | null) {
  if (!modeResult) {
    return null;
  }

  return {
    mode: modeResult.key,
    label: modeResult.label,
    metrics: serializeMetrics(modeResult.run.metrics),
    conversation: modeResult.run.conversation,
    tool_call_records: modeResult.run.toolCallRecords,
    eval: serializeEval(modeResult.eval),
    savings_vs_traditional: serializeSavings(modeResult.savingsVsTraditional),
  };
}

function buildJsonOutput(
  results: ScenarioResult[],
  options: Pick<RunBenchmarkOptions, "runs">,
) {
  return {
    timestamp: new Date().toISOString(),
    model: MODEL,
    judge_model: JUDGE_MODEL,
    runs: options.runs,
    cost_scope: "agent_only_excludes_judge",
    scenarios: results.map((r) => ({
      name: r.name,
      prompt: r.prompt,
      tools: r.toolCount,
      traditional: serializeScenarioMode(r.traditional),
      montygate: serializeScenarioMode(r.montygate),
    })),
  };
}

async function runScenario(
  scenario: ScenarioDef,
  options: RunBenchmarkOptions,
  runners: ScenarioRunners = DEFAULT_SCENARIO_RUNNERS,
): Promise<{ result: ScenarioResult; log: string[] }> {
  const log: string[] = [];
  const push = (msg: string) => log.push(msg);

  push(`Running: ${scenario.name}...`);
  push(`  Tools: ${scenario.toolNames.join(", ")}`);

  const traditionalRuns: AgentRunResult[] = [];
  const traditionalEvals: EvalResult[] = [];
  const montygateRuns: AgentRunResult[] = [];
  const montygateEvals: EvalResult[] = [];

  for (let run = 0; run < options.runs; run++) {
    if (options.runs > 1) push(`  --- run ${run + 1}/${options.runs} ---`);

    const traditionalTools = resolveTools(scenario.toolNames, createIdCounters());
    push("  [traditional] running...");
    const trad = await runners.traditional({ prompt: scenario.prompt, tools: traditionalTools });
    traditionalRuns.push(trad);
    push(
      `  [traditional] done — ${trad.metrics.roundTrips} round-trips, ${formatNumber(trad.metrics.inputTokens)} input tokens`,
    );

    push("  [judge] evaluating traditional...");
    const tradEval = await runners.judge(trad.conversation, scenario.expectations);
    traditionalEvals.push(tradEval);
    push(`  [judge] traditional: ${tradEval.passed}/${tradEval.total}`);

    const montygateTools = resolveTools(scenario.toolNames, createIdCounters());
    push("  [montygate] running...");
    const mg = await runners.montygate({ prompt: scenario.prompt, tools: montygateTools });
    montygateRuns.push(mg);
    push(
      `  [montygate] done — ${mg.metrics.roundTrips} round-trips, ${formatNumber(mg.metrics.inputTokens)} input tokens`,
    );

    push("  [judge] evaluating montygate...");
    const mgEval = await runners.judge(mg.conversation, scenario.expectations);
    montygateEvals.push(mgEval);
    push(`  [judge] montygate: ${mgEval.passed}/${mgEval.total}`);
  }

  const traditional = buildScenarioModeResult(
    "traditional",
    "Traditional",
    traditionalRuns,
    traditionalEvals,
  );
  const traditionalMetrics = traditional?.run.metrics ?? null;
  const montygate = buildScenarioModeResult(
    "montygate",
    "Montygate",
    montygateRuns,
    montygateEvals,
    traditionalMetrics,
  );

  if (options.runs > 1) {
    const tradScores = traditionalEvals.map((e) => `${e.passed}/${e.total}`);
    if (tradScores.length > 0) push(`  [summary] traditional evals: ${tradScores.join(", ")}`);
    const montygateScores = montygateEvals.map((e) => `${e.passed}/${e.total}`);
    if (montygateScores.length > 0) {
      push(`  [summary] montygate evals: ${montygateScores.join(", ")}`);
    }
  }

  const result: ScenarioResult = {
    name: scenario.name,
    toolCount: scenario.toolNames.length,
    prompt: scenario.prompt,
    traditional,
    montygate,
  };

  return { result, log };
}

async function runBenchmark(
  options: RunBenchmarkOptions,
  runners: ScenarioRunners = DEFAULT_SCENARIO_RUNNERS,
): Promise<ScenarioResult[]> {
  const settled = await Promise.all(
    scenarios.map(async (scenario, index) => {
      const { result, log } = await runScenario(scenario, options, runners);
      return { index, result, log };
    }),
  );

  settled.sort((a, b) => a.index - b.index);
  for (const { log } of settled) {
    for (const line of log) {
      console.log(line);
    }
  }

  return settled.map((s) => s.result);
}

async function main() {
  const options = parseArgs();
  const { runs } = options;

  console.log("Montygate vs Traditional — Real LLM Eval (LLM-as-Judge)");
  console.log(`Model: ${MODEL}`);
  console.log(`Judge: ${JUDGE_MODEL}`);
  console.log(`Scenarios: ${scenarios.length}`);
  console.log("Comparison: traditional vs Montygate");
  if (runs > 1) console.log(`Runs per scenario: ${runs}`);
  console.log("Scenario execution: parallel");
  console.log();

  const results = await runBenchmark(options);

  // ── Print conversations ───────────────────────────────────────────────
  console.log(`\n${"=".repeat(70)}`);
  console.log("  CONVERSATIONS");
  console.log("=".repeat(70));

  for (const result of results) {
    if (result.traditional?.run.conversation.length) {
      printConversation(
        result.name,
        "Traditional",
        result.traditional.run.conversation,
        result.traditional.run.metrics,
      );
    }
    if (result.montygate?.run.conversation.length) {
      printConversation(
        result.name,
        "Montygate",
        result.montygate.run.conversation,
        result.montygate.run.metrics,
      );
    }
  }

  // ── Print eval results ────────────────────────────────────────────────
  console.log(`\n${"=".repeat(70)}`);
  console.log("  CORRECTNESS EVAL (LLM-as-Judge)");
  console.log("=".repeat(70));

  for (const result of results) {
    if (result.traditional && result.montygate) {
      printEvalResults(result.name, result.traditional, result.montygate);
    }
  }

  // ── Print comparison tables ───────────────────────────────────────────
  console.log(`\n${"=".repeat(70)}`);
  console.log("  RESULTS — Montygate vs Traditional Tool Use");
  console.log("=".repeat(70));

  for (const result of results) {
    if (result.montygate) {
      printComparisonTable(result, result.montygate);
    }
  }

  // ── Write JSON output ─────────────────────────────────────────────────
  const jsonOutput = buildJsonOutput(results, options);

  const outPath = path.join(process.cwd(), "benchmark-results.json");
  fs.writeFileSync(outPath, `${JSON.stringify(jsonOutput, null, 2)}\n`);
  console.log(`\nResults written to ${outPath}`);
}

function isMainModule(): boolean {
  const entry = process.argv[1];
  if (!entry) {
    return false;
  }
  return import.meta.url === pathToFileURL(entry).href;
}

if (isMainModule()) {
  main().catch(console.error);
}

export {
  buildJudgePrompt,
  buildJsonOutput,
  formatConversationForJudge,
  parseArgs,
  printComparisonTable,
  runBenchmark,
  runScenario,
  scenarios,
};
