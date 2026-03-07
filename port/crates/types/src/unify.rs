use crate::ast::*;
use crate::env::*;
use crate::errors::TypeError;
use std::collections::HashSet;

/// Maximum recursion depth for unify/match to prevent runaway expansion
/// from adversarial type inputs (e.g. deeply nested mu-types).
const UNIFY_MAX_DEPTH: usize = 64;

// Occurs check: does variable v occur in type t?
fn occurs(v: &str, t: &Type) -> bool {
    free_type_vars(t).contains(v)
}

/// Count the number of nodes in a type tree. Returns early once count exceeds
/// the threshold to avoid walking enormous types.
fn type_node_count(t: &Type) -> usize {
    type_node_count_bounded(t, 65)
}

fn type_node_count_bounded(t: &Type, limit: usize) -> usize {
    if limit == 0 {
        return 1;
    }
    match t {
        Type::Var(_) | Type::Const(_) => 1,
        Type::App { constructor, args } => {
            let mut count = 1 + type_node_count_bounded(constructor, limit - 1);
            for a in args {
                if count >= limit {
                    return count;
                }
                count += type_node_count_bounded(a, limit - count);
            }
            count
        }
        Type::Arrow {
            params,
            rest,
            result,
            ..
        } => {
            let mut count = 1;
            for p in params {
                if count >= limit {
                    return count;
                }
                count += type_node_count_bounded(p, limit - count);
            }
            if let Some(r) = rest
                && count < limit
            {
                count += type_node_count_bounded(r, limit - count);
            }
            if count < limit {
                count += type_node_count_bounded(result, limit - count);
            }
            count
        }
        Type::Rec { body, .. } => 1 + type_node_count_bounded(body, limit - 1),
        Type::EffectRowRef(_) => 1,
    }
}

// Unroll a mu-type: (μ a. T) => T[μ a. T / a]
pub fn unroll_mu(t: &Type) -> Type {
    if let Type::Rec { var, body } = t {
        let subst = single_subst(var, t.clone());
        apply_subst(&subst, body)
    } else {
        t.clone()
    }
}

// Internal unify with cycle detection
fn unify_internal(
    t1: &Type,
    t2: &Type,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Result<Substitution, TypeError> {
    // Depth limit to prevent exponential blowup on adversarial types
    if depth >= UNIFY_MAX_DEPTH {
        return Err(TypeError::UnificationError {
            t1: t1.clone(),
            t2: t2.clone(),
            context: Some("recursion limit exceeded during unification".to_string()),
            span: None,
        });
    }

    // Check for cycles — only use format_type when types are small enough
    // to avoid O(n) formatting on exponentially-grown types.
    if type_node_count(t1) + type_node_count(t2) <= 64 {
        let key = format!("{}~{}", format_type(t1), format_type(t2));
        let key_reverse = format!("{}~{}", format_type(t2), format_type(t1));
        if seen.contains(&key) || seen.contains(&key_reverse) {
            return Ok(empty_subst());
        }
        seen.insert(key);
    }

    // Handle mu-types by unrolling
    if let Type::Rec { .. } = t1 {
        return unify_internal(&unroll_mu(t1), t2, seen, depth + 1);
    }
    if let Type::Rec { .. } = t2 {
        return unify_internal(t1, &unroll_mu(t2), seen, depth + 1);
    }

    // Type variable cases
    if let Type::Var(name1) = t1 {
        if let Type::Var(name2) = t2
            && name1 == name2
        {
            return Ok(empty_subst());
        }
        if occurs(name1, t2) {
            // Create mu-type instead of failing on occurs check
            let mu = type_rec(name1, t2.clone());
            return Ok(single_subst(name1, mu));
        }
        // Propagate level: if t2 is a Var, lower its level to min(level1, level2)
        if let Type::Var(name2) = t2 {
            lower_var_level(name2, get_var_level(name1));
        }
        return Ok(single_subst(name1, t2.clone()));
    }

    if let Type::Var(name2) = t2 {
        if occurs(name2, t1) {
            let mu = type_rec(name2, t1.clone());
            return Ok(single_subst(name2, mu));
        }
        // Propagate level: if t1 is a Var, lower its level to min(level1, level2)
        if let Type::Var(name1) = t1 {
            lower_var_level(name1, get_var_level(name2));
        }
        return Ok(single_subst(name2, t1.clone()));
    }

    // Type constant case
    if let (Type::Const(n1), Type::Const(n2)) = (t1, t2) {
        if n1 == n2 {
            return Ok(empty_subst());
        }
        return Err(TypeError::UnificationError {
            t1: t1.clone(),
            t2: t2.clone(),
            context: None,
            span: None,
        });
    }

    // Type application case
    if let (
        Type::App {
            constructor: c1,
            args: a1,
        },
        Type::App {
            constructor: c2,
            args: a2,
        },
    ) = (t1, t2)
    {
        if a1.len() != a2.len() {
            return Err(TypeError::UnificationError {
                t1: t1.clone(),
                t2: t2.clone(),
                context: None,
                span: None,
            });
        }

        let s1 = unify_internal(c1, c2, seen, depth + 1)?;
        let mut subst = s1;
        for i in 0..a1.len() {
            let arg1 = apply_subst(&subst, &a1[i]);
            let arg2 = apply_subst(&subst, &a2[i]);
            let s = unify_internal(&arg1, &arg2, seen, depth + 1)?;
            subst = compose_subst(&s, &subst);
        }
        return Ok(subst);
    }

    // EffectRowRef: unify effect rows stored as types
    if let (Type::EffectRowRef(r1), Type::EffectRowRef(r2)) = (t1, t2) {
        return unify_effect_rows_into_subst(r1, r2);
    }
    // EffectRowRef vs Var: bind the var to the effect row ref
    if let (Type::EffectRowRef(row), Type::Var(name)) = (t1, t2) {
        // Propagate level to any row var in the effect row
        if let EffectRow::Open { row_var, .. } = row {
            lower_var_level(row_var, get_var_level(name));
        }
        return Ok(single_subst(name, t1.clone()));
    }
    if let (Type::Var(name), Type::EffectRowRef(row)) = (t1, t2) {
        // Propagate level to any row var in the effect row
        if let EffectRow::Open { row_var, .. } = row {
            lower_var_level(row_var, get_var_level(name));
        }
        return Ok(single_subst(name, t2.clone()));
    }

    // Arrow type case
    if let (
        Type::Arrow {
            params: p1,
            rest: rest1,
            effects: eff1,
            result: r1,
        },
        Type::Arrow {
            params: p2,
            rest: rest2,
            effects: eff2,
            result: r2,
        },
    ) = (t1, t2)
    {
        let mut subst = empty_subst();

        match (rest1, rest2) {
            // Neither is variadic: strict arity match (original behavior)
            (None, None) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::ArityMismatch {
                        expected: p1.len(),
                        got: p2.len(),
                        context: "function type".to_string(),
                        span: None,
                    });
                }
                for i in 0..p1.len() {
                    let pa = apply_subst(&subst, &p1[i]);
                    let pb = apply_subst(&subst, &p2[i]);
                    let s = unify_internal(&pa, &pb, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
            }
            // Both variadic: unify required params pairwise, then unify rest types
            (Some(rt1), Some(rt2)) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::ArityMismatch {
                        expected: p1.len(),
                        got: p2.len(),
                        context: "function type".to_string(),
                        span: None,
                    });
                }
                for i in 0..p1.len() {
                    let pa = apply_subst(&subst, &p1[i]);
                    let pb = apply_subst(&subst, &p2[i]);
                    let s = unify_internal(&pa, &pb, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
                let ra = apply_subst(&subst, rt1);
                let rb = apply_subst(&subst, rt2);
                let s = unify_internal(&ra, &rb, seen, depth + 1)?;
                subst = compose_subst(&s, &subst);
            }
            // t1 is variadic, t2 is not: t2 must have at least p1.len() params,
            // extra t2 params unify with rest type
            (Some(rt1), None) => {
                if p2.len() < p1.len() {
                    return Err(TypeError::ArityMismatch {
                        expected: p1.len(),
                        got: p2.len(),
                        context: "variadic function type (too few arguments)".to_string(),
                        span: None,
                    });
                }
                // Unify required params
                for i in 0..p1.len() {
                    let pa = apply_subst(&subst, &p1[i]);
                    let pb = apply_subst(&subst, &p2[i]);
                    let s = unify_internal(&pa, &pb, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
                // Extra params in t2 unify with rest type
                for extra_param in &p2[p1.len()..] {
                    let rest_ty = apply_subst(&subst, rt1);
                    let pb = apply_subst(&subst, extra_param);
                    let s = unify_internal(&rest_ty, &pb, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
            }
            // t2 is variadic, t1 is not: t1 must have at least p2.len() params,
            // extra t1 params unify with rest type
            (None, Some(rt2)) => {
                if p1.len() < p2.len() {
                    return Err(TypeError::ArityMismatch {
                        expected: p2.len(),
                        got: p1.len(),
                        context: "variadic function type (too few arguments)".to_string(),
                        span: None,
                    });
                }
                // Unify required params
                for i in 0..p2.len() {
                    let pa = apply_subst(&subst, &p1[i]);
                    let pb = apply_subst(&subst, &p2[i]);
                    let s = unify_internal(&pa, &pb, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
                // Extra params in t1 unify with rest type
                for extra_param in &p1[p2.len()..] {
                    let pa = apply_subst(&subst, extra_param);
                    let rest_ty = apply_subst(&subst, rt2);
                    let s = unify_internal(&pa, &rest_ty, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
            }
        }

        // Unify result types
        let ra = apply_subst(&subst, r1);
        let rb = apply_subst(&subst, r2);
        let s = unify_internal(&ra, &rb, seen, depth + 1)?;
        subst = compose_subst(&s, &subst);

        // Unify effect rows — None is treated as pure (Closed([]))
        {
            let default_pure = effect_row_empty();
            let e1 = eff1.as_ref().unwrap_or(&default_pure);
            let e2 = eff2.as_ref().unwrap_or(&default_pure);
            let e1_applied = apply_effect_row_subst_via_type_subst(&subst, e1);
            let e2_applied = apply_effect_row_subst_via_type_subst(&subst, e2);
            let s = unify_effect_rows_into_subst(&e1_applied, &e2_applied)?;
            subst = compose_subst(&s, &subst);
        }

        return Ok(subst);
    }

    // Mismatched type kinds
    Err(TypeError::UnificationError {
        t1: t1.clone(),
        t2: t2.clone(),
        context: None,
        span: None,
    })
}

// Unify two types
pub fn unify(t1: &Type, t2: &Type) -> Result<Substitution, TypeError> {
    unify_internal(t1, t2, &mut HashSet::new(), 0)
}

// Unify with context for better error messages
pub fn unify_with_context(t1: &Type, t2: &Type, context: &str) -> Result<Substitution, TypeError> {
    match unify(t1, t2) {
        Err(TypeError::UnificationError {
            t1: e_t1,
            t2: e_t2,
            span,
            ..
        }) => Err(TypeError::UnificationError {
            t1: e_t1,
            t2: e_t2,
            context: Some(context.to_string()),
            span,
        }),
        other => other,
    }
}

// Unify a list of type pairs
pub fn unify_many(pairs: &[(Type, Type)]) -> Result<Substitution, TypeError> {
    let mut subst = empty_subst();
    for (t1, t2) in pairs {
        let s1 = apply_subst(&subst, t1);
        let s2 = apply_subst(&subst, t2);
        let s = unify(&s1, &s2)?;
        subst = compose_subst(&s, &subst);
    }
    Ok(subst)
}

// Match: like unify but only substitutes variables in the first type
pub fn match_types(pattern: &Type, target: &Type) -> Result<Substitution, TypeError> {
    match_types_internal(pattern, target, &mut HashSet::new(), 0)
}

// Internal match with cycle detection and depth limit
fn match_types_internal(
    pattern: &Type,
    target: &Type,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Result<Substitution, TypeError> {
    // Depth limit to prevent exponential blowup
    if depth >= UNIFY_MAX_DEPTH {
        return Err(TypeError::UnificationError {
            t1: pattern.clone(),
            t2: target.clone(),
            context: Some("recursion limit exceeded during type matching".to_string()),
            span: None,
        });
    }

    // Check for cycles — only when types are small enough to format cheaply
    if type_node_count(pattern) + type_node_count(target) <= 64 {
        let key = format!("{}~{}", format_type(pattern), format_type(target));
        if seen.contains(&key) {
            return Ok(empty_subst());
        }
        seen.insert(key);
    }

    // Handle mu-types by unrolling
    if let Type::Rec { .. } = pattern {
        return match_types_internal(&unroll_mu(pattern), target, seen, depth + 1);
    }
    if let Type::Rec { .. } = target {
        return match_types_internal(pattern, &unroll_mu(target), seen, depth + 1);
    }

    if let Type::Var(name) = pattern {
        return Ok(single_subst(name, target.clone()));
    }

    if let Type::Var(_) = target {
        return Err(TypeError::UnificationError {
            t1: pattern.clone(),
            t2: target.clone(),
            context: None,
            span: None,
        });
    }

    if let (Type::Const(n1), Type::Const(n2)) = (pattern, target) {
        if n1 == n2 {
            return Ok(empty_subst());
        }
        return Err(TypeError::UnificationError {
            t1: pattern.clone(),
            t2: target.clone(),
            context: None,
            span: None,
        });
    }

    if let (
        Type::App {
            constructor: c1,
            args: a1,
        },
        Type::App {
            constructor: c2,
            args: a2,
        },
    ) = (pattern, target)
    {
        if a1.len() != a2.len() {
            return Err(TypeError::UnificationError {
                t1: pattern.clone(),
                t2: target.clone(),
                context: None,
                span: None,
            });
        }

        let mut subst = match_types_internal(c1, c2, seen, depth + 1)?;
        for i in 0..a1.len() {
            let p = apply_subst(&subst, &a1[i]);
            let t = &a2[i];
            let s = match_types_internal(&p, t, seen, depth + 1)?;
            // Check consistency
            for (v, ty) in &s {
                if let Some(existing) = subst.get(v) {
                    let u = unify(existing, ty)?;
                    subst = compose_subst(&u, &subst);
                } else {
                    subst.insert(v.clone(), ty.clone());
                }
            }
        }
        return Ok(subst);
    }

    if let (
        Type::Arrow {
            params: p1,
            rest: rest1,
            result: r1,
            ..
        },
        Type::Arrow {
            params: p2,
            rest: rest2,
            result: r2,
            ..
        },
    ) = (pattern, target)
    {
        let mut subst = empty_subst();

        match (rest1, rest2) {
            (None, None) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::UnificationError {
                        t1: pattern.clone(),
                        t2: target.clone(),
                        context: None,
                        span: None,
                    });
                }
                for i in 0..p1.len() {
                    let p = apply_subst(&subst, &p1[i]);
                    let t = &p2[i];
                    let s = match_types_internal(&p, t, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
            }
            (Some(rt1), None) => {
                if p2.len() < p1.len() {
                    return Err(TypeError::UnificationError {
                        t1: pattern.clone(),
                        t2: target.clone(),
                        context: None,
                        span: None,
                    });
                }
                for i in 0..p1.len() {
                    let p = apply_subst(&subst, &p1[i]);
                    let t = &p2[i];
                    let s = match_types_internal(&p, t, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
                for extra_param in &p2[p1.len()..] {
                    let rest_ty = apply_subst(&subst, rt1);
                    let s = match_types_internal(&rest_ty, extra_param, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
            }
            (None, Some(rt2)) => {
                if p1.len() < p2.len() {
                    return Err(TypeError::UnificationError {
                        t1: pattern.clone(),
                        t2: target.clone(),
                        context: None,
                        span: None,
                    });
                }
                for i in 0..p2.len() {
                    let p = apply_subst(&subst, &p1[i]);
                    let t = &p2[i];
                    let s = match_types_internal(&p, t, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
                for extra_param in &p1[p2.len()..] {
                    let p = apply_subst(&subst, extra_param);
                    let s = match_types_internal(&p, rt2, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
            }
            (Some(rt1), Some(rt2)) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::UnificationError {
                        t1: pattern.clone(),
                        t2: target.clone(),
                        context: None,
                        span: None,
                    });
                }
                for i in 0..p1.len() {
                    let p = apply_subst(&subst, &p1[i]);
                    let t = &p2[i];
                    let s = match_types_internal(&p, t, seen, depth + 1)?;
                    subst = compose_subst(&s, &subst);
                }
                let rp = apply_subst(&subst, rt1);
                let s = match_types_internal(&rp, rt2, seen, depth + 1)?;
                subst = compose_subst(&s, &subst);
            }
        }

        let pr = apply_subst(&subst, r1);
        let s = match_types_internal(&pr, r2, seen, depth + 1)?;
        return Ok(compose_subst(&s, &subst));
    }

    Err(TypeError::UnificationError {
        t1: pattern.clone(),
        t2: target.clone(),
        context: None,
        span: None,
    })
}

// ===== Effect Row Unification =====

/// Unify two effect rows, producing a regular Substitution.
/// Effect row variable bindings are stored as ε_var → Type::EffectRowRef(row).
pub fn unify_effect_rows_into_subst(
    row1: &EffectRow,
    row2: &EffectRow,
) -> Result<Substitution, TypeError> {
    match (row1, row2) {
        (EffectRow::Closed(e1), EffectRow::Closed(e2)) => {
            let set1: HashSet<_> = e1.iter().collect();
            let set2: HashSet<_> = e2.iter().collect();
            if set1 == set2 {
                Ok(empty_subst())
            } else {
                Err(TypeError::UnificationError {
                    t1: Type::Const(format_effect_row(row1)),
                    t2: Type::Const(format_effect_row(row2)),
                    context: Some("effect rows".to_string()),
                    span: None,
                })
            }
        }
        (EffectRow::Open { effects, row_var }, EffectRow::Closed(e2)) => {
            for e in effects {
                if !e2.contains(e) {
                    return Err(TypeError::UnificationError {
                        t1: Type::Const(format_effect_row(row1)),
                        t2: Type::Const(format_effect_row(row2)),
                        context: Some("effect rows".to_string()),
                        span: None,
                    });
                }
            }
            let remaining: Vec<String> = e2
                .iter()
                .filter(|e| !effects.contains(e))
                .cloned()
                .collect();
            // row_var is eliminated (bound to a closed row), no level propagation needed
            Ok(single_subst(
                row_var,
                Type::EffectRowRef(effect_row_closed(remaining)),
            ))
        }
        (EffectRow::Closed(e1), EffectRow::Open { effects, row_var }) => {
            for e in effects {
                if !e1.contains(e) {
                    return Err(TypeError::UnificationError {
                        t1: Type::Const(format_effect_row(row1)),
                        t2: Type::Const(format_effect_row(row2)),
                        context: Some("effect rows".to_string()),
                        span: None,
                    });
                }
            }
            let remaining: Vec<String> = e1
                .iter()
                .filter(|e| !effects.contains(e))
                .cloned()
                .collect();
            // row_var is eliminated (bound to a closed row), no level propagation needed
            Ok(single_subst(
                row_var,
                Type::EffectRowRef(effect_row_closed(remaining)),
            ))
        }
        (
            EffectRow::Open {
                effects: e1,
                row_var: rv1,
            },
            EffectRow::Open {
                effects: e2,
                row_var: rv2,
            },
        ) => {
            if rv1 == rv2 {
                // Same row var — just check that explicitly listed effects are compatible
                let set1: HashSet<_> = e1.iter().collect();
                let set2: HashSet<_> = e2.iter().collect();
                if set1 == set2 {
                    return Ok(empty_subst());
                }
                // Different explicit effects but same row var — unify via fresh var
                let fresh_rv = fresh_effect_row_var();
                let only_in_2: Vec<String> =
                    e2.iter().filter(|e| !e1.contains(e)).cloned().collect();
                let only_in_1: Vec<String> =
                    e1.iter().filter(|e| !e2.contains(e)).cloned().collect();
                if only_in_1.is_empty() && only_in_2.is_empty() {
                    return Ok(empty_subst());
                }
                // Can't have different effects with same row var — this shouldn't happen normally
                // Fall through to open-open unification with fresh var
                let mut combined: Vec<String> = e1.clone();
                for e in e2 {
                    if !combined.contains(e) {
                        combined.push(e.clone());
                    }
                }
                return Ok(single_subst(
                    rv1,
                    Type::EffectRowRef(effect_row_open(combined, fresh_rv)),
                ));
            }
            // Different row vars: rv1 gets bound to {effects_only_in_2 | rv2}
            // Propagate level from rv1 to rv2
            lower_var_level(rv2, get_var_level(rv1));
            let only_in_2: Vec<String> = e2.iter().filter(|e| !e1.contains(e)).cloned().collect();
            Ok(single_subst(
                rv1,
                Type::EffectRowRef(effect_row_open(only_in_2, rv2.clone())),
            ))
        }
    }
}

// Combine two effect rows (union)
pub fn combine_effect_rows(r1: &EffectRow, r2: &EffectRow) -> EffectRow {
    let mut combined: Vec<String> = r1.effects().to_vec();
    for e in r2.effects() {
        if !combined.contains(e) {
            combined.push(e.clone());
        }
    }

    match (r1, r2) {
        (EffectRow::Open { row_var, .. }, _) => effect_row_open(combined, row_var.clone()),
        (_, EffectRow::Open { row_var, .. }) => effect_row_open(combined, row_var.clone()),
        _ => effect_row_closed(combined),
    }
}
