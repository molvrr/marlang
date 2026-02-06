use std::collections::HashSet;

use crate::ast::*;
use crate::env::*;
use crate::errors::TypeError;

// Occurs check: does variable v occur in type t?
fn occurs(v: &str, t: &Type) -> bool {
    free_type_vars(t).contains(v)
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
) -> Result<Substitution, TypeError> {
    // Check for cycles
    let key = format!("{}~{}", format_type(t1), format_type(t2));
    let key_reverse = format!("{}~{}", format_type(t2), format_type(t1));
    if seen.contains(&key) || seen.contains(&key_reverse) {
        return Ok(empty_subst());
    }
    seen.insert(key);

    // Handle mu-types by unrolling
    if let Type::Rec { .. } = t1 {
        return unify_internal(&unroll_mu(t1), t2, seen);
    }
    if let Type::Rec { .. } = t2 {
        return unify_internal(t1, &unroll_mu(t2), seen);
    }

    // Type variable cases
    if let Type::Var(name1) = t1 {
        if let Type::Var(name2) = t2 {
            if name1 == name2 {
                return Ok(empty_subst());
            }
        }
        if occurs(name1, t2) {
            // Create mu-type instead of failing on occurs check
            let mu = type_rec(name1, t2.clone());
            return Ok(single_subst(name1, mu));
        }
        return Ok(single_subst(name1, t2.clone()));
    }

    if let Type::Var(name2) = t2 {
        if occurs(name2, t1) {
            let mu = type_rec(name2, t1.clone());
            return Ok(single_subst(name2, mu));
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

        let s1 = unify_internal(c1, c2, seen)?;
        let mut subst = s1;
        for i in 0..a1.len() {
            let arg1 = apply_subst(&subst, &a1[i]);
            let arg2 = apply_subst(&subst, &a2[i]);
            let s = unify_internal(&arg1, &arg2, seen)?;
            subst = compose_subst(&s, &subst);
        }
        return Ok(subst);
    }

    // Arrow type case
    if let (
        Type::Arrow {
            params: p1,
            result: r1,
            ..
        },
        Type::Arrow {
            params: p2,
            result: r2,
            ..
        },
    ) = (t1, t2)
    {
        if p1.len() != p2.len() {
            return Err(TypeError::ArityMismatch {
                expected: p1.len(),
                got: p2.len(),
                context: "function type".to_string(),
                span: None,
            });
        }

        let mut subst = empty_subst();

        // Unify parameter types
        for i in 0..p1.len() {
            let pa = apply_subst(&subst, &p1[i]);
            let pb = apply_subst(&subst, &p2[i]);
            let s = unify_internal(&pa, &pb, seen)?;
            subst = compose_subst(&s, &subst);
        }

        // Unify result types
        let ra = apply_subst(&subst, r1);
        let rb = apply_subst(&subst, r2);
        let s = unify_internal(&ra, &rb, seen)?;
        return Ok(compose_subst(&s, &subst));
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
    unify_internal(t1, t2, &mut HashSet::new())
}

// Unify with context for better error messages
pub fn unify_with_context(
    t1: &Type,
    t2: &Type,
    context: &str,
) -> Result<Substitution, TypeError> {
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
    // Handle mu-types by unrolling
    if let Type::Rec { .. } = pattern {
        return match_types(&unroll_mu(pattern), target);
    }
    if let Type::Rec { .. } = target {
        return match_types(pattern, &unroll_mu(target));
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

        let mut subst = match_types(c1, c2)?;
        for i in 0..a1.len() {
            let p = apply_subst(&subst, &a1[i]);
            let t = &a2[i];
            let s = match_types(&p, t)?;
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
            result: r1,
            ..
        },
        Type::Arrow {
            params: p2,
            result: r2,
            ..
        },
    ) = (pattern, target)
    {
        if p1.len() != p2.len() {
            return Err(TypeError::UnificationError {
                t1: pattern.clone(),
                t2: target.clone(),
                context: None,
                span: None,
            });
        }

        let mut subst = empty_subst();
        for i in 0..p1.len() {
            let p = apply_subst(&subst, &p1[i]);
            let t = &p2[i];
            let s = match_types(&p, t)?;
            subst = compose_subst(&s, &subst);
        }

        let pr = apply_subst(&subst, r1);
        let s = match_types(&pr, r2)?;
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

pub fn unify_effect_rows(
    r1: Option<&EffectRow>,
    r2: Option<&EffectRow>,
) -> Result<EffectRowSubstitution, TypeError> {
    let default_closed = effect_row_closed(vec![]);
    let row1 = r1.unwrap_or(&default_closed);
    let row2 = r2.unwrap_or(&default_closed);

    match (row1, row2) {
        (EffectRow::Closed(e1), EffectRow::Closed(e2)) => {
            let set1: HashSet<_> = e1.iter().collect();
            let set2: HashSet<_> = e2.iter().collect();
            if set1 == set2 {
                Ok(EffectRowSubstitution::new())
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
            let remaining: Vec<String> =
                e2.iter().filter(|e| !effects.contains(e)).cloned().collect();
            let mut subst = EffectRowSubstitution::new();
            subst.insert(row_var.clone(), effect_row_closed(remaining));
            Ok(subst)
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
            let remaining: Vec<String> =
                e1.iter().filter(|e| !effects.contains(e)).cloned().collect();
            let mut subst = EffectRowSubstitution::new();
            subst.insert(row_var.clone(), effect_row_closed(remaining));
            Ok(subst)
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
                return Ok(EffectRowSubstitution::new());
            }
            let only_in2: Vec<String> = e2.iter().filter(|e| !e1.contains(e)).cloned().collect();
            let mut subst = EffectRowSubstitution::new();
            subst.insert(
                rv1.clone(),
                effect_row_open(only_in2, rv2.clone()),
            );
            Ok(subst)
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
