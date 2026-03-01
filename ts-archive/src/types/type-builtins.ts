// Built-in types, typeclasses, and instances

import type { Type, TypeScheme, TypeClass, Constraint } from './type-ast';
import {
  typeVar,
  typeConst,
  typeArrow,
  typeApp,
  tInt,
  tFloat,
  tBool,
  tString,
  tUnit,
  tList,
  tFFILibrary,
  tFFIStructDef,
  tFFIStruct,
  tFFIPointer,
  tPid,
  tChannel,
  monotype,
  polytype,
  constrainedType,
} from './type-ast';
import type { EffectDeclaration } from './type-ast';
import type { TypeEnv } from './type-env';
import {
  emptyEnv,
  extendEnv,
  defineClass,
  defineDataType,
  defineEffect,
} from './type-env';

// ===== Built-in Typeclasses =====

export const numClass: TypeClass = {
  name: 'Num',
  typeParam: 'a',
  superclasses: [],
  methods: new Map([
    ['+', typeArrow([typeVar('a'), typeVar('a')], typeVar('a'))],
    ['-', typeArrow([typeVar('a'), typeVar('a')], typeVar('a'))],
    ['*', typeArrow([typeVar('a'), typeVar('a')], typeVar('a'))],
    ['/', typeArrow([typeVar('a'), typeVar('a')], typeVar('a'))],
    ['negate', typeArrow([typeVar('a')], typeVar('a'))],
  ]),
};

export const eqClass: TypeClass = {
  name: 'Eq',
  typeParam: 'a',
  superclasses: [],
  methods: new Map([
    ['=', typeArrow([typeVar('a'), typeVar('a')], tBool)],
  ]),
};

export const ordClass: TypeClass = {
  name: 'Ord',
  typeParam: 'a',
  superclasses: ['Eq'],
  methods: new Map([
    ['<', typeArrow([typeVar('a'), typeVar('a')], tBool)],
    ['>', typeArrow([typeVar('a'), typeVar('a')], tBool)],
    ['<=', typeArrow([typeVar('a'), typeVar('a')], tBool)],
    ['>=', typeArrow([typeVar('a'), typeVar('a')], tBool)],
  ]),
};

export const showClass: TypeClass = {
  name: 'Show',
  typeParam: 'a',
  superclasses: [],
  methods: new Map([
    ['show', typeArrow([typeVar('a')], tString)],
  ]),
};

// ===== Built-in Type Schemes for Functions =====

// Helper for creating Num-constrained binary operation
const numBinOp = (): TypeScheme => constrainedType(
  ['a'],
  [{ class: 'Num', typeArg: typeVar('a') }],
  typeArrow([typeVar('a'), typeVar('a')], typeVar('a'))
);

// Helper for Num-constrained unary operation (for unary minus)
const numUnaryOrBinOp = (): TypeScheme => constrainedType(
  ['a'],
  [{ class: 'Num', typeArg: typeVar('a') }],
  typeArrow([typeVar('a'), typeVar('a')], typeVar('a'))
);

// Helper for Num-constrained unary operation
const numUnaryOp = (): TypeScheme => constrainedType(
  ['a'],
  [{ class: 'Num', typeArg: typeVar('a') }],
  typeArrow([typeVar('a')], typeVar('a'))
);

// Helper for Eq comparison
const eqCompare = (): TypeScheme => constrainedType(
  ['a'],
  [{ class: 'Eq', typeArg: typeVar('a') }],
  typeArrow([typeVar('a'), typeVar('a')], tBool)
);

// Helper for Ord comparison
const ordCompare = (): TypeScheme => constrainedType(
  ['a'],
  [{ class: 'Ord', typeArg: typeVar('a') }],
  typeArrow([typeVar('a'), typeVar('a')], tBool)
);

// ===== Built-in Data Types =====

// Option type: (data (Option a) None (Some a))
const optionDataType = {
  name: 'Option',
  typeParams: ['a'],
  constructors: [
    {
      name: 'None',
      params: [],
      resultType: typeApp(typeConst('Option'), [typeVar('a')]),
    },
    {
      name: 'Some',
      params: [typeVar('a')],
      resultType: typeApp(typeConst('Option'), [typeVar('a')]),
    },
  ],
};

// List type: (data (List a) Nil (Cons a (List a)))
const listDataType = {
  name: 'List',
  typeParams: ['a'],
  constructors: [
    {
      name: 'Nil',
      params: [],
      resultType: typeApp(typeConst('List'), [typeVar('a')]),
    },
    {
      name: 'Cons',
      params: [typeVar('a'), typeApp(typeConst('List'), [typeVar('a')])],
      resultType: typeApp(typeConst('List'), [typeVar('a')]),
    },
  ],
};

// Pair type: (data (Pair a b) (MkPair a b))
const pairDataType = {
  name: 'Pair',
  typeParams: ['a', 'b'],
  constructors: [
    {
      name: 'MkPair',
      params: [typeVar('a'), typeVar('b')],
      resultType: typeApp(typeConst('Pair'), [typeVar('a'), typeVar('b')]),
    },
  ],
};

// ===== Create Initial Type Environment =====

export const createBuiltinEnv = (): TypeEnv => {
  let env = emptyEnv();

  // Add typeclasses
  env = defineClass(env, numClass);
  env = defineClass(env, eqClass);
  env = defineClass(env, ordClass);
  env = defineClass(env, showClass);

  // Add built-in data types
  env = defineDataType(env, optionDataType);
  env = defineDataType(env, listDataType);
  env = defineDataType(env, pairDataType);

  // Add constructor types
  // None : forall a. Option a
  env = extendEnv(env, 'None', polytype(['a'], typeApp(typeConst('Option'), [typeVar('a')])));

  // Some : forall a. a -> Option a
  env = extendEnv(env, 'Some', polytype(
    ['a'],
    typeArrow([typeVar('a')], typeApp(typeConst('Option'), [typeVar('a')]))
  ));

  // Nil : forall a. List a
  env = extendEnv(env, 'Nil', polytype(['a'], typeApp(typeConst('List'), [typeVar('a')])));

  // Cons : forall a. a -> List a -> List a
  env = extendEnv(env, 'Cons', polytype(
    ['a'],
    typeArrow(
      [typeVar('a'), typeApp(typeConst('List'), [typeVar('a')])],
      typeApp(typeConst('List'), [typeVar('a')])
    )
  ));

  // MkPair : forall a b. a -> b -> Pair a b
  env = extendEnv(env, 'MkPair', polytype(
    ['a', 'b'],
    typeArrow(
      [typeVar('a'), typeVar('b')],
      typeApp(typeConst('Pair'), [typeVar('a'), typeVar('b')])
    )
  ));

  // Add arithmetic operators
  env = extendEnv(env, '+', numBinOp());
  env = extendEnv(env, '-', numBinOp());
  env = extendEnv(env, '*', numBinOp());
  env = extendEnv(env, '/', numBinOp());
  env = extendEnv(env, 'negate', numUnaryOp());

  // Add comparison operators
  env = extendEnv(env, '=', eqCompare());
  env = extendEnv(env, '<', ordCompare());
  env = extendEnv(env, '>', ordCompare());
  env = extendEnv(env, '<=', ordCompare());
  env = extendEnv(env, '>=', ordCompare());

  // Add nil constant (Unit type)
  env = extendEnv(env, 'nil', monotype(tUnit));

  // Add list constructor function
  // list : forall a. a... -> List a (variadic, but we type it as a -> List a for now)
  // Actually, we'll handle this specially since it's variadic
  env = extendEnv(env, 'list', polytype(
    ['a'],
    typeArrow([typeVar('a')], tList(typeVar('a')))
  ));

  // Add boolean literals (these are typically keywords but we add them here)
  // In practice, #t and #f would be handled as literals, not variables

  // show : forall a. (Show a) => a -> String
  env = extendEnv(env, 'show', constrainedType(
    ['a'],
    [{ class: 'Show', typeArg: typeVar('a') }],
    typeArrow([typeVar('a')], tString)
  ));

  // FFI Library functions
  // ffi-open : forall a. String -> (List a) -> FFILibrary
  env = extendEnv(env, 'ffi-open', polytype(
    ['a'],
    typeArrow([tString, tList(typeVar('a'))], tFFILibrary)
  ));

  // ffi-call : forall a. FFILibrary -> String -> a (variadic args, polymorphic return)
  env = extendEnv(env, 'ffi-call', polytype(
    ['a'],
    typeArrow([tFFILibrary, tString], typeVar('a'))
  ));

  // ffi-close : FFILibrary -> Unit
  env = extendEnv(env, 'ffi-close', monotype(
    typeArrow([tFFILibrary], tUnit)
  ));

  // FFI Struct functions
  // ffi-struct : forall a. String -> (List a) -> FFIStructDef
  env = extendEnv(env, 'ffi-struct', polytype(
    ['a'],
    typeArrow([tString, tList(typeVar('a'))], tFFIStructDef)
  ));

  // struct-new : forall a. FFIStructDef -> a -> FFIStruct (variadic args)
  env = extendEnv(env, 'struct-new', polytype(
    ['a'],
    typeArrow([tFFIStructDef, typeVar('a')], tFFIStruct)
  ));

  // struct-get : forall a. FFIStruct -> String -> a (polymorphic return)
  env = extendEnv(env, 'struct-get', polytype(
    ['a'],
    typeArrow([tFFIStruct, tString], typeVar('a'))
  ));

  // struct-set : forall a. FFIStruct -> String -> a -> Unit
  env = extendEnv(env, 'struct-set', polytype(
    ['a'],
    typeArrow([tFFIStruct, tString, typeVar('a')], tUnit)
  ));

  // === Math builtins ===

  // abs : forall a. (Num a) => a -> a
  env = extendEnv(env, 'abs', numUnaryOp());

  // rem : forall a. (Num a) => a -> a -> a
  env = extendEnv(env, 'rem', numBinOp());

  // mod : forall a. (Num a) => a -> a -> a
  env = extendEnv(env, 'mod', numBinOp());

  // floor : Float -> Int
  env = extendEnv(env, 'floor', monotype(typeArrow([tFloat], tInt)));

  // ceil : Float -> Int
  env = extendEnv(env, 'ceil', monotype(typeArrow([tFloat], tInt)));

  // round : Float -> Int
  env = extendEnv(env, 'round', monotype(typeArrow([tFloat], tInt)));

  // truncate : Float -> Int
  env = extendEnv(env, 'truncate', monotype(typeArrow([tFloat], tInt)));

  // pow : Float -> Float -> Float
  env = extendEnv(env, 'pow', monotype(typeArrow([tFloat, tFloat], tFloat)));

  // sqrt : Float -> Float
  env = extendEnv(env, 'sqrt', monotype(typeArrow([tFloat], tFloat)));

  // exp : Float -> Float
  env = extendEnv(env, 'exp', monotype(typeArrow([tFloat], tFloat)));

  // log : Float -> Float
  env = extendEnv(env, 'log', monotype(typeArrow([tFloat], tFloat)));

  // log10 : Float -> Float
  env = extendEnv(env, 'log10', monotype(typeArrow([tFloat], tFloat)));

  // log2 : Float -> Float
  env = extendEnv(env, 'log2', monotype(typeArrow([tFloat], tFloat)));

  // sin : Float -> Float
  env = extendEnv(env, 'sin', monotype(typeArrow([tFloat], tFloat)));

  // cos : Float -> Float
  env = extendEnv(env, 'cos', monotype(typeArrow([tFloat], tFloat)));

  // tan : Float -> Float
  env = extendEnv(env, 'tan', monotype(typeArrow([tFloat], tFloat)));

  // asin : Float -> Float
  env = extendEnv(env, 'asin', monotype(typeArrow([tFloat], tFloat)));

  // acos : Float -> Float
  env = extendEnv(env, 'acos', monotype(typeArrow([tFloat], tFloat)));

  // atan : Float -> Float
  env = extendEnv(env, 'atan', monotype(typeArrow([tFloat], tFloat)));

  // atan2 : Float -> Float -> Float
  env = extendEnv(env, 'atan2', monotype(typeArrow([tFloat, tFloat], tFloat)));

  // sinh : Float -> Float
  env = extendEnv(env, 'sinh', monotype(typeArrow([tFloat], tFloat)));

  // cosh : Float -> Float
  env = extendEnv(env, 'cosh', monotype(typeArrow([tFloat], tFloat)));

  // tanh : Float -> Float
  env = extendEnv(env, 'tanh', monotype(typeArrow([tFloat], tFloat)));

  // is-nan : Float -> Bool
  env = extendEnv(env, 'is-nan', monotype(typeArrow([tFloat], tBool)));

  // is-infinite : Float -> Bool
  env = extendEnv(env, 'is-infinite', monotype(typeArrow([tFloat], tBool)));

  // is-finite : Float -> Bool
  env = extendEnv(env, 'is-finite', monotype(typeArrow([tFloat], tBool)));

  // === String builtins ===

  // string-length : String -> Int
  env = extendEnv(env, 'string-length', monotype(typeArrow([tString], tInt)));

  // string-concat : String -> String -> String
  env = extendEnv(env, 'string-concat', monotype(typeArrow([tString, tString], tString)));

  // string-append : String... -> String (variadic, typed as binary)
  env = extendEnv(env, 'string-append', monotype(typeArrow([tString, tString], tString)));

  // string-is-empty : String -> Bool
  env = extendEnv(env, 'string-is-empty', monotype(typeArrow([tString], tBool)));

  // string-reverse : String -> String
  env = extendEnv(env, 'string-reverse', monotype(typeArrow([tString], tString)));

  // string-repeat : String -> Int -> String
  env = extendEnv(env, 'string-repeat', monotype(typeArrow([tString, tInt], tString)));

  // string-to-upper : String -> String
  env = extendEnv(env, 'string-to-upper', monotype(typeArrow([tString], tString)));

  // string-to-lower : String -> String
  env = extendEnv(env, 'string-to-lower', monotype(typeArrow([tString], tString)));

  // string-trim : String -> String
  env = extendEnv(env, 'string-trim', monotype(typeArrow([tString], tString)));

  // string-trim-start : String -> String
  env = extendEnv(env, 'string-trim-start', monotype(typeArrow([tString], tString)));

  // string-trim-end : String -> String
  env = extendEnv(env, 'string-trim-end', monotype(typeArrow([tString], tString)));

  // string-starts-with : String -> String -> Bool
  env = extendEnv(env, 'string-starts-with', monotype(typeArrow([tString, tString], tBool)));

  // string-ends-with : String -> String -> Bool
  env = extendEnv(env, 'string-ends-with', monotype(typeArrow([tString, tString], tBool)));

  // string-contains : String -> String -> Bool
  env = extendEnv(env, 'string-contains', monotype(typeArrow([tString, tString], tBool)));

  // string-index-of : String -> String -> Option Int
  env = extendEnv(env, 'string-index-of', monotype(
    typeArrow([tString, tString], typeApp(typeConst('Option'), [tInt]))
  ));

  // string-substring : String -> Int -> Int -> String
  env = extendEnv(env, 'string-substring', monotype(typeArrow([tString, tInt, tInt], tString)));

  // string-char-at : String -> Int -> Option String
  env = extendEnv(env, 'string-char-at', monotype(
    typeArrow([tString, tInt], typeApp(typeConst('Option'), [tString]))
  ));

  // string-split : String -> String -> List String
  env = extendEnv(env, 'string-split', monotype(
    typeArrow([tString, tString], tList(tString))
  ));

  // string-join : String -> List String -> String
  env = extendEnv(env, 'string-join', monotype(
    typeArrow([tString, tList(tString)], tString)
  ));

  // string-replace : String -> String -> String -> String
  env = extendEnv(env, 'string-replace', monotype(typeArrow([tString, tString, tString], tString)));

  // string-replace-all : String -> String -> String -> String
  env = extendEnv(env, 'string-replace-all', monotype(typeArrow([tString, tString, tString], tString)));

  // string-pad-start : String -> Int -> String -> String
  env = extendEnv(env, 'string-pad-start', monotype(typeArrow([tString, tInt, tString], tString)));

  // string-pad-end : String -> Int -> String -> String
  env = extendEnv(env, 'string-pad-end', monotype(typeArrow([tString, tInt, tString], tString)));

  // char-code : String -> Int
  env = extendEnv(env, 'char-code', monotype(typeArrow([tString], tInt)));

  // char-from-code : Int -> String
  env = extendEnv(env, 'char-from-code', monotype(typeArrow([tInt], tString)));

  // === Concurrency builtins ===

  // run-fibers : forall a. (-> Unit a) -> a
  env = extendEnv(env, 'run-fibers', polytype(
    ['a'],
    typeArrow([typeArrow([], typeVar('a'))], typeVar('a'))
  ));

  // error : forall a. String -> a
  env = extendEnv(env, 'error', polytype(
    ['a'],
    typeArrow([tString], typeVar('a'))
  ));

  // === Concurrency effects ===

  // Fiber effect: spawn, yield, self
  const fiberEffect: EffectDeclaration = {
    name: 'Fiber',
    typeParams: [],
    operations: [
      { name: 'fiber-spawn', typeParams: [], constraints: [], params: [typeArrow([], tUnit)], result: tPid },
      { name: 'fiber-yield', typeParams: [], constraints: [], params: [], result: tUnit },
      { name: 'fiber-self', typeParams: [], constraints: [], params: [], result: tPid },
    ],
  };
  env = defineEffect(env, fiberEffect);

  // Channel effect: new, send, recv, close
  const channelEffect: EffectDeclaration = {
    name: 'Channel',
    typeParams: [],
    operations: [
      { name: 'chan-new', typeParams: [], constraints: [], params: [tInt], result: tChannel },
      { name: 'chan-send', typeParams: ['a'], constraints: [], params: [tChannel, typeVar('a')], result: tUnit },
      { name: 'chan-recv', typeParams: ['a'], constraints: [], params: [tChannel], result: typeVar('a') },
      { name: 'chan-close', typeParams: [], constraints: [], params: [tChannel], result: tUnit },
    ],
  };
  env = defineEffect(env, channelEffect);

  // Actor effect: send, receive, monitor, link, trap-exit
  const actorEffect: EffectDeclaration = {
    name: 'Actor',
    typeParams: [],
    operations: [
      { name: 'actor-send', typeParams: ['a'], constraints: [], params: [tPid, typeVar('a')], result: tUnit },
      { name: 'actor-receive', typeParams: ['a'], constraints: [], params: [], result: typeVar('a') },
      { name: 'actor-receive-match', typeParams: ['a'], constraints: [], params: [typeArrow([typeVar('a')], tBool)], result: typeVar('a') },
      { name: 'actor-monitor', typeParams: [], constraints: [], params: [tPid], result: tUnit },
      { name: 'actor-link', typeParams: [], constraints: [], params: [tPid], result: tUnit },
      { name: 'actor-unlink', typeParams: [], constraints: [], params: [tPid], result: tUnit },
      { name: 'actor-trap-exit', typeParams: [], constraints: [], params: [tBool], result: tUnit },
    ],
  };
  env = defineEffect(env, actorEffect);

  // IO effect (built-in default handlers): print, println, read-line, read-file, write-file
  const ioEffect: EffectDeclaration = {
    name: 'IO',
    typeParams: [],
    operations: [
      { name: 'print', typeParams: ['a'], constraints: [{ class: 'Show', typeArg: typeVar('a') }], params: [typeVar('a')], result: tUnit },
      { name: 'println', typeParams: ['a'], constraints: [{ class: 'Show', typeArg: typeVar('a') }], params: [typeVar('a')], result: tUnit },
      { name: 'read-line', typeParams: [], constraints: [], params: [], result: tString },
      { name: 'read-file', typeParams: [], constraints: [], params: [tString], result: tString },
      { name: 'write-file', typeParams: [], constraints: [], params: [tString, tString], result: tUnit },
    ],
  };
  env = defineEffect(env, ioEffect);

  return env;
};

// ===== Instance Checking =====

// Check if a type has an instance for a class
// Returns the set of constraints that need to be satisfied
export const hasInstance = (
  className: string,
  type: Type
): { has: boolean; neededConstraints: Constraint[] } => {
  // Built-in instances
  if (className === 'Num') {
    if (type.kind === 'type-const' && (type.name === 'Int' || type.name === 'Float')) {
      return { has: true, neededConstraints: [] };
    }
    if (type.kind === 'type-var') {
      // Type variable - need to propagate constraint
      return { has: true, neededConstraints: [{ class: 'Num', typeArg: type }] };
    }
    return { has: false, neededConstraints: [] };
  }

  if (className === 'Eq') {
    if (type.kind === 'type-const') {
      if (['Int', 'Float', 'Bool', 'String', 'Unit'].includes(type.name)) {
        return { has: true, neededConstraints: [] };
      }
    }
    if (type.kind === 'type-var') {
      return { has: true, neededConstraints: [{ class: 'Eq', typeArg: type }] };
    }
    // List a is Eq if a is Eq
    if (type.kind === 'type-app' && type.constructor.kind === 'type-const' && type.constructor.name === 'List') {
      const elemType = type.args[0];
      if (elemType) {
        const elemCheck = hasInstance('Eq', elemType);
        return {
          has: elemCheck.has,
          neededConstraints: elemCheck.neededConstraints,
        };
      }
    }
    return { has: false, neededConstraints: [] };
  }

  if (className === 'Ord') {
    if (type.kind === 'type-const') {
      if (['Int', 'Float', 'String'].includes(type.name)) {
        return { has: true, neededConstraints: [] };
      }
    }
    if (type.kind === 'type-var') {
      return { has: true, neededConstraints: [{ class: 'Ord', typeArg: type }] };
    }
    return { has: false, neededConstraints: [] };
  }

  if (className === 'Show') {
    if (type.kind === 'type-const') {
      if (['Int', 'Float', 'Bool', 'String', 'Unit'].includes(type.name)) {
        return { has: true, neededConstraints: [] };
      }
      // User-defined ADTs automatically have Show
      return { has: true, neededConstraints: [] };
    }
    if (type.kind === 'type-var') {
      return { has: true, neededConstraints: [{ class: 'Show', typeArg: type }] };
    }
    // Parameterized types (List a, Option a) have Show if args have Show
    if (type.kind === 'type-app') {
      const neededConstraints: Constraint[] = [];
      for (const arg of type.args) {
        const argCheck = hasInstance('Show', arg);
        if (!argCheck.has) return { has: false, neededConstraints: [] };
        neededConstraints.push(...argCheck.neededConstraints);
      }
      return { has: true, neededConstraints };
    }
    return { has: false, neededConstraints: [] };
  }

  // Unknown class - assume it doesn't have instance
  return { has: false, neededConstraints: [] };
};

// Resolve constraints and return residual (unsatisfied) constraints
export const resolveConstraints = (constraints: Constraint[]): Constraint[] => {
  const residual: Constraint[] = [];

  for (const c of constraints) {
    const { has, neededConstraints } = hasInstance(c.class, c.typeArg);
    if (!has) {
      residual.push(c);
    } else {
      // Add any needed constraints from the instance
      for (const nc of neededConstraints) {
        // Check if this constraint is already satisfied or in residual
        const alreadyPresent = residual.some(
          (r) => r.class === nc.class &&
            r.typeArg.kind === 'type-var' &&
            nc.typeArg.kind === 'type-var' &&
            r.typeArg.name === nc.typeArg.name
        );
        if (!alreadyPresent) {
          residual.push(nc);
        }
      }
    }
  }

  // Filter out constraints on concrete types that have instances
  return residual.filter((c) => {
    const { has } = hasInstance(c.class, c.typeArg);
    // Keep constraint if type is a variable or if no instance exists
    return c.typeArg.kind === 'type-var' || !has;
  });
};
