import type { z } from "zod";

/**
 * Convert a Zod schema to a JSON Schema object.
 *
 * Uses Zod's built-in toJSONSchema() if available (Zod 4+),
 * otherwise falls back to a basic conversion.
 */
export function zodToJsonSchema(schema: z.ZodType): Record<string, unknown> {
  // Zod v3.24+ has a toJSONSchema method
  if ("toJSONSchema" in (schema.constructor as never)) {
    try {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      return (schema as any).toJSONSchema() as Record<string, unknown>;
    } catch {
      // fallback
    }
  }

  // Zod v3 fallback: use _def to extract basic shape
  return zodDefToJsonSchema(schema);
}

function zodDefToJsonSchema(schema: z.ZodType): Record<string, unknown> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const def = (schema as any)._def;
  if (!def) return { type: "object" };

  const typeName: string = def.typeName;

  switch (typeName) {
    case "ZodString":
      return { type: "string" };
    case "ZodNumber":
      return { type: "number" };
    case "ZodBoolean":
      return { type: "boolean" };
    case "ZodArray":
      return {
        type: "array",
        items: zodDefToJsonSchema(def.type),
      };
    case "ZodObject": {
      const shape = def.shape();
      const properties: Record<string, unknown> = {};
      const required: string[] = [];

      for (const [key, value] of Object.entries(shape)) {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const fieldDef = (value as any)._def;
        if (fieldDef?.typeName === "ZodOptional") {
          properties[key] = zodDefToJsonSchema(fieldDef.innerType);
        } else {
          properties[key] = zodDefToJsonSchema(value as z.ZodType);
          required.push(key);
        }
      }

      const result: Record<string, unknown> = {
        type: "object",
        properties,
      };
      if (required.length > 0) {
        result.required = required;
      }
      return result;
    }
    case "ZodOptional":
      return zodDefToJsonSchema(def.innerType);
    case "ZodEnum":
      return { type: "string", enum: def.values };
    case "ZodRecord":
      return {
        type: "object",
        additionalProperties: zodDefToJsonSchema(def.valueType),
      };
    default:
      return {};
  }
}
