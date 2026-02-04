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
): { effect: EffectDeclaration; op: { name: string; typeParams: string[]; constraints: Constraint[]; params: Type[]; result: Type } } | undefined => {
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

// Rewrite type constant names in a Type based on a name mapping
// Used for qualified imports to transform type references
const rewriteType = (t: Type, nameMap: Map<string, string>): Type => {
  switch (t.kind) {
    case 'type-var':
      return t;
    case 'type-const': {
      const newName = nameMap.get(t.name);
      return newName ? { kind: 'type-const', name: newName } : t;
    }
    case 'type-app': {
      const newConstructor = rewriteType(t.constructor, nameMap);
      const newArgs = t.args.map(arg => rewriteType(arg, nameMap));
      return { kind: 'type-app', constructor: newConstructor, args: newArgs };
    }
    case 'type-arrow': {
      const newParams = t.params.map(p => rewriteType(p, nameMap));
      const newResult = rewriteType(t.result, nameMap);
      const newEffects = t.effects ? rewriteEffectRow(t.effects, nameMap) : null;
      return { kind: 'type-arrow', params: newParams, effects: newEffects, result: newResult };
    }
    case 'type-rec': {
      const newBody = rewriteType(t.body, nameMap);
      return { kind: 'type-rec', var: t.var, body: newBody };
    }
  }
};

// Rewrite effect row (effect names might need qualification too)
const rewriteEffectRow = (row: EffectRow, nameMap: Map<string, string>): EffectRow => {
  const newEffects = row.effects.map(e => nameMap.get(e) || e);
  if (row.kind === 'effect-row-open') {
    return { kind: 'effect-row-open', effects: newEffects, rowVar: row.rowVar };
  }
  return { kind: 'effect-row-closed', effects: newEffects };
};

// Rewrite constraint type references
const rewriteConstraint = (c: Constraint, nameMap: Map<string, string>): Constraint => ({
  class: c.class,
  typeArg: rewriteType(c.typeArg, nameMap),
});

// Rewrite type scheme
const rewriteTypeScheme = (scheme: TypeScheme, nameMap: Map<string, string>): TypeScheme => ({
  quantifiedVars: scheme.quantifiedVars,
  qualType: {
    constraints: scheme.qualType.constraints.map(c => rewriteConstraint(c, nameMap)),
    type: rewriteType(scheme.qualType.type, nameMap),
  },
});

// Rewrite constructor
const rewriteConstructor = (ctor: Constructor, nameMap: Map<string, string>): Constructor => ({
  name: ctor.name,
  params: ctor.params.map(p => rewriteType(p, nameMap)),
  resultType: rewriteType(ctor.resultType, nameMap),
});

// Rewrite data type definition
const rewriteDataType = (dt: DataType, nameMap: Map<string, string>, newName: string): DataType => ({
  name: newName,
  typeParams: dt.typeParams,
  constructors: dt.constructors.map(c => rewriteConstructor(c, nameMap)),
});

// Rewrite effect declaration
const rewriteEffectDecl = (effect: EffectDeclaration, nameMap: Map<string, string>, newName: string): EffectDeclaration => ({
  name: newName,
  typeParams: effect.typeParams,
  operations: effect.operations.map(op => ({
    name: op.name,
    typeParams: op.typeParams,
    constraints: op.constraints.map(c => rewriteConstraint(c, nameMap)),
    params: op.params.map(p => rewriteType(p, nameMap)),
    result: rewriteType(op.result, nameMap),
  })),
});

// Merge another environment's exports into this one
// Used for imports - merges bindings, dataTypes, constructors, effects, etc.
export const mergeEnv = (
  target: TypeEnv,
  source: TypeEnv,
  options: {
    mode: 'all' | 'selective' | 'qualified';
    selectedNames?: string[];
    qualifier?: string;
  }
): TypeEnv => {
  const { mode, selectedNames = [], qualifier = '' } = options;

  let result = { ...target };

  // Helper to decide if a name should be imported
  const shouldImport = (name: string): boolean => {
    if (mode === 'all') return true;
    if (mode === 'selective') return selectedNames.includes(name);
    if (mode === 'qualified') return true;
    return false;
  };

  // Helper to transform name based on mode
  const transformName = (name: string): string => {
    if (mode === 'qualified') return `${qualifier}/${name}`;
    return name;
  };

  // Build a name mapping for type rewriting (only for qualified imports)
  // Maps original type names to their qualified versions
  // Important: Only rewrite types that are actually NEW in the source module,
  // not built-in types that already exist in the target environment
  const typeNameMap = new Map<string, string>();
  if (mode === 'qualified') {
    // Add data type names (only those not already in target)
    for (const [name] of source.dataTypes) {
      if (!target.dataTypes.has(name)) {
        typeNameMap.set(name, `${qualifier}/${name}`);
      }
    }
    // Add effect names (only those not already in target)
    for (const [name] of source.effects) {
      if (!target.effects.has(name)) {
        typeNameMap.set(name, `${qualifier}/${name}`);
      }
    }
    // Add type alias names (only those not already in target)
    for (const [name] of source.aliases) {
      if (!target.aliases.has(name)) {
        typeNameMap.set(name, `${qualifier}/${name}`);
      }
    }
  }

  // Merge bindings (with type rewriting for qualified imports)
  const newBindings = new Map(target.bindings);
  for (const [name, scheme] of source.bindings) {
    if (shouldImport(name)) {
      const rewrittenScheme = mode === 'qualified' ? rewriteTypeScheme(scheme, typeNameMap) : scheme;
      newBindings.set(transformName(name), rewrittenScheme);
    }
  }
  result = { ...result, bindings: newBindings };

  // Merge dataTypes (with type rewriting for qualified imports)
  const newDataTypes = new Map(target.dataTypes);
  for (const [name, dt] of source.dataTypes) {
    if (shouldImport(name)) {
      const newName = transformName(name);
      const rewrittenDt = mode === 'qualified' ? rewriteDataType(dt, typeNameMap, newName) : dt;
      newDataTypes.set(newName, rewrittenDt);
    }
  }
  result = { ...result, dataTypes: newDataTypes };

  // Merge constructors (with type rewriting for qualified imports)
  const newConstructors = new Map(target.constructors);
  for (const [name, entry] of source.constructors) {
    if (shouldImport(name)) {
      const newName = transformName(name);
      if (mode === 'qualified') {
        // Rewrite constructor and update dataType reference
        const rewrittenCtor = rewriteConstructor(entry.constructor, typeNameMap);
        // Also need the rewritten dataType
        const dtNewName = transformName(entry.dataType.name);
        const rewrittenDt = rewriteDataType(entry.dataType, typeNameMap, dtNewName);
        newConstructors.set(newName, { dataType: rewrittenDt, constructor: rewrittenCtor });
      } else {
        newConstructors.set(newName, entry);
      }
    }
  }
  result = { ...result, constructors: newConstructors };

  // Merge effects (with type rewriting for qualified imports)
  const newEffects = new Map(target.effects);
  for (const [name, effect] of source.effects) {
    if (shouldImport(name)) {
      const newName = transformName(name);
      const rewrittenEffect = mode === 'qualified' ? rewriteEffectDecl(effect, typeNameMap, newName) : effect;
      newEffects.set(newName, rewrittenEffect);
    }
  }
  result = { ...result, effects: newEffects };

  // Merge aliases (with type rewriting for qualified imports)
  const newAliases = new Map(target.aliases);
  for (const [name, alias] of source.aliases) {
    if (shouldImport(name)) {
      const newName = transformName(name);
      if (mode === 'qualified') {
        const rewrittenAlias: TypeAlias = {
          name: newName,
          typeParams: alias.typeParams,
          body: rewriteType(alias.body, typeNameMap),
        };
        newAliases.set(newName, rewrittenAlias);
      } else {
        newAliases.set(newName, alias);
      }
    }
  }
  result = { ...result, aliases: newAliases };

  // Classes and instances are typically global, import all for now
  const newClasses = new Map(target.classes);
  for (const [name, cls] of source.classes) {
    newClasses.set(name, cls);
  }
  result = { ...result, classes: newClasses };

  const newInstances = new Map(target.instances);
  for (const [name, insts] of source.instances) {
    const existing = newInstances.get(name) || [];
    newInstances.set(name, [...existing, ...insts]);
  }
  result = { ...result, instances: newInstances };

  return result;
};
