use crate::syntax::*;

pub fn merge_prelude(annotations: &mut Vec<Annotation>) {
    let prelude: Vec<Annotation> = vec![
        Annotation {
            target: AnnotationTarget::Param {
                function_name: "Math.sqrt".into(),
                function_start: 0,
                param_name: "x".into(),
                index: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("number".into()),
                predicate: Some(PredicateExpr::Binary(
                    BinaryOp::Gte,
                    Box::new(PredicateExpr::Identifier("x".into())),
                    Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                )),
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
        Annotation {
            target: AnnotationTarget::Return {
                function_name: "Math.sqrt".into(),
                function_start: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("number".into()),
                predicate: Some(PredicateExpr::Logical(
                    LogicalOp::And,
                    Box::new(PredicateExpr::Binary(
                        BinaryOp::Gte,
                        Box::new(PredicateExpr::Return),
                        Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                    )),
                    Box::new(PredicateExpr::Logical(
                        LogicalOp::Or,
                        Box::new(PredicateExpr::Binary(
                            BinaryOp::EqEqEq,
                            Box::new(PredicateExpr::Identifier("x".into())),
                            Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                        )),
                        Box::new(PredicateExpr::Binary(
                            BinaryOp::Gt,
                            Box::new(PredicateExpr::Return),
                            Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                        )),
                    )),
                )),
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
        Annotation {
            target: AnnotationTarget::Param {
                function_name: "Math.abs".into(),
                function_start: 0,
                param_name: "x".into(),
                index: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("number".into()),
                predicate: Some(PredicateExpr::Binary(
                    BinaryOp::EqEqEq,
                    Box::new(PredicateExpr::Identifier("x".into())),
                    Box::new(PredicateExpr::Identifier("x".into())),
                )),
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
        Annotation {
            target: AnnotationTarget::Return {
                function_name: "Math.abs".into(),
                function_start: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("number".into()),
                predicate: Some(PredicateExpr::Binary(
                    BinaryOp::Gte,
                    Box::new(PredicateExpr::Return),
                    Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                )),
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
        Annotation {
            target: AnnotationTarget::Param {
                function_name: "Array.isArray".into(),
                function_start: 0,
                param_name: "x".into(),
                index: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("unknown".into()),
                predicate: None,
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
        Annotation {
            target: AnnotationTarget::Return {
                function_name: "Array.isArray".into(),
                function_start: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("boolean".into()),
                predicate: None,
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
        Annotation {
            target: AnnotationTarget::Param {
                function_name: "Number.isInteger".into(),
                function_start: 0,
                param_name: "x".into(),
                index: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("unknown".into()),
                predicate: None,
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
        Annotation {
            target: AnnotationTarget::Return {
                function_name: "Number.isInteger".into(),
                function_start: 0,
            },
            ty: RefinementType {
                base: BaseType::Primitive("boolean".into()),
                predicate: None,
            },
            predicate_params: vec![],
            loc: SourceLocation {
                file: Some("<prelude>".into()),
                line: 0,
                column: 0,
            },
        },
    ];
    let mut prelude = prelude;
    annotations.append(&mut prelude);
}
