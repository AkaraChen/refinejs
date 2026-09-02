use crate::syntax::*;
use std::collections::HashSet;

pub fn check_annotations(annotations: &[Annotation]) -> Vec<RtError> {
    let mut errors = Vec::new();

    let mut params_by_function: std::collections::HashMap<String, HashSet<String>> = std::collections::HashMap::new();
    for a in annotations {
        if let AnnotationTarget::Param { function_name, param_name, .. } = &a.target {
            params_by_function
                .entry(function_name.clone())
                .or_default()
                .insert(param_name.clone());
        }
    }

    for a in annotations {
        let mut allowed = HashSet::new();
        let is_return = matches!(a.target, AnnotationTarget::Return { .. });

        match &a.target {
            AnnotationTarget::Param { function_name, .. } => {
                if let Some(set) = params_by_function.get(function_name) {
                    for p in set {
                        allowed.insert(p.clone());
                    }
                }
                allowed.insert("length".to_string());
            }
            AnnotationTarget::Return { function_name } => {
                if let Some(set) = params_by_function.get(function_name) {
                    for p in set {
                        allowed.insert(p.clone());
                    }
                }
                allowed.insert("$".to_string());
                allowed.insert("length".to_string());
            }
            AnnotationTarget::Variable { name } => {
                allowed.insert(name.clone());
                allowed.insert("length".to_string());
            }
        }

        if let Some(err) = check_type(&a.ty, a.loc.clone()) {
            errors.push(err);
        }

        if let Some(pred) = &a.ty.predicate {
            if let Some(err) = check_predicate(pred, &allowed, is_return, a.loc.clone()) {
                errors.push(err);
            }
        }
    }

    errors
}

fn check_type(ty: &RefinementType, loc: SourceLocation) -> Option<RtError> {
    match &ty.base {
        BaseType::Array(el) => check_type(&RefinementType { base: (**el).clone(), predicate: None }, loc),
        BaseType::Object(fields) => {
            for (_, t) in fields {
                if let Some(err) = check_type(&RefinementType { base: t.clone(), predicate: None }, loc.clone()) {
                    return Some(err);
                }
            }
            None
        }
        BaseType::Function(params, ret) => {
            for p in params {
                if let Some(err) = check_type(&p.ty, loc.clone()) {
                    return Some(err);
                }
            }
            check_type(ret, loc)
        }
        _ => None,
    }
}

fn check_predicate(pred: &PredicateExpr, allowed: &HashSet<String>, is_return: bool, loc: SourceLocation) -> Option<RtError> {
    match pred {
        PredicateExpr::Literal(_) => None,
        PredicateExpr::Identifier(name) => {
            if name == "$" && !is_return {
                return Some(RtError {
                    message: "Return marker '$' is only allowed in return refinement predicates".into(),
                    loc: Some(loc),
                });
            }
            if !allowed.contains(name) {
                return Some(RtError {
                    message: format!("Unknown identifier '{}' in refinement predicate", name),
                    loc: Some(loc),
                });
            }
            None
        }
        PredicateExpr::Member(obj, _) => {
            if !allowed.contains(obj) {
                return Some(RtError {
                    message: format!("Unknown identifier '{}' in refinement predicate", obj),
                    loc: Some(loc),
                });
            }
            None
        }
        PredicateExpr::Return => {
            if !is_return {
                return Some(RtError {
                    message: "Return marker '$' is only allowed in return refinement predicates".into(),
                    loc: Some(loc),
                });
            }
            None
        }
        PredicateExpr::Not(expr) => check_predicate(expr, allowed, is_return, loc),
        PredicateExpr::Logical(_, left, right) | PredicateExpr::Binary(_, left, right) => {
            if let Some(err) = check_predicate(left, allowed, is_return, loc.clone()) {
                return Some(err);
            }
            check_predicate(right, allowed, is_return, loc)
        }
    }
}
