// Unification algorithm for type checking

import type { Type } from './type-ast';
import { freeTypeVars, formatType, typeRec } from './type-ast';
import type { Substitution } from './type-env';
import { applySubst, composeSubst, emptySubst } from './type-env';
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

