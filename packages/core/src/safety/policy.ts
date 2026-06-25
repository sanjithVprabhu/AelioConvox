import type { FunctionDefinition, SafetyLevel } from '@aelio/protocol';

export type SafetyOverride = {
  mode?: 'read' | 'write' | 'destructive' | 'blocked';
  require_confirmation?: boolean;
  blocked?: boolean;
};

export type SafetyConfig = {
  defaultMode: 'read_only' | 'full';
  requireConfirmationFor: SafetyLevel[];
  overrides?: Record<string, SafetyOverride>;
};

export type SafetyDecision =
  | { allowed: true; requiresConfirmation: false; effectiveSafety: SafetyLevel }
  | { allowed: true; requiresConfirmation: true; effectiveSafety: SafetyLevel }
  | { allowed: false; reason: string; effectiveSafety: SafetyLevel };

function effectiveSafetyLevel(
  fn: FunctionDefinition,
  config: SafetyConfig,
): SafetyLevel {
  const override = config.overrides?.[fn.name];
  if (override?.blocked || override?.mode === 'blocked') {
    return 'destructive';
  }
  if (override?.mode) {
    return override.mode;
  }
  return fn.safety;
}

export function evaluateSafety(
  fn: FunctionDefinition,
  config: SafetyConfig,
): SafetyDecision {
  const safety = effectiveSafetyLevel(fn, config);

  if (safety === 'destructive') {
    return {
      allowed: false,
      reason: 'This action is not available via chat.',
      effectiveSafety: safety,
    };
  }

  if (safety === 'write') {
    if (config.defaultMode === 'read_only') {
      const override = config.overrides?.[fn.name];
      if (!override || override.mode !== 'write') {
        return {
          allowed: false,
          reason: 'Write actions require explicit configuration approval.',
          effectiveSafety: safety,
        };
      }
    }

    const override = config.overrides?.[fn.name];
    const requiresConfirmation =
      override?.require_confirmation ?? config.requireConfirmationFor.includes('write');

    if (requiresConfirmation) {
      return { allowed: true, requiresConfirmation: true, effectiveSafety: safety };
    }
  }

  return { allowed: true, requiresConfirmation: false, effectiveSafety: safety };
}