/**
 * Example: Using Montygate with the OpenAI SDK.
 *
 * This shows how to convert registered tools into OpenAI-compatible
 * function tool definitions and handle tool calls.
 */
import { Montygate, toOpenAI, handleOpenAIToolCall } from "montygate";
import { z } from "zod";
// import OpenAI from "openai";

const engine = new Montygate();

engine.tool("get_weather", {
  description: "Get current weather for a city",
  params: z.object({ city: z.string() }),
  run: async ({ city }) => ({
    city,
    temp: 72,
    condition: "sunny",
  }),
});

engine.tool("get_forecast", {
  description: "Get 5-day forecast for a city",
  params: z.object({ city: z.string(), days: z.number().optional() }),
  run: async ({ city, days }) => ({
    city,
    days: days ?? 5,
    forecast: ["sunny", "cloudy", "rain", "sunny", "sunny"],
  }),
});

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
