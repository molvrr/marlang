// Type environment for type checking

import type {
  Type,
  TypeScheme,
  TypeClass,
  ClassInstance,
  DataType,
  Constructor,
  TypeAlias,
  Constraint,
  EffectRow,
  EffectDeclaration,
} from './type-ast';
import { freeTypeVars, freeTypeVarsScheme, typeVar } from './type-ast';

// Substitution: maps type variable names to types
export type Substitution = Map<string, Type>;

// Type environment
export type TypeEnv = {
  // Variable bindings: name -> type scheme
  bindings: Map<string, TypeScheme>;
  // ADT definitions: name -> DataType
  dataTypes: Map<string, DataType>;
  // Constructors: name -> (DataType, Constructor)
  constructors: Map<string, { dataType: DataType; constructor: Constructor }>;
  // Type classes: name -> TypeClass
  classes: Map<string, TypeClass>;
  // Class instances: className -> list of instances
  instances: Map<string, ClassInstance[]>;
  // Type aliases: name -> TypeAlias
  aliases: Map<string, TypeAlias>;
  // Effect declarations: name -> EffectDeclaration
  effects: Map<string, EffectDeclaration>;
  // Parent environment (for scoping)
  parent: TypeEnv | null;
};

// ===== Environment Operations =====

export const emptyEnv = (): TypeEnv => ({
  bindings: new Map(),
  dataTypes: new Map(),
  constructors: new Map(),
  classes: new Map(),
  instances: new Map(),
  aliases: new Map(),
  effects: new Map(),
  parent: null,
});

export const childEnv = (parent: TypeEnv): TypeEnv => ({
  bindings: new Map(),
  dataTypes: new Map(),
  constructors: new Map(),
  classes: new Map(),
  instances: new Map(),
  aliases: new Map(),
  effects: new Map(),
  parent,
});

export const extendEnv = (env: TypeEnv, name: string, scheme: TypeScheme): TypeEnv => ({
  ...env,
  bindings: new Map(env.bindings).set(name, scheme),
});

export const extendEnvMany = (
  env: TypeEnv,
  bindings: [string, TypeScheme][]
): TypeEnv => {
  const newBindings = new Map(env.bindings);
  for (const [name, scheme] of bindings) {
    newBindings.set(name, scheme);
  }
  return { ...env, bindings: newBindings };
};

export const lookupVar = (env: TypeEnv, name: string): TypeScheme | undefined => {
  const scheme = env.bindings.get(name);
  if (scheme !== undefined) return scheme;
  if (env.parent) return lookupVar(env.parent, name);
  return undefined;
};

export const lookupDataType = (env: TypeEnv, name: string): DataType | undefined => {
  const dt = env.dataTypes.get(name);
  if (dt !== undefined) return dt;
  if (env.parent) return lookupDataType(env.parent, name);
  return undefined;
};

export const lookupConstructor = (
  env: TypeEnv,
  name: string
): { dataType: DataType; constructor: Constructor } | undefined => {
  const entry = env.constructors.get(name);
  if (entry !== undefined) return entry;
  if (env.parent) return lookupConstructor(env.parent, name);
  return undefined;
};

export const lookupClass = (env: TypeEnv, name: string): TypeClass | undefined => {
  const cls = env.classes.get(name);
  if (cls !== undefined) return cls;
  if (env.parent) return lookupClass(env.parent, name);
  return undefined;
};

export const lookupInstances = (env: TypeEnv, className: string): ClassInstance[] => {
  const instances: ClassInstance[] = [];
  let current: TypeEnv | null = env;
  while (current !== null) {
    const classInstances = current.instances.get(className);
    if (classInstances) {
      instances.push(...classInstances);
    }
    current = current.parent;
  }
  return instances;
};

export const lookupAlias = (env: TypeEnv, name: string): TypeAlias | undefined => {
  const alias = env.aliases.get(name);
  if (alias !== undefined) return alias;
  if (env.parent) return lookupAlias(env.parent, name);
  return undefined;
};

export const lookupEffect = (env: TypeEnv, name: string): EffectDeclaration | undefined => {
  const effect = env.effects.get(name);
  if (effect !== undefined) return effect;
  if (env.parent) return lookupEffect(env.parent, name);
  return undefined;
};

// Lookup an effect operation by name, returning the effect and operation
export const lookupEffectOp = (
  env: TypeEnv,
  opName: string
): { effect: EffectDeclaration; op: { name: string; params: Type[]; result: Type } } | undefined => {
  let current: TypeEnv | null = env;
  while (current !== null) {
    for (const effect of current.effects.values()) {
      for (const op of effect.operations) {
        if (op.name === opName) {
          return { effect, op };
        }
      }
    }
    current = current.parent;
  }
  return undefined;
};

// ===== Environment Modification =====

export const defineDataType = (env: TypeEnv, dt: DataType): TypeEnv => {
  const newDataTypes = new Map(env.dataTypes);
  newDataTypes.set(dt.name, dt);

  const newConstructors = new Map(env.constructors);
  for (const ctor of dt.constructors) {
    newConstructors.set(ctor.name, { dataType: dt, constructor: ctor });
  }

  return { ...env, dataTypes: newDataTypes, constructors: newConstructors };
};

export const defineClass = (env: TypeEnv, cls: TypeClass): TypeEnv => {
  const newClasses = new Map(env.classes);
  newClasses.set(cls.name, cls);
  return { ...env, classes: newClasses };
};

export const defineInstance = (env: TypeEnv, instance: ClassInstance): TypeEnv => {
  const newInstances = new Map(env.instances);
  const existing = newInstances.get(instance.className) || [];
  newInstances.set(instance.className, [...existing, instance]);
  return { ...env, instances: newInstances };
};

export const defineAlias = (env: TypeEnv, alias: TypeAlias): TypeEnv => {
  const newAliases = new Map(env.aliases);
  newAliases.set(alias.name, alias);
  return { ...env, aliases: newAliases };
};

export const defineEffect = (env: TypeEnv, effect: EffectDeclaration): TypeEnv => {
  const newEffects = new Map(env.effects);
  newEffects.set(effect.name, effect);
  return { ...env, effects: newEffects };
};

// ===== Substitution Operations =====

export const emptySubst = (): Substitution => new Map();

export const singleSubst = (name: string, t: Type): Substitution =>
  new Map([[name, t]]);

// Effect row substitution (maps effect row variable names to effect rows)
export type EffectRowSubstitution = Map<string, EffectRow>;

export const emptyEffectRowSubst = (): EffectRowSubstitution => new Map();

// Apply effect row substitution to an effect row
export const applyEffectRowSubst = (
  subst: EffectRowSubstitution,
  row: EffectRow | null
): EffectRow | null => {
  if (row === null) return null;
  switch (row.kind) {
    case 'effect-row-closed':
      return row;
    case 'effect-row-open': {
      const replacement = subst.get(row.rowVar);
      if (replacement === undefined) return row;
      // Combine the effects from this row with the replacement
      switch (replacement.kind) {
        case 'effect-row-closed':
          return {
            kind: 'effect-row-closed',
            effects: [...row.effects, ...replacement.effects],
          };
        case 'effect-row-open':
          return {
            kind: 'effect-row-open',
            effects: [...row.effects, ...replacement.effects],
            rowVar: replacement.rowVar,
          };
      }
    }
  }
};

// Apply substitution to a type
export const applySubst = (subst: Substitution, t: Type): Type => {
  switch (t.kind) {
    case 'type-var': {
      const replacement = subst.get(t.name);
      return replacement !== undefined ? applySubst(subst, replacement) : t;
    }
    case 'type-const':
      return t;
    case 'type-app':
      return {
        kind: 'type-app',
        constructor: applySubst(subst, t.constructor),
        args: t.args.map((arg) => applySubst(subst, arg)),
      };
    case 'type-arrow':
      return {
        kind: 'type-arrow',
        params: t.params.map((p) => applySubst(subst, p)),
        effects: t.effects,  // Effect rows have their own substitution
        result: applySubst(subst, t.result),
      };
    case 'type-rec': {
      // Capture-avoiding substitution: don't substitute the bound variable
      const filteredSubst = new Map(subst);
      filteredSubst.delete(t.var);
      return {
        kind: 'type-rec',
        var: t.var,
        body: applySubst(filteredSubst, t.body),
      };
    }
  }
};

// Apply substitution to a constraint
export const applySubstConstraint = (subst: Substitution, c: Constraint): Constraint => ({
  class: c.class,
  typeArg: applySubst(subst, c.typeArg),
});

// Apply substitution to a type scheme (only to free variables)
export const applySubstScheme = (subst: Substitution, scheme: TypeScheme): TypeScheme => {
  // Remove quantified variables from substitution
  const filteredSubst = new Map(subst);
  for (const v of scheme.quantifiedVars) {
    filteredSubst.delete(v);
  }
  return {
    quantifiedVars: scheme.quantifiedVars,
    qualType: {
      constraints: scheme.qualType.constraints.map((c) =>
        applySubstConstraint(filteredSubst, c)
      ),
      type: applySubst(filteredSubst, scheme.qualType.type),
    },
  };
};

// Apply substitution to environment
export const applySubstEnv = (subst: Substitution, env: TypeEnv): TypeEnv => {
  const newBindings = new Map<string, TypeScheme>();
  for (const [name, scheme] of env.bindings) {
    newBindings.set(name, applySubstScheme(subst, scheme));
  }
  return {
    ...env,
    bindings: newBindings,
    parent: env.parent ? applySubstEnv(subst, env.parent) : null,
  };
};

// Compose substitutions: apply s2, then s1
export const composeSubst = (s1: Substitution, s2: Substitution): Substitution => {
  const result = new Map<string, Type>();
  // Apply s1 to all values in s2
  for (const [name, t] of s2) {
    result.set(name, applySubst(s1, t));
  }
  // Add all bindings from s1 that aren't in s2
  for (const [name, t] of s1) {
    if (!result.has(name)) {
      result.set(name, t);
    }
  }
  return result;
};

// ===== Generalization and Instantiation =====

// Generate fresh type variable names
let typeVarCounter = 0;

export const freshTypeVar = (prefix: string = 't'): Type => {
  return typeVar(`${prefix}${typeVarCounter++}`);
};

export const resetTypeVarCounter = (): void => {
  typeVarCounter = 0;
};

// Free type variables in environment
export const freeTypeVarsEnv = (env: TypeEnv): Set<string> => {
  const result = new Set<string>();
  for (const scheme of env.bindings.values()) {
    for (const v of freeTypeVarsScheme(scheme)) {
      result.add(v);
    }
  }
  if (env.parent) {
    for (const v of freeTypeVarsEnv(env.parent)) {
      result.add(v);
    }
  }
  return result;
};

// Generalize a type to a type scheme (quantify over variables not free in env)
export const generalize = (env: TypeEnv, constraints: Constraint[], t: Type): TypeScheme => {
  const envFree = freeTypeVarsEnv(env);
  const typeFree = freeTypeVars(t);

  // Also include free vars from constraints
  for (const c of constraints) {
    for (const v of freeTypeVars(c.typeArg)) {
      typeFree.add(v);
    }
  }

  const quantified: string[] = [];
  for (const v of typeFree) {
    if (!envFree.has(v)) {
      quantified.push(v);
    }
  }

  // Filter constraints to only those mentioning quantified variables
  const quantifiedSet = new Set(quantified);
  const relevantConstraints = constraints.filter((c) => {
    const cFree = freeTypeVars(c.typeArg);
    for (const v of cFree) {
      if (quantifiedSet.has(v)) return true;
    }
    return false;
  });

  return {
    quantifiedVars: quantified,
    qualType: {
      constraints: relevantConstraints,
      type: t,
    },
  };
};

// Instantiate a type scheme with fresh type variables
export const instantiate = (scheme: TypeScheme): { type: Type; constraints: Constraint[] } => {
  const subst = new Map<string, Type>();
  for (const v of scheme.quantifiedVars) {
    subst.set(v, freshTypeVar());
  }
  return {
    type: applySubst(subst, scheme.qualType.type),
    constraints: scheme.qualType.constraints.map((c) => applySubstConstraint(subst, c)),
  };
};
