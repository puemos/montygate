import { describe, expect, it } from "vitest";
import { z } from "zod";
import { zodToJsonSchema } from "./schema.js";

describe("zodToJsonSchema", () => {
  it("converts a simple object schema", () => {
    const schema = z.object({
      name: z.string(),
      age: z.number(),
    });

    const result = zodToJsonSchema(schema);

    expect(result.type).toBe("object");
    expect(result.properties).toBeDefined();
    const props = result.properties as Record<string, { type: string }>;
    expect(props.name.type).toBe("string");
    expect(props.age.type).toBe("number");
    expect(result.required).toEqual(["name", "age"]);
  });

  it("handles optional fields", () => {
    const schema = z.object({
      required_field: z.string(),
      optional_field: z.string().optional(),
    });

    const result = zodToJsonSchema(schema);

    expect(result.required).toEqual(["required_field"]);
    const props = result.properties as Record<string, { type: string }>;
    expect(props.optional_field.type).toBe("string");
  });

  it("converts string type", () => {
    const result = zodToJsonSchema(z.string());
    expect(result).toEqual({ type: "string" });
  });

  it("converts number type", () => {
    const result = zodToJsonSchema(z.number());
    expect(result).toEqual({ type: "number" });
  });

  it("converts boolean type", () => {
    const result = zodToJsonSchema(z.boolean());
    expect(result).toEqual({ type: "boolean" });
  });

  it("converts array type", () => {
    const result = zodToJsonSchema(z.array(z.string()));
    expect(result).toEqual({
      type: "array",
      items: { type: "string" },
    });
  });

  it("converts enum type", () => {
    const result = zodToJsonSchema(z.enum(["a", "b", "c"]));
    expect(result).toEqual({
      type: "string",
      enum: ["a", "b", "c"],
    });
  });

  it("converts nested object", () => {
    const schema = z.object({
      user: z.object({
        name: z.string(),
      }),
    });

    const result = zodToJsonSchema(schema);
    const props = result.properties as Record<string, unknown>;
    const userSchema = props.user as Record<string, unknown>;
    expect(userSchema.type).toBe("object");
    const userProps = userSchema.properties as Record<string, { type: string }>;
    expect(userProps.name.type).toBe("string");
  });

  it("converts record type", () => {
    const result = zodToJsonSchema(z.record(z.number()));
    expect(result).toEqual({
      type: "object",
      additionalProperties: { type: "number" },
    });
  });

  it("preserves field descriptions", () => {
    const result = zodToJsonSchema(z.string().describe("hello world"));
    expect(result).toEqual({
      type: "string",
      description: "hello world",
    });
  });

  it("preserves optional wrapper descriptions", () => {
    const schema = z.object({
      note: z.string().optional().describe("Optional note"),
    });

    const result = zodToJsonSchema(schema);
    const props = result.properties as Record<string, { description?: string }>;
    expect(props.note.description).toBe("Optional note");
  });
});
