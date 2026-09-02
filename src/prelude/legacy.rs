use crate::syntax::{Annotation, AnnotationTarget, SourceLocation};

use super::{Environment, LibraryRegistry};

const LEGACY_STATIC_FUNCTIONS: [&str; 4] =
    ["Math.sqrt", "Math.abs", "Array.isArray", "Number.isInteger"];

pub(crate) fn append(annotations: &mut Vec<Annotation>) {
    let registry = super::catalog::build(Environment::Ecmascript);
    for function_name in LEGACY_STATIC_FUNCTIONS {
        append_first_overload(annotations, &registry, function_name);
    }
}

fn append_first_overload(
    annotations: &mut Vec<Annotation>,
    registry: &LibraryRegistry,
    function_name: &str,
) {
    let signature = registry
        .static_function(function_name)
        .and_then(|overloads| overloads.first())
        .unwrap_or_else(|| panic!("missing legacy prelude function '{function_name}'"));
    let loc = SourceLocation {
        file: Some("<prelude>".into()),
        line: 0,
        column: 0,
    };

    for (index, parameter) in signature.parameters.iter().enumerate() {
        annotations.push(Annotation {
            target: AnnotationTarget::Param {
                function_name: function_name.to_string(),
                function_start: 0,
                param_name: parameter.name.clone(),
                index,
            },
            ty: parameter.ty.clone(),
            predicate_params: Vec::new(),
            loc: loc.clone(),
        });
    }
    annotations.push(Annotation {
        target: AnnotationTarget::Return {
            function_name: function_name.to_string(),
            function_start: 0,
        },
        ty: signature.returns.clone(),
        predicate_params: Vec::new(),
        loc,
    });
}
