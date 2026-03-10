type ConversationMessage = {
  role: string;
  content?: string;
  toolCalls?: Array<{ name: string; input: unknown }>;
  toolResults?: Array<{
    name: string;
    output: unknown;
    isError?: boolean;
    trace?: Array<{
      toolName: string;
      args: Record<string, unknown>;
      output?: unknown;
      error?: string;
    }>;
  }>;
};

type Savings = {
  round_trips_pct: number;
  tool_invocations_pct: number;
  input_tokens_pct: number;
  output_tokens_pct: number;
  cost_pct: number;
};

type EvalResult = {
  passed: number;
  failed: number;
  total: number;
  score: number;
  results: Array<{
    description: string;
    passed: boolean;
    evidence: string;
  }>;
};

type Metrics = {
  round_trips: number;
  total_tool_invocations: number;
  execute_call_count: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
};

type ModeData = {
  mode?: string;
  label?: string;
  metrics: Metrics;
  conversation: ConversationMessage[];
  tool_call_records?: Array<{
    toolName: string;
    args: Record<string, unknown>;
    output?: unknown;
    error?: string;
  }>;
  eval: EvalResult;
  savings_vs_traditional: Savings | null;
};

type Scenario = {
  name: string;
  prompt: string;
  tools: number;
  traditional: ModeData | null;
  montygate: ModeData | null;
};

export type NormalizedMode = {
  key: "traditional" | "montygate";
  label: string;
  metrics: Metrics;
  conversation: ConversationMessage[];
  eval: EvalResult;
  savingsVsTraditional: Savings | null;
};

export type NormalizedScenario = {
  name: string;
  prompt: string;
  tools: number;
  traditional: NormalizedMode | null;
  montygate: NormalizedMode | null;
};

export type BenchmarkTableRow = {
  scenario: string;
  tools: number;
  mode: string;
  savings: Savings;
  eval: EvalResult;
};

export function normalizeBenchmarkData(data: { scenarios: Scenario[] }) {
  const scenarios = data.scenarios.map((scenario) => ({
    name: scenario.name,
    prompt: scenario.prompt,
    tools: scenario.tools,
    traditional: normalizeMode(
      "traditional",
      scenario.traditional,
      "Traditional",
    ),
    montygate: normalizeMode(
      "montygate",
      scenario.montygate,
      scenario.montygate?.label ?? "Montygate",
    ),
  }));

  return {
    scenarios,
    tableRows: buildBenchmarkTableRows(scenarios),
  };
}

function normalizeMode(
  key: "traditional" | "montygate",
  mode: ModeData | null,
  fallbackLabel: string,
): NormalizedMode | null {
  if (!mode) {
    return null;
  }

  return {
    key,
    label: mode.label ?? fallbackLabel,
    metrics: mode.metrics,
    conversation: mode.conversation,
    eval: mode.eval,
    savingsVsTraditional: mode.savings_vs_traditional,
  };
}

function buildBenchmarkTableRows(
  scenarios: NormalizedScenario[],
): BenchmarkTableRow[] {
  const rows: BenchmarkTableRow[] = [];

  for (const scenario of scenarios) {
    const variant = scenario.montygate;
    if (!variant?.savingsVsTraditional) {
      continue;
    }

    rows.push({
      scenario: scenario.name,
      tools: scenario.tools,
      mode: variant.label,
      savings: variant.savingsVsTraditional,
      eval: variant.eval,
    });
  }

  return rows;
}
