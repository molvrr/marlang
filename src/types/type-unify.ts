// Unification algorithm for type checking

import type { Type, EffectRow } from './type-ast';
import { freeTypeVars, formatType, typeRec, effectRowClosed, effectRowOpen, formatEffectRow } from './type-ast';
import type { Substitution, EffectRowSubstitution } from './type-env';
import { applySubst, composeSubst, emptySubst, applyEffectRowSubst } from './type-env';
import type { TypeResult } from './type-errors';
import { ok, err, flatMap } from './type-errors';

// Occurs check: does variable v occur in type t?
const occurs = (v: string, t: Type): boolean => {
  return freeTypeVars(t).has(v);
};

// Unroll a mu-type: (μ a. T) => T[μ a. T / a]
export const unrollMu = (t: Type): Type => {
  if (t.kind !== 'type-rec') return t;
  const subst = new Map([[t.var, t]]);
  return applySubst(subst, t.body);
};

// Generate a unique key for a type (used for cycle detection)
const typeKey = (t: Type): string => formatType(t);

// Internal unify with cycle detection
const unifyInternal = (t1: Type, t2: Type, seen: Set<string>): TypeResult<Substitution> => {
  // Check for cycles (same pair seen before means they're compatible)
  const key = `${typeKey(t1)}~${typeKey(t2)}`;
  const keyReverse = `${typeKey(t2)}~${typeKey(t1)}`;
  if (seen.has(key) || seen.has(keyReverse)) {
    return ok(emptySubst());
  }
  seen.add(key);

  // Handle mu-types by unrolling
  if (t1.kind === 'type-rec') {
    return unifyInternal(unrollMu(t1), t2, seen);
  }
  if (t2.kind === 'type-rec') {
    return unifyInternal(t1, unrollMu(t2), seen);
  }

  // Type variable cases
  if (t1.kind === 'type-var') {
    if (t2.kind === 'type-var' && t1.name === t2.name) {
      return ok(emptySubst());
    }
    if (occurs(t1.name, t2)) {
      // Create mu-type instead of failing on occurs check
      const muType = typeRec(t1.name, t2);
      return ok(new Map([[t1.name, muType]]));
    }
    return ok(new Map([[t1.name, t2]]));
  }

  if (t2.kind === 'type-var') {
    if (occurs(t2.name, t1)) {
      // Create mu-type instead of failing on occurs check
      const muType = typeRec(t2.name, t1);
      return ok(new Map([[t2.name, muType]]));
    }
    return ok(new Map([[t2.name, t1]]));
  }

  // Type constant case
  if (t1.kind === 'type-const' && t2.kind === 'type-const') {
    if (t1.name === t2.name) {
      return ok(emptySubst());
    }
    return err({ kind: 'unification-error', t1, t2 });
  }

  // Type application case
  if (t1.kind === 'type-app' && t2.kind === 'type-app') {
    if (t1.args.length !== t2.args.length) {
      return err({ kind: 'unification-error', t1, t2 });
    }

    return flatMap(unifyInternal(t1.constructor, t2.constructor, seen), (s1) => {
      let subst = s1;
      for (let i = 0; i < t1.args.length; i++) {
        const arg1 = applySubst(subst, t1.args[i]!);
        const arg2 = applySubst(subst, t2.args[i]!);
        const result = unifyInternal(arg1, arg2, seen);
        if (!result.ok) return result;
        subst = composeSubst(result.value, subst);
      }
      return ok(subst);
    });
  }

  // Arrow type case
  if (t1.kind === 'type-arrow' && t2.kind === 'type-arrow') {
    if (t1.params.length !== t2.params.length) {
      return err({
        kind: 'arity-mismatch',
        expected: t1.params.length,
        got: t2.params.length,
        context: 'function type',
      });
    }

    let subst = emptySubst();

    // Unify parameter types
    for (let i = 0; i < t1.params.length; i++) {
      const p1 = applySubst(subst, t1.params[i]!);
      const p2 = applySubst(subst, t2.params[i]!);
      const result = unifyInternal(p1, p2, seen);
      if (!result.ok) return result;
      subst = composeSubst(result.value, subst);
    }

    // Unify result types
    const r1 = applySubst(subst, t1.result);
    const r2 = applySubst(subst, t2.result);
    return flatMap(unifyInternal(r1, r2, seen), (s) => ok(composeSubst(s, subst)));
  }

  // Mismatched type kinds
  return err({ kind: 'unification-error', t1, t2 });
};

// Unify two types, returning a substitution if successful
export const unify = (t1: Type, t2: Type): TypeResult<Substitution> => {
  return unifyInternal(t1, t2, new Set());
};

// Unify with context for better error messages
export const unifyWithContext = (
  t1: Type,
  t2: Type,
  context: string
): TypeResult<Substitution> => {
  const result = unify(t1, t2);
  if (!result.ok && result.error.kind === 'unification-error') {
    return err({ ...result.error, context });
  }
  return result;
};

// Unify a list of type pairs
export const unifyMany = (pairs: [Type, Type][]): TypeResult<Substitution> => {
  let subst = emptySubst();
  for (const [t1, t2] of pairs) {
    const s1 = applySubst(subst, t1);
    const s2 = applySubst(subst, t2);
    const result = unify(s1, s2);
    if (!result.ok) return result;
    subst = composeSubst(result.value, subst);
  }
  return ok(subst);
};

// Match: like unify but only substitutes variables in the first type
// Used for instance matching
export const match = (pattern: Type, target: Type): TypeResult<Substitution> => {
  // Handle mu-types by unrolling
  if (pattern.kind === 'type-rec') {
    return match(unrollMu(pattern), target);
  }
  if (target.kind === 'type-rec') {
    return match(pattern, unrollMu(target));
  }

  if (pattern.kind === 'type-var') {
    return ok(new Map([[pattern.name, target]]));
  }

  if (target.kind === 'type-var') {
    // Can't substitute variables in target
    return err({ kind: 'unification-error', t1: pattern, t2: target });
  }

  if (pattern.kind === 'type-const' && target.kind === 'type-const') {
    if (pattern.name === target.name) {
      return ok(emptySubst());
    }
    return err({ kind: 'unification-error', t1: pattern, t2: target });
  }

  if (pattern.kind === 'type-app' && target.kind === 'type-app') {
    if (pattern.args.length !== target.args.length) {
      return err({ kind: 'unification-error', t1: pattern, t2: target });
    }

    return flatMap(match(pattern.constructor, target.constructor), (s1) => {
      let subst = s1;
      for (let i = 0; i < pattern.args.length; i++) {
        const p = applySubst(subst, pattern.args[i]!);
        const t = target.args[i]!;
        const result = match(p, t);
        if (!result.ok) return result;
        // Check consistency
        for (const [v, type] of result.value) {
          const existing = subst.get(v);
          if (existing !== undefined) {
            // Must unify with existing binding
            const uResult = unify(existing, type);
            if (!uResult.ok) return uResult;
            subst = composeSubst(uResult.value, subst);
          } else {
            subst = new Map([...subst, [v, type]]);
          }
        }
      }
      return ok(subst);
    });
  }

  if (pattern.kind === 'type-arrow' && target.kind === 'type-arrow') {
    if (pattern.params.length !== target.params.length) {
      return err({ kind: 'unification-error', t1: pattern, t2: target });
    }

    let subst = emptySubst();

    for (let i = 0; i < pattern.params.length; i++) {
      const p = applySubst(subst, pattern.params[i]!);
      const t = target.params[i]!;
      const result = match(p, t);
      if (!result.ok) return result;
      subst = composeSubst(result.value, subst);
    }

    const pResult = applySubst(subst, pattern.result);
    return flatMap(match(pResult, target.result), (s) => ok(composeSubst(s, subst)));
  }

  return err({ kind: 'unification-error', t1: pattern, t2: target });
};

// ===== Effect Row Unification =====

// Unify two effect rows, returning an effect row substitution
// Effect row unification is simpler than type unification:
// - Closed rows unify if they have the same effects (order independent)
// - Open rows can unify by binding the row variable
export const unifyEffectRows = (
  r1: EffectRow | null,
  r2: EffectRow | null
): TypeResult<EffectRowSubstitution> => {
  // null means pure (no effects) - treat as empty closed row
  const row1 = r1 || effectRowClosed([]);
  const row2 = r2 || effectRowClosed([]);

  // Both closed: must have same effects (order independent)
  if (row1.kind === 'effect-row-closed' && row2.kind === 'effect-row-closed') {
    const set1 = new Set(row1.effects);
    const set2 = new Set(row2.effects);
    if (set1.size !== set2.size) {
      return err({
        kind: 'unification-error',
        t1: { kind: 'type-const', name: formatEffectRow(row1) },
        t2: { kind: 'type-const', name: formatEffectRow(row2) },
        context: 'effect rows',
      });
    }
    for (const e of set1) {
      if (!set2.has(e)) {
        return err({
          kind: 'unification-error',
          t1: { kind: 'type-const', name: formatEffectRow(row1) },
          t2: { kind: 'type-const', name: formatEffectRow(row2) },
          context: 'effect rows',
        });
      }
    }
    return ok(new Map());
  }

  // row1 open, row2 closed: bind row var to remaining effects
  if (row1.kind === 'effect-row-open' && row2.kind === 'effect-row-closed') {
    // r1 = {e1, e2 | ε} and r2 = {e1, e2, e3}
    // => ε = {e3}
    const remaining = row2.effects.filter((e) => !row1.effects.includes(e));
    // Check that all effects in row1 are present in row2
    for (const e of row1.effects) {
      if (!row2.effects.includes(e)) {
        return err({
          kind: 'unification-error',
          t1: { kind: 'type-const', name: formatEffectRow(row1) },
          t2: { kind: 'type-const', name: formatEffectRow(row2) },
          context: 'effect rows',
        });
      }
    }
    return ok(new Map([[row1.rowVar, effectRowClosed(remaining)]]));
  }

  // row1 closed, row2 open: symmetric case
  if (row1.kind === 'effect-row-closed' && row2.kind === 'effect-row-open') {
    const remaining = row1.effects.filter((e) => !row2.effects.includes(e));
    for (const e of row2.effects) {
      if (!row1.effects.includes(e)) {
        return err({
          kind: 'unification-error',
          t1: { kind: 'type-const', name: formatEffectRow(row1) },
          t2: { kind: 'type-const', name: formatEffectRow(row2) },
          context: 'effect rows',
        });
      }
    }
    return ok(new Map([[row2.rowVar, effectRowClosed(remaining)]]));
  }

  // Both open: unify row variables
  if (row1.kind === 'effect-row-open' && row2.kind === 'effect-row-open') {
    if (row1.rowVar === row2.rowVar) {
      // Same row var, just check that named effects are compatible
      const set1 = new Set(row1.effects);
      const set2 = new Set(row2.effects);
      // Combine effects and keep the shared row var
      const combined = [...new Set([...set1, ...set2])];
      // If they're identical, no substitution needed
      if (set1.size === set2.size && [...set1].every((e) => set2.has(e))) {
        return ok(new Map());
      }
      // Otherwise create a fresh row var for the combined row
      // This is a simplification - full row polymorphism would be more complex
      return ok(new Map());
    }

    // Different row vars: bind one to the other (with effect combination)
    // r1 = {e1 | ε1}, r2 = {e2 | ε2}
    // We need to find effects that are in one but not the other
    const onlyIn1 = row1.effects.filter((e) => !row2.effects.includes(e));
    const onlyIn2 = row2.effects.filter((e) => !row1.effects.includes(e));

    // ε1 = {onlyIn2 | ε2}  (row1's var gets row2's extra effects plus row2's var)
    // or equivalently: ε2 = {onlyIn1 | ε1}
    const subst = new Map<string, EffectRow>();
    if (onlyIn2.length > 0 || true) {
      // Bind ε1 to {onlyIn2 | ε2}
      subst.set(row1.rowVar, effectRowOpen(onlyIn2, row2.rowVar));
    }
    return ok(subst);
  }

  return ok(new Map());
};

// Combine two effect rows (union)
export const combineEffectRows = (r1: EffectRow, r2: EffectRow): EffectRow => {
  const effects1 = r1.effects;
  const effects2 = r2.effects;
  const combined = [...new Set([...effects1, ...effects2])];

  // If either is open, result is open
  if (r1.kind === 'effect-row-open') {
    if (r2.kind === 'effect-row-open' && r1.rowVar !== r2.rowVar) {
      // Two different row vars - keep one (simplified)
      return effectRowOpen(combined, r1.rowVar);
    }
    return effectRowOpen(combined, r1.rowVar);
  }
  if (r2.kind === 'effect-row-open') {
    return effectRowOpen(combined, r2.rowVar);
  }

  // Both closed
  return effectRowClosed(combined);
};

