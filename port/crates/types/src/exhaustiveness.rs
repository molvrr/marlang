//! Pattern exhaustiveness checking using the Maranget algorithm.
//!
//! Based on Luc Maranget, "Warnings for Pattern Matching" (2007).
//!
//! The core idea: a match expression is exhaustive if and only if the wildcard
//! pattern `_` is *not useful* — that is, every possible value is already
//! covered by at least one existing arm. When the match is not exhaustive,
//! we construct *witness* patterns showing example uncovered values.

use crate::ast::{Constructor, DataType, Pattern, Type};
use crate::env::{TypeEnv, lookup_constructor, lookup_data_type};

/// A row in the pattern matrix — one pattern per column.
type PatternRow = Vec<Pattern>;

/// The pattern matrix: each row corresponds to one match arm.
type PatternMatrix = Vec<PatternRow>;

/// Check that `patterns` (from a match expression on `scrutinee_type`) are
/// exhaustive. Returns `Ok(())` if exhaustive, or `Err(witnesses)` with
/// example uncovered patterns.
pub fn check_exhaustiveness(
    env: &TypeEnv,
    scrutinee_type: &Type,
    patterns: &[Pattern],
) -> Result<(), Vec<Pattern>> {
    // Build a single-column matrix from the match arms.
    let mut matrix: PatternMatrix = patterns.iter().map(|p| vec![p.clone()]).collect();
    let col_types = vec![scrutinee_type.clone()];

    // Iteratively find all missing witnesses. After finding one, add it to
    // the matrix and search again. This gives the user all missing patterns
    // (similar to the old behavior), not just the first one found.
    let mut all_witnesses: Vec<Pattern> = Vec::new();

    // Safety limit to prevent infinite loops on pathological cases.
    const MAX_WITNESSES: usize = 32;

    loop {
        if all_witnesses.len() >= MAX_WITNESSES {
            break;
        }
        let wildcard_row = vec![Pattern::Wildcard];
        match is_useful(env, &matrix, &wildcard_row, &col_types) {
            Useful::No => break,
            Useful::Yes(witness) => {
                if witness.is_empty() {
                    all_witnesses.push(Pattern::Wildcard);
                    break;
                }
                // The witness has one pattern (single-column matrix).
                let pat = witness.into_iter().next().unwrap();
                matrix.push(vec![pat.clone()]);
                all_witnesses.push(pat);
            }
        }
    }

    if all_witnesses.is_empty() {
        Ok(())
    } else {
        Err(all_witnesses)
    }
}

/// Result of a usefulness check.
enum Useful {
    /// The test row is not useful — all values it matches are already covered.
    No,
    /// The test row is useful. The `Vec<Pattern>` contains witness patterns
    /// (one per column of the original matrix) demonstrating an uncovered value.
    Yes(Vec<Pattern>),
}

/// Core algorithm: is `test_row` useful given the existing `matrix`?
///
/// The matrix and test_row have the same number of columns. `col_types[i]`
/// is the type of column `i`, used to look up constructors.
fn is_useful(
    env: &TypeEnv,
    matrix: &PatternMatrix,
    test_row: &PatternRow,
    col_types: &[Type],
) -> Useful {
    let ncols = col_types.len();

    // Base case: zero columns.
    if ncols == 0 {
        return if matrix.is_empty() {
            // No existing rows cover the empty tuple → test row is useful.
            Useful::Yes(vec![])
        } else {
            Useful::No
        };
    }

    // Pick the first column to decompose.
    let col = 0;
    let col_type = &col_types[col];

    match &test_row[col] {
        // ── Test pattern is a constructor ──────────────────────────────
        Pattern::Constructor { name, args } => {
            let arity = args.len();
            let spec_matrix = specialize_matrix(matrix, name, arity);

            // Build the specialized test row: replace column `col` with
            // the constructor's sub-patterns, keep remaining columns.
            let mut spec_test: PatternRow = args.clone();
            spec_test.extend_from_slice(&test_row[col + 1..]);

            // Build the column types for the specialized matrix.
            let spec_col_types = specialize_col_types(env, col_types, name, arity);

            match is_useful(env, &spec_matrix, &spec_test, &spec_col_types) {
                Useful::No => Useful::No,
                Useful::Yes(sub_witness) => {
                    // Reconstruct: the first `arity` witness patterns become
                    // arguments to the constructor; the rest are the remaining
                    // columns.
                    let (ctor_args, rest) = sub_witness.split_at(arity);
                    let mut witness = vec![Pattern::Constructor {
                        name: name.clone(),
                        args: ctor_args.to_vec(),
                    }];
                    witness.extend_from_slice(rest);
                    Useful::Yes(witness)
                }
            }
        }

        // ── Test pattern is a wildcard or variable ─────────────────────
        Pattern::Wildcard | Pattern::Var(_) => {
            // Collect the set of constructors that appear in column `col`
            // of the existing matrix.
            let head_ctors = collect_head_constructors(matrix);

            if let Some(all_ctors) = all_constructors_of_type(env, col_type) {
                // The type is an ADT. Check if all constructors are present.
                let is_complete = all_ctors.iter().all(|c| head_ctors.contains(&c.name));

                if is_complete {
                    // All constructors are present → split on each and check.
                    for ctor in &all_ctors {
                        let arity = ctor.params.len();
                        let spec_matrix = specialize_matrix(matrix, &ctor.name, arity);

                        let mut spec_test: PatternRow = vec![Pattern::Wildcard; arity];
                        spec_test.extend_from_slice(&test_row[col + 1..]);

                        let spec_col_types =
                            specialize_col_types(env, col_types, &ctor.name, arity);

                        if let Useful::Yes(sub_witness) =
                            is_useful(env, &spec_matrix, &spec_test, &spec_col_types)
                        {
                            let (ctor_args, rest) = sub_witness.split_at(arity);
                            let mut witness = vec![Pattern::Constructor {
                                name: ctor.name.clone(),
                                args: ctor_args.to_vec(),
                            }];
                            witness.extend_from_slice(rest);
                            return Useful::Yes(witness);
                        }
                    }
                    Useful::No
                } else {
                    // Not all constructors present → use the default matrix.
                    let default_mat = default_matrix(matrix);

                    let mut default_test: PatternRow = Vec::new();
                    default_test.extend_from_slice(&test_row[col + 1..]);

                    let default_col_types = col_types[col + 1..].to_vec();

                    match is_useful(env, &default_mat, &default_test, &default_col_types) {
                        Useful::No => Useful::No,
                        Useful::Yes(sub_witness) => {
                            // Find a missing constructor to use as witness.
                            let missing_ctor = all_ctors
                                .iter()
                                .find(|c| !head_ctors.contains(&c.name))
                                .unwrap();
                            let arity = missing_ctor.params.len();
                            let mut witness = vec![Pattern::Constructor {
                                name: missing_ctor.name.clone(),
                                args: vec![Pattern::Wildcard; arity],
                            }];
                            witness.extend(sub_witness);
                            Useful::Yes(witness)
                        }
                    }
                }
            } else {
                // Not an ADT type (e.g. Int, String, etc.) — use default matrix.
                // These types have infinitely many values, so we can't be
                // complete. The wildcard is always potentially useful unless
                // there's already a wildcard/variable row in the matrix.
                let default_mat = default_matrix(matrix);

                let mut default_test: PatternRow = Vec::new();
                default_test.extend_from_slice(&test_row[col + 1..]);

                let default_col_types = col_types[col + 1..].to_vec();

                match is_useful(env, &default_mat, &default_test, &default_col_types) {
                    Useful::No => Useful::No,
                    Useful::Yes(sub_witness) => {
                        let mut witness = vec![Pattern::Wildcard];
                        witness.extend(sub_witness);
                        Useful::Yes(witness)
                    }
                }
            }
        }

        // ── Literal pattern ────────────────────────────────────────────
        Pattern::Literal(_) => {
            // Literals have infinitely many values — we can't enumerate them.
            // Treat like a constructor with no sub-patterns but that doesn't
            // complete the type. Use the default matrix approach.
            // The literal itself is "useful" if it's not already present, but
            // for exhaustiveness checking of the overall match, we defer to
            // default matrix logic — literals never make a match exhaustive
            // on their own (you always need a wildcard/variable catch-all).
            let default_mat = default_matrix(matrix);

            let mut default_test: PatternRow = Vec::new();
            default_test.extend_from_slice(&test_row[col + 1..]);

            let default_col_types = col_types[col + 1..].to_vec();

            match is_useful(env, &default_mat, &default_test, &default_col_types) {
                Useful::No => Useful::No,
                Useful::Yes(sub_witness) => {
                    let mut witness = vec![Pattern::Wildcard];
                    witness.extend(sub_witness);
                    Useful::Yes(witness)
                }
            }
        }
    }
}

/// Specialize the matrix by constructor `ctor_name` with `arity` parameters.
///
/// For each row:
/// - If the first pattern is `Constructor { name, args }` with `name == ctor_name`,
///   replace the first column with the constructor's args (expanding to `arity` columns).
/// - If the first pattern is `Wildcard` or `Var`, replace it with `arity` wildcards.
/// - If the first pattern is a different constructor, drop the row.
fn specialize_matrix(matrix: &PatternMatrix, ctor_name: &str, arity: usize) -> PatternMatrix {
    let mut result = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0] {
            Pattern::Constructor { name, args } if name == ctor_name => {
                let mut new_row: PatternRow = args.clone();
                new_row.extend_from_slice(&row[1..]);
                result.push(new_row);
            }
            Pattern::Wildcard | Pattern::Var(_) => {
                let mut new_row: PatternRow = vec![Pattern::Wildcard; arity];
                new_row.extend_from_slice(&row[1..]);
                result.push(new_row);
            }
            _ => {
                // Different constructor or literal — skip this row.
            }
        }
    }
    result
}

/// Build column types for a specialized matrix.
///
/// The first column (at `col_types[0]`) is replaced by the parameter types
/// of the constructor. The remaining columns keep their original types.
fn specialize_col_types(
    env: &TypeEnv,
    col_types: &[Type],
    ctor_name: &str,
    arity: usize,
) -> Vec<Type> {
    let mut result = Vec::with_capacity(arity + col_types.len() - 1);

    // Look up the constructor to get its parameter types.
    if let Some((_dt, ctor)) = lookup_constructor(env, ctor_name) {
        // The constructor params may contain type variables from the data type
        // definition. We need to instantiate them with the actual type arguments
        // from the scrutinee type.
        let instantiated_params = instantiate_ctor_params(env, col_types, &ctor);
        result.extend(instantiated_params);
    } else {
        // Fallback: if we can't find the constructor, use wildcards.
        for _ in 0..arity {
            result.push(Type::Var("_unknown".into()));
        }
    }

    result.extend_from_slice(&col_types[1..]);
    result
}

/// Instantiate a constructor's parameter types using the actual type arguments
/// from the scrutinee type.
///
/// For example, if we have `(data (Tree a) Leaf (Node a (Tree a)))` and the
/// scrutinee type is `(Tree Int)`, then for constructor `Node` with params
/// `[a, (Tree a)]`, we return `[Int, (Tree Int)]`.
fn instantiate_ctor_params(env: &TypeEnv, col_types: &[Type], ctor: &Constructor) -> Vec<Type> {
    let scrutinee_type = &col_types[0];

    // Get the data type to find its type parameters.
    let dt = match get_data_type_from_type(env, scrutinee_type) {
        Some(dt) => dt,
        None => return ctor.params.clone(),
    };

    if dt.type_params.is_empty() {
        return ctor.params.clone();
    }

    // Extract the actual type arguments from the scrutinee type.
    let actual_args = match scrutinee_type {
        Type::App { args, .. } => args.clone(),
        _ => return ctor.params.clone(),
    };

    if actual_args.len() != dt.type_params.len() {
        return ctor.params.clone();
    }

    // Build a substitution from type params to actual types.
    let subst: std::collections::HashMap<String, Type> = dt
        .type_params
        .iter()
        .zip(actual_args.iter())
        .map(|(param, actual)| (param.clone(), actual.clone()))
        .collect();

    // Apply the substitution to each constructor param.
    ctor.params
        .iter()
        .map(|p| apply_type_subst(&subst, p))
        .collect()
}

/// Apply a type variable substitution to a type.
fn apply_type_subst(subst: &std::collections::HashMap<String, Type>, ty: &Type) -> Type {
    match ty {
        Type::Var(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Const(_) => ty.clone(),
        Type::App { constructor, args } => Type::App {
            constructor: Box::new(apply_type_subst(subst, constructor)),
            args: args.iter().map(|a| apply_type_subst(subst, a)).collect(),
        },
        Type::Arrow {
            params,
            rest,
            effects,
            result,
        } => Type::Arrow {
            params: params.iter().map(|p| apply_type_subst(subst, p)).collect(),
            rest: rest.as_ref().map(|r| Box::new(apply_type_subst(subst, r))),
            effects: effects.clone(),
            result: Box::new(apply_type_subst(subst, result)),
        },
        Type::Rec { var, body } => {
            // Don't substitute the bound variable.
            if subst.contains_key(var) {
                ty.clone()
            } else {
                Type::Rec {
                    var: var.clone(),
                    body: Box::new(apply_type_subst(subst, body)),
                }
            }
        }
        Type::EffectRowRef(_) => ty.clone(),
    }
}

/// Build the default matrix: rows whose first pattern is a wildcard/variable,
/// with the first column removed.
fn default_matrix(matrix: &PatternMatrix) -> PatternMatrix {
    let mut result = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0] {
            Pattern::Wildcard | Pattern::Var(_) => {
                result.push(row[1..].to_vec());
            }
            _ => {
                // Constructor or literal — skip.
            }
        }
    }
    result
}

/// Collect the set of constructor names that appear at the head of column 0.
fn collect_head_constructors(matrix: &PatternMatrix) -> std::collections::HashSet<String> {
    let mut ctors = std::collections::HashSet::new();
    for row in matrix {
        if let Some(Pattern::Constructor { name, .. }) = row.first() {
            ctors.insert(name.clone());
        }
    }
    ctors
}

/// Get all constructors of an ADT type. Returns `None` if the type is not an ADT.
fn all_constructors_of_type(env: &TypeEnv, ty: &Type) -> Option<Vec<Constructor>> {
    get_data_type_from_type(env, ty).map(|dt| dt.constructors)
}

/// Resolve a `Type` to its `DataType` definition, if it's an ADT.
fn get_data_type_from_type(env: &TypeEnv, t: &Type) -> Option<DataType> {
    match t {
        Type::Const(name) => lookup_data_type(env, name),
        Type::App { constructor, .. } => {
            if let Type::Const(name) = constructor.as_ref() {
                lookup_data_type(env, name)
            } else {
                None
            }
        }
        _ => None,
    }
}
