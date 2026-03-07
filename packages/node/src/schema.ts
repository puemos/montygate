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
  let result: Record<string, unknown>;

  switch (typeName) {
    case "ZodString":
      result = { type: "string" };
      break;
    case "ZodNumber":
      result = { type: "number" };
      break;
    case "ZodBoolean":
      result = { type: "boolean" };
      break;
    case "ZodArray":
      result = {
        type: "array",
        items: zodDefToJsonSchema(def.type),
      };
      break;
    case "ZodObject": {
      const shape = def.shape();
      const properties: Record<string, unknown> = {};
      const required: string[] = [];

      for (const [key, value] of Object.entries(shape)) {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const fieldDef = (value as any)._def;
        if (fieldDef?.typeName === "ZodOptional") {
          properties[key] = withDescription(
            zodDefToJsonSchema(fieldDef.innerType),
            fieldDef.description,
          );
        } else {
          properties[key] = zodDefToJsonSchema(value as z.ZodType);
          required.push(key);
        }
      }

      result = {
        type: "object",
        properties,
      };
      if (required.length > 0) {
        result.required = required;
      }
      break;
    }
    case "ZodOptional":
      result = zodDefToJsonSchema(def.innerType);
      break;
    case "ZodEnum":
      result = { type: "string", enum: def.values };
      break;
    case "ZodRecord":
      result = {
        type: "object",
        additionalProperties: zodDefToJsonSchema(def.valueType),
      };
      break;
    default:
      result = {};
  }

  return withDescription(result, def.description);
}

function withDescription(
  schema: Record<string, unknown>,
  description?: string,
): Record<string, unknown> {
  if (description && schema.description == null) {
    schema.description = description;
  }
  return schema;
}
