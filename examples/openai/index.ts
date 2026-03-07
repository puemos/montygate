/**
 * Example: Using Montygate with the OpenAI SDK.
 *
 * This shows how to wrap existing OpenAI tool definitions directly —
 * no need to rewrite schemas. Montygate auto-detects the format.
 */
import { Montygate, toOpenAI, handleOpenAIToolCall } from "montygate";
// import OpenAI from "openai";

// Your existing OpenAI tool definitions — pass them straight to Montygate
const myOpenAITools = [
  {
    type: "function" as const,
    function: {
      name: "get_weather",
      description: "Get current weather for a city",
      parameters: {
        type: "object",
        properties: { city: { type: "string" } },
        required: ["city"],
      },
    },
  },
  {
    type: "function" as const,
    function: {
      name: "get_forecast",
      description: "Get 5-day forecast for a city",
      parameters: {
        type: "object",
        properties: {
          city: { type: "string" },
          days: { type: "number" },
        },
        required: ["city"],
      },
    },
  },
];

// Just wrap your existing tools + provide handlers
const engine = new Montygate();
engine.tools(myOpenAITools, {
  get_weather: async (args: unknown) => {
    const { city } = args as { city: string };
    return { city, temp: 72, condition: "sunny" };
  },
  get_forecast: async (args: unknown) => {
    const { city, days } = args as { city: string; days?: number };
    return {
      city,
      days: days ?? 5,
      forecast: ["sunny", "cloudy", "rain", "sunny", "sunny"],
    };
  },
});

// Alternative: register tools one-by-one with Zod schemas (still supported)
// import { z } from "zod";
// engine.tool("get_weather", {
//   description: "Get current weather for a city",
//   params: z.object({ city: z.string() }),
//   run: async ({ city }) => ({ city, temp: 72, condition: "sunny" }),
// });

// Get OpenAI-compatible tool definitions
const tools = toOpenAI(engine);
console.log("OpenAI tools:", JSON.stringify(tools, null, 2));

// In a real app:
//
// const client = new OpenAI();
// const response = await client.chat.completions.create({
//   model: "gpt-4",
//   tools,
//   messages: [{ role: "user", content: "What's the weather in NYC and the 3-day forecast?" }],
// });
//
// for (const call of response.choices[0].message.tool_calls ?? []) {
//   const result = await handleOpenAIToolCall(engine, call.function.name, call.function.arguments);
//   // Send result back...
// }

// Simulate
async function main() {
  const result = await handleOpenAIToolCall(
    engine,
    "execute",
    JSON.stringify({
      code: `
weather = tool('get_weather', city='New York')
forecast = tool('get_forecast', city='New York', days=3)
{'current': weather, 'forecast': forecast}
      `,
    }),
  );

  console.log("\nResult:", result);
}

main().catch(console.error);
