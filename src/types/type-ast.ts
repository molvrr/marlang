// Type system AST definitions

import type { Expr } from '../core/types';

// ===== Effect System Types =====

// Effect row - represents a set of effects a function may perform
// Closed: {Console, State} - exactly these effects
// Open: {Console | ε} - Console plus whatever ε represents (for polymorphism)
export type EffectRow =
  | { kind: 'effect-row-closed'; effects: string[] }                    // Closed row
  | { kind: 'effect-row-open'; effects: string[]; rowVar: string };     // Open row with variable

// Effect operation (single operation in an effect declaration)
export type EffectOp = {
  name: string;
  typeParams: string[];       // Quantified type variables (e.g., ['a'])
  constraints: Constraint[];  // Typeclass constraints (e.g., [{class:'Show', typeArg:a}])
  params: Type[];             // Parameter types
  result: Type;               // Return type
};

// Effect declaration (defines an effect with its operations)
export type EffectDeclaration = {
  name: string;
  typeParams: string[];    // Type parameters for parameterized effects like (State a)
  operations: EffectOp[];
};

// ===== Core Type Representation =====

export type Type =
  | { kind: 'type-var'; name: string }                   // Type variable: a, b, etc.
  | { kind: 'type-const'; name: string }                 // Concrete type: Int, Float, Bool, String
  | { kind: 'type-app'; constructor: Type; args: Type[] } // Type application: (List a), (Option Int)
  | { kind: 'type-arrow'; params: Type[]; effects: EffectRow | null; result: Type }  // Function type with effects
  | { kind: 'type-rec'; var: string; body: Type }        // Recursive type: μ var. body

// Typeclass constraint: (Num a), (Eq a), etc.
export type Constraint = {
  class: string;
  typeArg: Type;
}

// Qualified type: constraints => type
export type QualType = {
  constraints: Constraint[];
  type: Type;
}

// Type scheme with quantified variables: forall a b. constraints => type
export type TypeScheme = {
  quantifiedVars: string[];
  qualType: QualType;
}

// Data constructor: (Some a), None, (Cons a (List a))
export type Constructor = {
  name: string;
  params: Type[];       // Constructor argument types
  resultType: Type;     // The result type (e.g., (Option a))
}

// Algebraic data type definition
export type DataType = {
  name: string;
  typeParams: string[];  // Type parameters: a, b, etc.
  constructors: Constructor[];
}

// Typeclass definition
export type TypeClass = {
  name: string;
  typeParam: string;              // The 'a' in (class (Num a) ...)
  superclasses: string[];         // Superclass constraints
  methods: Map<string, Type>;     // Method signatures
}

// Typeclass instance
export type ClassInstance = {
  className: string;
  forType: Type;                  // The concrete type (Int, Float, etc.)
  methods: Map<string, Expr>;     // Method implementations (AST expressions)
}

// Pattern for match expressions
export type Pattern =
  | { kind: 'pattern-wildcard' }                         // _
  | { kind: 'pattern-var'; name: string }                // x
  | { kind: 'pattern-literal'; value: number | string | boolean }  // 42, "hello", true
  | { kind: 'pattern-constructor'; name: string; args: Pattern[] } // (Some x), None, (Cons h t)

// Match branch
export type MatchBranch = {
  pattern: Pattern;
  body: Expr;
}

// Type annotation for define expressions
export type TypeAnnotation = {
  name: string;
  type: Type;
}

// Type alias definition
export type TypeAlias = {
  name: string;
  typeParams: string[];
  body: Type;
}

// ===== Type Constructors =====

export const typeVar = (name: string): Type => ({ kind: 'type-var', name });

export const typeConst = (name: string): Type => ({ kind: 'type-const', name });

export const typeApp = (constructor: Type, args: Type[]): Type => ({
  kind: 'type-app',
  constructor,
  args,
});

export const typeArrow = (params: Type[], result: Type, effects: EffectRow | null = null): Type => ({
  kind: 'type-arrow',
  params,
  effects,
  result,
});

// ===== Effect Row Constructors =====

export const effectRowClosed = (effects: string[]): EffectRow => ({
  kind: 'effect-row-closed',
  effects,
});

export const effectRowOpen = (effects: string[], rowVar: string): EffectRow => ({
  kind: 'effect-row-open',
  effects,
  rowVar,
});

export const effectRowEmpty: EffectRow = effectRowClosed([]);

// Fresh effect row variable counter
let effectRowVarCounter = 0;

export const freshEffectRowVar = (prefix: string = 'ε'): string => {
  return `${prefix}${effectRowVarCounter++}`;
};

export const resetEffectRowVarCounter = (): void => {
  effectRowVarCounter = 0;
};

export const typeRec = (varName: string, body: Type): Type => ({
  kind: 'type-rec',
  var: varName,
  body,
});

// ===== Built-in Types =====

export const tInt: Type = typeConst('Int');
export const tFloat: Type = typeConst('Float');
export const tBool: Type = typeConst('Bool');
export const tString: Type = typeConst('String');
export const tUnit: Type = typeConst('Unit');

// Helper to create List type
export const tList = (elem: Type): Type => typeApp(typeConst('List'), [elem]);

// FFI Types
export const tFFILibrary: Type = typeConst('FFILibrary');
export const tFFIStructDef: Type = typeConst('FFIStructDef');
export const tFFIStruct: Type = typeConst('FFIStruct');
export const tFFIPointer: Type = typeConst('FFIPointer');

// Concurrency types
export const tPid: Type = typeConst('Pid');
export const tChannel: Type = typeConst('Channel');

// ===== Type Scheme Helpers =====

export const monotype = (t: Type): TypeScheme => ({
  quantifiedVars: [],
  qualType: { constraints: [], type: t },
});

export const polytype = (vars: string[], t: Type): TypeScheme => ({
  quantifiedVars: vars,
  qualType: { constraints: [], type: t },
});

export const constrainedType = (
  vars: string[],
  constraints: Constraint[],
  t: Type
): TypeScheme => ({
  quantifiedVars: vars,
  qualType: { constraints, type: t },
});

// ===== Free Type Variables =====

// Free effect row variables in an effect row
export const freeEffectRowVars = (row: EffectRow | null): Set<string> => {
  if (row === null) return new Set();
  switch (row.kind) {
    case 'effect-row-closed':
      return new Set();
    case 'effect-row-open':
      return new Set([row.rowVar]);
  }
};

export const freeTypeVars = (t: Type): Set<string> => {
  switch (t.kind) {
    case 'type-var':
      return new Set([t.name]);
    case 'type-const':
      return new Set();
    case 'type-app': {
      const result = freeTypeVars(t.constructor);
      for (const arg of t.args) {
        for (const v of freeTypeVars(arg)) {
          result.add(v);
        }
      }
      return result;
    }
    case 'type-arrow': {
      const result = new Set<string>();
      for (const param of t.params) {
        for (const v of freeTypeVars(param)) {
          result.add(v);
        }
      }
      for (const v of freeTypeVars(t.result)) {
        result.add(v);
      }
      // Include effect row variables
      for (const v of freeEffectRowVars(t.effects)) {
        result.add(v);
      }
      return result;
    }
    case 'type-rec': {
      // The bound variable is not free in a recursive type
      const bodyFree = freeTypeVars(t.body);
      bodyFree.delete(t.var);
      return bodyFree;
    }
  }
};

export const freeTypeVarsScheme = (scheme: TypeScheme): Set<string> => {
  const bodyFree = freeTypeVars(scheme.qualType.type);
  for (const v of scheme.quantifiedVars) {
    bodyFree.delete(v);
  }
  // Also include free vars from constraints
  for (const c of scheme.qualType.constraints) {
    for (const v of freeTypeVars(c.typeArg)) {
      if (!scheme.quantifiedVars.includes(v)) {
        bodyFree.add(v);
      }
    }
  }
  return bodyFree;
};

// ===== Type Printing =====

export const formatEffectRow = (row: EffectRow | null): string => {
  if (row === null) return '';
  switch (row.kind) {
    case 'effect-row-closed':
      if (row.effects.length === 0) return '{}';
      return `{${row.effects.join(', ')}}`;
    case 'effect-row-open':
      if (row.effects.length === 0) return `{${row.rowVar}}`;
      return `{${row.effects.join(', ')} | ${row.rowVar}}`;
  }
};

export const formatType = (t: Type): string => {
  switch (t.kind) {
    case 'type-var':
      return t.name;
    case 'type-const':
      return t.name;
    case 'type-app':
      if (t.args.length === 0) {
        return formatType(t.constructor);
      }
      return `(${formatType(t.constructor)} ${t.args.map(formatType).join(' ')})`;
    case 'type-arrow': {
      const paramsStr = t.params.map(formatType).join(' ');
      const effectsStr = t.effects ? ` ${formatEffectRow(t.effects)}` : '';
      if (t.params.length === 0) {
        return `(->${effectsStr} ${formatType(t.result)})`;
      }
      return `(-> ${paramsStr}${effectsStr} ${formatType(t.result)})`;
    }
    case 'type-rec':
      return `(mu ${t.var} ${formatType(t.body)})`;
  }
};

export const formatConstraint = (c: Constraint): string => {
  return `(${c.class} ${formatType(c.typeArg)})`;
};

export const formatScheme = (scheme: TypeScheme): string => {
  const { quantifiedVars, qualType } = scheme;
  let result = '';

  if (quantifiedVars.length > 0) {
    result += `(forall ${quantifiedVars.join(' ')} `;
  }

  if (qualType.constraints.length > 0) {
    result += `(=> ${qualType.constraints.map(formatConstraint).join(' ')} `;
    result += formatType(qualType.type);
    result += ')';
  } else {
    result += formatType(qualType.type);
  }

  if (quantifiedVars.length > 0) {
    result += ')';
  }

  return result;
};

export const formatPattern = (p: Pattern): string => {
  switch (p.kind) {
    case 'pattern-wildcard':
      return '_';
    case 'pattern-var':
      return p.name;
    case 'pattern-literal':
      if (typeof p.value === 'string') return `"${p.value}"`;
      if (typeof p.value === 'boolean') return p.value ? '#t' : '#f';
      return String(p.value);
    case 'pattern-constructor':
      if (p.args.length === 0) return p.name;
      return `(${p.name} ${p.args.map(formatPattern).join(' ')})`;
  }
};
