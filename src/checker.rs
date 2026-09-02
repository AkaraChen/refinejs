use crate::syntax::*;
use std::collections::HashSet;
use std::path::Path;

pub fn check_source(source: &str, file_name: &str, annotations: &[Annotation]) -> Vec<RtError> {
    check_source_with_environment(
        source,
        file_name,
        annotations,
        crate::prelude::Environment::Auto,
    )
}

pub fn check_source_with_environment(
    source: &str,
    file_name: &str,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
) -> Vec<RtError> {
    let mut errors = check_annotations(annotations);
    if errors.is_empty() {
        errors.extend(crate::verifier::verify_source_with_environment(
            source,
            file_name,
            annotations,
            environment,
        ));
    }
    errors
}

pub fn check_source_with_environment_and_compiler(
    source: &str,
    file_name: &str,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    provider: &dyn crate::type_provider::CompilerTypeProvider,
    config_path: &Path,
    source_path: &Path,
) -> Result<Vec<RtError>, crate::type_provider::CompilerTypeProviderError> {
    let mut errors = check_annotations(annotations);
    if !errors.is_empty() {
        return Ok(errors);
    }

    let hints = crate::compiler_hints::analyze_source(provider, source, config_path, source_path)?;
    errors.extend(compiler_errors(
        source,
        file_name,
        source_path,
        hints.diagnostics(),
    ));
    if errors.is_empty() {
        errors.extend(
            crate::verifier::verify_source_with_environment_and_compiler_hints(
                source,
                file_name,
                annotations,
                environment,
                &hints,
            ),
        );
    }
    Ok(errors)
}

fn compiler_errors(
    source: &str,
    file_name: &str,
    source_path: &Path,
    diagnostics: &[crate::type_provider::CompilerDiagnostic],
) -> Vec<RtError> {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.severity == crate::type_provider::CompilerDiagnosticSeverity::Error
        })
        .map(|diagnostic| {
            let diagnostic_is_for_source =
                diagnostic.file.is_empty() || Path::new(&diagnostic.file) == source_path;
            let (line, column) = if diagnostic_is_for_source {
                utf16_offset_to_line_column(source, diagnostic.range.start_utf16)
            } else {
                (1, 1)
            };
            let code = diagnostic
                .code
                .as_deref()
                .map_or(String::new(), |code| format!(" TS{code}"));
            RtError {
                message: format!(
                    "TypeScript {:?} error{code}: {}",
                    diagnostic.kind, diagnostic.message
                ),
                loc: Some(SourceLocation {
                    file: Some(if diagnostic.file.is_empty() {
                        file_name.to_string()
                    } else {
                        diagnostic.file.clone()
                    }),
                    line,
                    column,
                }),
            }
        })
        .collect()
}

fn utf16_offset_to_line_column(source: &str, target: u32) -> (u32, u32) {
    let mut offset = 0u32;
    let mut line = 1u32;
    let mut column = 1u32;
    for character in source.chars() {
        if offset >= target {
            break;
        }
        offset = offset.saturating_add(character.len_utf16() as u32);
        if character == '\n' {
            line = line.saturating_add(1);
            column = 1;
        } else {
            column = column.saturating_add(character.len_utf16() as u32);
        }
    }
    (line, column)
}

pub fn check_annotations(annotations: &[Annotation]) -> Vec<RtError> {
    let mut errors = Vec::new();

    let mut params_by_function: std::collections::HashMap<(String, u32), HashSet<String>> =
        std::collections::HashMap::new();
    for a in annotations {
        if let AnnotationTarget::Param {
            function_name,
            function_start,
            param_name,
            ..
        } = &a.target
        {
            params_by_function
                .entry((function_name.clone(), *function_start))
                .or_default()
                .insert(param_name.clone());
        }
    }

    for a in annotations {
        let mut allowed = HashSet::new();
        for name in &a.predicate_params {
            allowed.insert(format!("@predicate:{name}"));
        }
        let is_return = matches!(a.target, AnnotationTarget::Return { .. });

        match &a.target {
            AnnotationTarget::Param {
                function_name,
                function_start,
                ..
            } => {
                if let Some(set) = params_by_function.get(&(function_name.clone(), *function_start))
                {
                    for p in set {
                        allowed.insert(p.clone());
                    }
                }
                allowed.insert("length".to_string());
            }
            AnnotationTarget::Return {
                function_name,
                function_start,
            } => {
                if let Some(set) = params_by_function.get(&(function_name.clone(), *function_start))
                {
                    for p in set {
                        allowed.insert(p.clone());
                    }
                }
                allowed.insert("$".to_string());
                allowed.insert("length".to_string());
            }
            AnnotationTarget::Variable { name, .. } => {
                allowed.insert(name.clone());
                allowed.insert("length".to_string());
            }
        }

        if let Some(err) = check_type(&a.ty, a.loc.clone()) {
            errors.push(err);
        }

        if let Some(index) = &a.ty.index
            && let Some(err) = check_predicate(index, &allowed, is_return, a.loc.clone())
        {
            errors.push(err);
        }
        if let Some(pred) = &a.ty.predicate
            && let Some(err) = check_predicate(pred, &allowed, is_return, a.loc.clone())
        {
            errors.push(err);
        }
    }

    errors
}

fn check_type(ty: &RefinementType, loc: SourceLocation) -> Option<RtError> {
    match &ty.base {
        BaseType::Array(el) => check_type(&RefinementType::from_base((**el).clone()), loc),
        BaseType::Generic(_, arguments) | BaseType::Union(arguments) => {
            for argument in arguments {
                if let Some(err) =
                    check_type(&RefinementType::from_base(argument.clone()), loc.clone())
                {
                    return Some(err);
                }
            }
            None
        }
        BaseType::Object(fields) => {
            for (_, t) in fields {
                if let Some(err) = check_type(&RefinementType::from_base(t.clone()), loc.clone()) {
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

fn check_predicate(
    pred: &PredicateExpr,
    allowed: &HashSet<String>,
    is_return: bool,
    loc: SourceLocation,
) -> Option<RtError> {
    match pred {
        PredicateExpr::Literal(_) => None,
        PredicateExpr::Identifier(name) => {
            if name == "$" && !is_return {
                return Some(RtError {
                    message: "Return marker '$' is only allowed in return refinement predicates"
                        .into(),
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
        PredicateExpr::Member(object, _) => check_predicate(object, allowed, is_return, loc),
        PredicateExpr::PredicateApply(name, argument) => {
            if !allowed.contains(&format!("@predicate:{name}")) {
                return Some(RtError {
                    message: format!("Unknown predicate parameter '{}'", name),
                    loc: Some(loc),
                });
            }
            check_predicate(argument, allowed, is_return, loc)
        }
        PredicateExpr::Return => {
            if !is_return {
                return Some(RtError {
                    message: "Return marker '$' is only allowed in return refinement predicates"
                        .into(),
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
