/**
 * Converts a function's declared `params` into a JSON Schema the LLM understands,
 * and validates the arguments the LLM produces before we invoke the dev's handler.
 *
 * A param can be declared three ways (all interchangeable, pick per need):
 *
 *   1. Shorthand string (required):      orderId: 'string'
 *   2. Shorthand string (optional):      status:  'string?'      // trailing ? = optional
 *   3. Full object (most control):       tone: {
 *                                          type: 'string',
 *                                          description: 'Reply length',
 *                                          enum: ['short', 'detailed'],
 *                                          optional: true,
 *                                          format: 'email',       // optional JSON-schema format
 *                                          items: 'string',       // for type: 'array'
 *                                        }
 *
 * Backward compatible: `{ orderId: 'string' }` still means a single required string,
 * exactly as before.
 */

export type JsonSchemaType = 'string' | 'number' | 'integer' | 'boolean' | 'array' | 'object';

/** A single property's emitted JSON Schema. */
export type ParamJsonSchema = {
  type?: string;
  description?: string;
  enum?: unknown[];
  format?: string;
  items?: ParamJsonSchema;
  properties?: Record<string, ParamJsonSchema>;
  required?: string[];
};

export type InputJsonSchema = {
  type: 'object';
  properties: Record<string, ParamJsonSchema>;
  required?: string[];
};

function stripOptionalMarker(type: string): { type: string; optional: boolean } {
  if (type.endsWith('?')) {
    return { type: type.slice(0, -1).trim(), optional: true };
  }
  return { type, optional: false };
}

/**
 * Normalize one declared param into { schema, required }.
 * Unknown / malformed inputs degrade gracefully to a required string.
 */
function normalizeParam(value: unknown): { schema: ParamJsonSchema; required: boolean } {
  if (typeof value === 'string') {
    const { type, optional } = stripOptionalMarker(value);
    return { schema: { type }, required: !optional };
  }

  if (value && typeof value === 'object') {
    const spec = value as Record<string, unknown>;
    const schema: ParamJsonSchema = {};

    if (typeof spec.type === 'string') {
      schema.type = stripOptionalMarker(spec.type).type;
    }
    if (typeof spec.description === 'string') {
      schema.description = spec.description;
    }
    if (Array.isArray(spec.enum)) {
      schema.enum = spec.enum;
    }
    if (typeof spec.format === 'string') {
      schema.format = spec.format;
    }
    if (spec.items !== undefined) {
      schema.items = normalizeParam(spec.items).schema;
    }
    if (spec.properties && typeof spec.properties === 'object') {
      const nested: Record<string, ParamJsonSchema> = {};
      const nestedRequired: string[] = [];
      for (const [key, child] of Object.entries(spec.properties as Record<string, unknown>)) {
        const result = normalizeParam(child);
        nested[key] = result.schema;
        if (result.required) {
          nestedRequired.push(key);
        }
      }
      schema.properties = nested;
      if (nestedRequired.length > 0) {
        schema.required = nestedRequired;
      }
    }

    // Optional if `optional: true`, or `required: false`, or the type carried a `?`.
    const typeOptional =
      typeof spec.type === 'string' && stripOptionalMarker(spec.type).optional;
    const required = spec.required === false || spec.optional === true ? false : !typeOptional;

    return { schema, required };
  }

  return { schema: { type: 'string' }, required: true };
}

/** Build the `input_schema` object passed to the LLM for one function. */
export function buildInputSchema(params: Record<string, unknown> | undefined): InputJsonSchema {
  const properties: Record<string, ParamJsonSchema> = {};
  const required: string[] = [];

  for (const [key, value] of Object.entries(params ?? {})) {
    const { schema, required: isRequired } = normalizeParam(value);
    properties[key] = schema;
    if (isRequired) {
      required.push(key);
    }
  }

  return required.length > 0
    ? { type: 'object', properties, required }
    : { type: 'object', properties };
}

/** Names of the params that must be present. */
export function requiredParamNames(params: Record<string, unknown> | undefined): string[] {
  return Object.entries(params ?? {})
    .filter(([, value]) => normalizeParam(value).required)
    .map(([key]) => key);
}

/**
 * Return the required params the LLM failed to supply. A value counts as missing
 * only when absent / null / undefined — an explicit empty string or `false` is
 * treated as a real value, to avoid rejecting legitimate arguments.
 */
export function findMissingRequiredArgs(
  params: Record<string, unknown> | undefined,
  args: Record<string, unknown> | undefined,
): string[] {
  const provided = args ?? {};
  return requiredParamNames(params).filter((name) => {
    const value = provided[name];
    return value === undefined || value === null;
  });
}

export type ArgCoercionResult = {
  args: Record<string, unknown>;
  errors: string[];
};

/**
 * Coerce and validate the LLM's arguments against the declared types before we
 * invoke the handler. LLMs commonly emit `"5"` for a number or `"true"` for a
 * boolean — we coerce those rather than rejecting. Genuinely wrong shapes (e.g.
 * a number where an array is required, or a value outside an `enum`) produce an
 * error string, which the caller hands back to the model to retry.
 *
 * Conservative by design: undeclared keys pass through untouched, and absent
 * values are left to {@link findMissingRequiredArgs}.
 */
export function coerceArgs(
  params: Record<string, unknown> | undefined,
  args: Record<string, unknown> | undefined,
): ArgCoercionResult {
  const out: Record<string, unknown> = { ...(args ?? {}) };
  const errors: string[] = [];

  for (const [key, spec] of Object.entries(params ?? {})) {
    if (!(key in out)) continue;
    let value = out[key];
    if (value === undefined || value === null) continue; // absence handled elsewhere

    const { schema } = normalizeParam(spec);
    const type = schema.type;

    if (type === 'number' || type === 'integer') {
      if (typeof value === 'number') {
        // ok
      } else if (typeof value === 'string' && value.trim() !== '' && Number.isFinite(Number(value))) {
        value = Number(value);
      } else {
        errors.push(`"${key}" must be a ${type}`);
        continue;
      }
      if (type === 'integer' && !Number.isInteger(value)) {
        errors.push(`"${key}" must be an integer`);
        continue;
      }
    } else if (type === 'boolean') {
      if (typeof value === 'boolean') {
        // ok
      } else if (value === 'true') {
        value = true;
      } else if (value === 'false') {
        value = false;
      } else {
        errors.push(`"${key}" must be a boolean`);
        continue;
      }
    } else if (type === 'string') {
      if (typeof value !== 'string') {
        value = String(value);
      }
    } else if (type === 'array') {
      if (!Array.isArray(value)) {
        errors.push(`"${key}" must be an array`);
        continue;
      }
    } else if (type === 'object') {
      if (typeof value !== 'object' || Array.isArray(value)) {
        errors.push(`"${key}" must be an object`);
        continue;
      }
    }

    if (Array.isArray(schema.enum) && schema.enum.length > 0 && !schema.enum.includes(value)) {
      errors.push(`"${key}" must be one of: ${schema.enum.join(', ')}`);
      continue;
    }

    out[key] = value;
  }

  return { args: out, errors };
}
