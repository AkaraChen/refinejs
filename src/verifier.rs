use crate::syntax::{
    Annotation, AnnotationTarget, BaseType, BinaryOp, Literal, LogicalOp, PredicateExpr,
    RefinementType, RtError, SourceLocation,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Expression, Function, Program, SimpleAssignmentTarget, Statement,
};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator};
use std::collections::HashMap;
use z3::{
    Fixedpoint, FuncDecl, SatResult, Sort as Z3Sort,
    ast::{self, Ast, Bool, Float, RoundingMode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Sort {
    Number,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Term {
    Number(i64),
    Bool(bool),
    Var(String, Sort),
    Pred(String, Box<Term>),
    Add(Box<Term>, Box<Term>),
    Sub(Box<Term>, Box<Term>),
    Mul(Box<Term>, Box<Term>),
    Same(Box<Term>, Box<Term>),
    Eq(Box<Term>, Box<Term>),
    Ne(Box<Term>, Box<Term>),
    Gt(Box<Term>, Box<Term>),
    Lt(Box<Term>, Box<Term>),
    Ge(Box<Term>, Box<Term>),
    Le(Box<Term>, Box<Term>),
    And(Box<Term>, Box<Term>),
    Or(Box<Term>, Box<Term>),
    Not(Box<Term>),
}

impl Term {
    fn sort(&self) -> Sort {
        match self {
            Self::Number(_) | Self::Add(..) | Self::Sub(..) | Self::Mul(..) => Sort::Number,
            Self::Bool(_)
            | Self::Eq(..)
            | Self::Same(..)
            | Self::Ne(..)
            | Self::Gt(..)
            | Self::Lt(..)
            | Self::Ge(..)
            | Self::Le(..)
            | Self::And(..)
            | Self::Or(..)
            | Self::Not(..) => Sort::Bool,
            Self::Pred(..) => Sort::Bool,
            Self::Var(_, sort) => *sort,
        }
    }
}

#[derive(Debug, Clone)]
struct Qualifier {
    value: Term,
    formula: Term,
}

#[derive(Debug, Clone)]
struct Value {
    term: Term,
    base: BaseType,
    declared_base: Option<BaseType>,
    qualifier: Option<Qualifier>,
    mutable: bool,
}

#[derive(Debug, Clone)]
struct Contract {
    params: Vec<(String, RefinementType)>,
    ret: RefinementType,
    predicate_params: Vec<String>,
    loc: SourceLocation,
}

#[derive(Debug, Clone, Default)]
struct State {
    env: HashMap<String, Value>,
    entry_params: HashMap<String, Term>,
    assumptions: Vec<Term>,
    scopes: Vec<HashMap<String, (Option<Value>, bool)>>,
    uninitialized: std::collections::HashSet<String>,
}

/// A liquid subtyping obligation in Horn-clause form:
/// `assumptions => consequent`. Abstract predicate applications are reduced to
/// freely chosen Boolean atoms with congruence constraints before the Horn
/// rule is sent to Z3.
struct FixpointConstraint<'a> {
    assumptions: &'a [Term],
    consequent: &'a Term,
}

pub fn verify_source(source: &str, file_name: &str, annotations: &[Annotation]) -> Vec<RtError> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::default().with_module(true))
        .with_options(ParseOptions::default())
        .parse();
    if !parsed.diagnostics.is_empty() {
        return vec![RtError {
            message: format!("JavaScript parse errors: {:?}", parsed.diagnostics),
            loc: None,
        }];
    }

    let contracts = collect_contracts(annotations);
    let annotation_errors = validate_annotation_structure(annotations, &contracts);
    let signatures = contracts
        .iter()
        .filter(|(_, contract)| contract.loc.file.as_deref() == Some("<prelude>"))
        .map(|((name, _), contract)| (name.clone(), contract.clone()))
        .collect();
    let variable_types = collect_variable_types(annotations);
    let mut verifier = Verifier {
        source,
        file_name,
        contracts,
        signatures,
        variable_types,
        consumed_variable_types: std::collections::HashSet::new(),
        errors: annotation_errors,
        fresh: 0,
    };
    verifier.verify_program(&parsed.program);
    verifier.errors
}

fn collect_contracts(annotations: &[Annotation]) -> HashMap<(String, u32), Contract> {
    let mut params: HashMap<(String, u32), Vec<(usize, String, RefinementType)>> = HashMap::new();
    let mut returns: HashMap<(String, u32), (RefinementType, Vec<String>, SourceLocation)> =
        HashMap::new();
    for annotation in annotations {
        match &annotation.target {
            AnnotationTarget::Param {
                function_name,
                function_start,
                param_name,
                index,
            } => {
                params
                    .entry((function_name.clone(), *function_start))
                    .or_default()
                    .push((*index, param_name.clone(), annotation.ty.clone()));
            }
            AnnotationTarget::Return {
                function_name,
                function_start,
            } => {
                returns.insert(
                    (function_name.clone(), *function_start),
                    (
                        annotation.ty.clone(),
                        annotation.predicate_params.clone(),
                        annotation.loc.clone(),
                    ),
                );
            }
            AnnotationTarget::Variable { .. } => {}
        }
    }

    returns
        .into_iter()
        .map(
            |((name, declaration_start), (ret, predicate_params, loc))| {
                let key = (name.clone(), declaration_start);
                let mut function_params = params.remove(&key).unwrap_or_default();
                function_params.sort_by_key(|(index, _, _)| *index);
                let params = function_params
                    .into_iter()
                    .map(|(_, name, ty)| (name, ty))
                    .collect();
                (
                    key,
                    Contract {
                        params,
                        ret,
                        predicate_params,
                        loc,
                    },
                )
            },
        )
        .collect()
}

fn collect_variable_types(
    annotations: &[Annotation],
) -> HashMap<u32, (String, RefinementType, SourceLocation)> {
    annotations
        .iter()
        .filter_map(|annotation| match &annotation.target {
            AnnotationTarget::Variable {
                name,
                declaration_start,
            } => Some((
                *declaration_start,
                (name.clone(), annotation.ty.clone(), annotation.loc.clone()),
            )),
            _ => None,
        })
        .collect()
}

fn validate_annotation_structure(
    annotations: &[Annotation],
    contracts: &HashMap<(String, u32), Contract>,
) -> Vec<RtError> {
    let mut errors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for annotation in annotations {
        let key = match &annotation.target {
            AnnotationTarget::Param {
                function_name,
                function_start,
                index,
                ..
            } => format!("param:{function_name}:{function_start}:{index}"),
            AnnotationTarget::Return {
                function_name,
                function_start,
            } => format!("return:{function_name}:{function_start}"),
            AnnotationTarget::Variable {
                declaration_start, ..
            } => format!("variable:{declaration_start}"),
        };
        if !seen.insert(key) {
            errors.push(RtError {
                message: "Duplicate refinement annotation for one declaration".into(),
                loc: Some(annotation.loc.clone()),
            });
        }
        if let AnnotationTarget::Param {
            function_name,
            function_start,
            ..
        } = &annotation.target
        {
            if !contracts.contains_key(&(function_name.clone(), *function_start)) {
                errors.push(RtError {
                    message: format!(
                        "Refined parameter of '{function_name}' requires a function signature"
                    ),
                    loc: Some(annotation.loc.clone()),
                });
            }
        }
    }
    errors
}

struct Verifier<'a> {
    source: &'a str,
    file_name: &'a str,
    contracts: HashMap<(String, u32), Contract>,
    signatures: HashMap<String, Contract>,
    variable_types: HashMap<u32, (String, RefinementType, SourceLocation)>,
    consumed_variable_types: std::collections::HashSet<u32>,
    errors: Vec<RtError>,
    fresh: usize,
}

impl Verifier<'_> {
    fn verify_program(&mut self, program: &Program<'_>) {
        let mut top_level_functions = std::collections::HashSet::new();
        let mut top_level_function_names = std::collections::HashSet::new();
        for statement in &program.body {
            if let Statement::FunctionDeclaration(function) = statement {
                if let Some(identifier) = &function.id {
                    let name = identifier.name.to_string();
                    if !top_level_function_names.insert(name.clone()) {
                        self.error(
                            format!("Duplicate top-level function name '{name}'"),
                            function.span,
                        );
                        continue;
                    }
                    if is_reserved_runtime_root(&name) {
                        self.error(
                            format!("'{name}' is reserved by the refinement runtime or prelude"),
                            function.span,
                        );
                        continue;
                    }
                    let key = (name.clone(), function.span.start);
                    top_level_functions.insert(key.clone());
                    if let Some(contract) = self.contracts.get(&key).cloned() {
                        if self.signatures.insert(name.clone(), contract).is_some() {
                            self.error(
                                format!("Duplicate refined function name '{name}'"),
                                function.span,
                            );
                        }
                    }
                }
            }
        }
        for statement in &program.body {
            if let Statement::FunctionDeclaration(function) = statement {
                self.verify_function(function);
            }
        }
        for ((name, declaration_start), contract) in &self.contracts {
            if contract.loc.file.as_deref() != Some("<prelude>")
                && !top_level_functions.contains(&(name.clone(), *declaration_start))
            {
                self.errors.push(RtError {
                    message: format!(
                        "Refined function '{name}' must be a top-level function declaration"
                    ),
                    loc: Some(contract.loc.clone()),
                });
            }
        }

        let statements: Vec<&Statement<'_>> = program
            .body
            .iter()
            .filter(|statement| !matches!(statement, Statement::FunctionDeclaration(_)))
            .collect();
        let mut states = vec![State::default()];
        for state in &mut states {
            Self::enter_scope(&statements, state);
        }
        for statement in statements {
            states = self.verify_statement(statement, states, None);
        }
        let unconsumed: Vec<_> = self
            .variable_types
            .iter()
            .filter(|(start, _)| !self.consumed_variable_types.contains(start))
            .map(|(_, (name, _, loc))| (name.clone(), loc.clone()))
            .collect();
        for (name, loc) in unconsumed {
            self.errors.push(RtError {
                message: format!("Refined variable '{name}' is outside a statically checked scope"),
                loc: Some(loc),
            });
        }
    }

    fn verify_function(&mut self, function: &Function<'_>) {
        let Some(name) = function.id.as_ref().map(|id| id.name.to_string()) else {
            return;
        };
        let Some(contract) = self
            .contracts
            .get(&(name.clone(), function.span.start))
            .cloned()
        else {
            return;
        };
        let Some(body) = &function.body else { return };
        if function.r#async || function.generator {
            self.error(
                "Async and generator functions are outside the supported refinement subset".into(),
                function.span,
            );
            return;
        }
        if function.params.items.len() != contract.params.len() {
            self.error(
                format!(
                    "Refinement signature for '{name}' declares {} parameters, but JavaScript declares {}",
                    contract.params.len(),
                    function.params.items.len()
                ),
                function.params.span,
            );
            return;
        }
        if function.params.rest.is_some() {
            self.error(
                "Rest parameters are outside the supported refinement subset".into(),
                function.params.span,
            );
            return;
        }
        for (formal, (annotated_name, _)) in function.params.items.iter().zip(&contract.params) {
            if formal.initializer.is_some() {
                self.error(
                    "Default parameters are outside the supported refinement subset".into(),
                    formal.span,
                );
                return;
            }
            let actual_name = match &formal.pattern {
                BindingPattern::BindingIdentifier(identifier) => Some(identifier.name.as_str()),
                _ => None,
            };
            if actual_name != Some(annotated_name.as_str()) {
                self.error(
                    format!(
                        "Refinement parameter '{}' does not match the JavaScript parameter",
                        annotated_name
                    ),
                    formal.span,
                );
                return;
            }
            if actual_name == Some("__rt") {
                self.error(
                    "'__rt' is reserved for refinement runtime assertions".into(),
                    formal.span,
                );
                return;
            }
        }
        let mut state = State::default();
        let mut replacements = HashMap::new();

        for predicate_name in &contract.predicate_params {
            if !contract
                .params
                .iter()
                .any(|(_, ty)| contains_predicate(ty.predicate.as_ref(), predicate_name))
            {
                self.error(
                    format!(
                        "Predicate parameter '{predicate_name}' must occur in a parameter refinement"
                    ),
                    function.span,
                );
                return;
            }
        }

        for (param_name, ty) in &contract.params {
            let term = Term::Var(format!("{name}.{param_name}"), sort_for_base(&ty.base));
            replacements.insert(param_name.clone(), term.clone());
            state.entry_params.insert(param_name.clone(), term.clone());
            state.env.insert(
                param_name.clone(),
                Value {
                    term,
                    base: ty.base.clone(),
                    declared_base: Some(ty.base.clone()),
                    qualifier: None,
                    mutable: false,
                },
            );
        }
        let mut predicate_replacements = replacements.clone();
        predicate_replacements.insert(
            "$".into(),
            Term::Var(format!("{name}.$return"), sort_for_base(&contract.ret.base)),
        );
        if let Err(message) =
            validate_predicate_parameter_domains(&contract, &predicate_replacements)
        {
            self.error(message, function.span);
            return;
        }
        for (param_name, ty) in &contract.params {
            if let Some(predicate) = &ty.predicate {
                match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                    Ok(formula) => {
                        state.assumptions.push(formula.clone());
                        if let Some(value) = state.env.get_mut(param_name) {
                            value.qualifier = Some(Qualifier {
                                value: value.term.clone(),
                                formula,
                            });
                        }
                    }
                    Err(message) => self.error(message, function.span),
                }
            }
        }

        let mut states = vec![state];
        let body_statements: Vec<_> = body.statements.iter().collect();
        for state in &mut states {
            Self::enter_scope(&body_statements, state);
        }
        for statement in &body.statements {
            states = self.verify_statement(statement, states, Some((&name, &contract)));
        }
        if !states.is_empty() && !is_void(&contract.ret.base) {
            self.errors.push(RtError {
                message: format!("Function '{name}' may complete without returning a value"),
                loc: Some(contract.loc),
            });
        }
    }

    fn verify_statement<'a>(
        &mut self,
        statement: &'a Statement<'a>,
        states: Vec<State>,
        current_function: Option<(&str, &Contract)>,
    ) -> Vec<State> {
        match statement {
            Statement::BlockStatement(block) => {
                let mut current = states;
                let block_statements: Vec<_> = block.body.iter().collect();
                for state in &mut current {
                    Self::enter_scope(&block_statements, state);
                }
                for statement in &block.body {
                    current = self.verify_statement(statement, current, current_function);
                }
                for state in &mut current {
                    Self::leave_scope(state);
                }
                current
            }
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    if self.variable_types.contains_key(&declarator.span.start) {
                        self.consumed_variable_types.insert(declarator.span.start);
                    }
                }
                if declaration.kind.is_var() || declaration.kind.is_using() {
                    self.error(
                        "Only let and const declarations are supported by refinement checking"
                            .into(),
                        declaration.span,
                    );
                    return Vec::new();
                }
                let mut output = Vec::new();
                for mut state in states {
                    for declarator in &declaration.declarations {
                        let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
                            self.error(
                                "Destructuring declarations are outside the supported refinement subset"
                                    .into(),
                                declarator.id.span(),
                            );
                            continue;
                        };
                        let annotation = self.variable_types.get(&declarator.span.start).cloned();
                        let name = identifier.name.to_string();
                        if is_reserved_runtime_root(&name) {
                            self.error(
                                format!(
                                    "'{name}' is reserved by the refinement runtime or prelude"
                                ),
                                declarator.span,
                            );
                            continue;
                        }
                        if self.signatures.contains_key(&name) {
                            self.error(
                                format!(
                                    "Declaration '{name}' shadows a refined function signature"
                                ),
                                declarator.span,
                            );
                            continue;
                        }
                        if current_function.is_some_and(|(_, contract)| {
                            contract.params.iter().any(|(param, _)| param == &name)
                        }) {
                            self.error(
                                format!(
                                    "Declaration shadows refined parameter '{name}', which is not supported"
                                ),
                                declarator.span,
                            );
                            continue;
                        }
                        if !Self::initialize_name(&name, &mut state) {
                            self.error(
                                format!("Duplicate declaration of '{name}' in one scope"),
                                declarator.span,
                            );
                            continue;
                        }
                        let Some(initializer) = &declarator.init else {
                            let loc = annotation
                                .map(|(_, _, loc)| loc)
                                .unwrap_or_else(|| self.location(declarator.span));
                            self.errors.push(RtError {
                                message: format!(
                                    "Variable '{name}' requires an initializer in refinement checking"
                                ),
                                loc: Some(loc),
                            });
                            continue;
                        };
                        if let Some(mut value) = self.infer_expression(initializer, &mut state) {
                            value.mutable = !declaration.kind.is_const();
                            let mut declared_predicate = None;
                            if let Some((annotated_name, annotation, loc)) = annotation {
                                debug_assert_eq!(annotated_name, name);
                                self.check_base(&value.base, &annotation.base, initializer.span());
                                value.base = annotation.base.clone();
                                value.declared_base = Some(annotation.base.clone());
                                if let Some(predicate) = &annotation.predicate {
                                    let replacements =
                                        HashMap::from([(name.clone(), value.term.clone())]);
                                    match predicate_term(
                                        predicate,
                                        &replacements,
                                        &HashMap::new(),
                                        None,
                                    ) {
                                        Ok(goal) => {
                                            self.prove(
                                                &state.assumptions,
                                                &goal,
                                                format!("Initializer for '{name}' does not satisfy its refinement"),
                                                loc,
                                            );
                                            declared_predicate = Some(predicate.clone());
                                        }
                                        Err(message) => self.error(message, declarator.span),
                                    }
                                }
                            }
                            if declared_predicate.is_some() {
                                value.qualifier = None;
                            }
                            self.bind_value(&name, value, &mut state);
                            if let Some(predicate) = declared_predicate {
                                let symbol = state.env[&name].term.clone();
                                let replacements = HashMap::from([(name.clone(), symbol.clone())]);
                                match predicate_term(
                                    &predicate,
                                    &replacements,
                                    &HashMap::new(),
                                    None,
                                ) {
                                    Ok(formula) => {
                                        state.assumptions.push(formula.clone());
                                        state.env.get_mut(&name).unwrap().qualifier =
                                            Some(Qualifier {
                                                value: symbol,
                                                formula,
                                            });
                                    }
                                    Err(message) => self.error(message, declarator.span),
                                }
                            }
                        }
                    }
                    output.push(state);
                }
                output
            }
            Statement::ExpressionStatement(expression_statement) => {
                let mut output = Vec::new();
                for mut state in states {
                    if let Expression::AssignmentExpression(assignment) =
                        &expression_statement.expression
                    {
                        if assignment.operator == AssignmentOperator::Assign {
                            if let Some(SimpleAssignmentTarget::AssignmentTargetIdentifier(
                                identifier,
                            )) = assignment.left.as_simple_assignment_target()
                            {
                                let name = identifier.name.as_str();
                                if current_function.is_some_and(|(_, contract)| {
                                    contract.params.iter().any(|(param, _)| param == name)
                                }) {
                                    self.error(
                                        format!("Reassignment of refined parameter '{name}' is not supported"),
                                        assignment.span,
                                    );
                                    output.push(state);
                                    continue;
                                }
                                let Some(previous) = state.env.get(name).cloned() else {
                                    self.error(
                                        format!("Assignment to untracked variable '{name}'"),
                                        identifier.span,
                                    );
                                    output.push(state);
                                    continue;
                                };
                                if !previous.mutable {
                                    self.error(
                                        format!("Assignment to immutable binding '{name}'"),
                                        assignment.span,
                                    );
                                    output.push(state);
                                    continue;
                                }
                                if let Some(mut value) =
                                    self.infer_expression(&assignment.right, &mut state)
                                {
                                    if let Some(expected) = &previous.declared_base {
                                        self.check_base(
                                            &value.base,
                                            expected,
                                            assignment.right.span(),
                                        );
                                        value.declared_base = Some(expected.clone());
                                    }
                                    value.mutable = previous.mutable;
                                    self.bind_value(name, value, &mut state);
                                }
                            } else {
                                self.error(
                                    "Only identifier assignment is supported by refinement checking".into(),
                                    assignment.left.span(),
                                );
                            }
                        } else {
                            self.error(
                                "Compound assignment is outside the supported static refinement subset".into(),
                                assignment.span,
                            );
                        }
                    } else {
                        self.infer_expression(&expression_statement.expression, &mut state);
                    }
                    output.push(state);
                }
                output
            }
            Statement::IfStatement(if_statement) => {
                let mut output = Vec::new();
                for mut state in states {
                    let Some(test) = self.infer_expression(&if_statement.test, &mut state) else {
                        continue;
                    };
                    if value_sort(&test) != Some(Sort::Bool) {
                        self.error(
                            "if condition must be boolean".into(),
                            if_statement.test.span(),
                        );
                        continue;
                    }
                    let mut then_state = state.clone();
                    then_state.assumptions.push(test.term.clone());
                    Self::narrow_qualifiers(&mut then_state, &test.term);
                    if Self::path_is_reachable(&then_state) {
                        output.extend(self.verify_statement(
                            &if_statement.consequent,
                            vec![then_state],
                            current_function,
                        ));
                    }

                    let mut else_state = state;
                    let negated = Term::Not(Box::new(test.term));
                    else_state.assumptions.push(negated.clone());
                    Self::narrow_qualifiers(&mut else_state, &negated);
                    if Self::path_is_reachable(&else_state) {
                        if let Some(alternate) = &if_statement.alternate {
                            output.extend(self.verify_statement(
                                alternate,
                                vec![else_state],
                                current_function,
                            ));
                        } else {
                            output.push(else_state);
                        }
                    }
                }
                output
            }
            Statement::ReturnStatement(return_statement) => {
                let Some((function_name, contract)) = current_function else {
                    return Vec::new();
                };
                for mut state in states {
                    let value = return_statement
                        .argument
                        .as_ref()
                        .and_then(|argument| self.infer_expression(argument, &mut state));
                    let Some(value) = value else {
                        if !is_void(&contract.ret.base) {
                            self.error(
                                format!("Function '{function_name}' returns no value"),
                                return_statement.span,
                            );
                        }
                        continue;
                    };
                    self.check_base(&value.base, &contract.ret.base, return_statement.span);
                    if let Some(predicate) = &contract.ret.predicate {
                        let mut replacements =
                            HashMap::from([("$".to_string(), value.term.clone())]);
                        for (param_name, term) in &state.entry_params {
                            replacements.insert(param_name.clone(), term.clone());
                        }
                        match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                            Ok(goal) => self.prove(
                                &state.assumptions,
                                &goal,
                                format!("Return value of '{function_name}' does not satisfy its refinement"),
                                contract.loc.clone(),
                            ),
                            Err(message) => self.error(message, return_statement.span),
                        }
                    }
                }
                Vec::new()
            }
            Statement::EmptyStatement(_) => states,
            _ => {
                self.error(
                    "Statement is outside the supported static refinement subset".into(),
                    statement.span(),
                );
                Vec::new()
            }
        }
    }

    fn infer_expression(
        &mut self,
        expression: &Expression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        match expression {
            Expression::NumericLiteral(literal) => {
                if literal.value.fract() != 0.0 || literal.value.abs() > 9_007_199_254_740_991_f64 {
                    self.error(
                        "Only safe integer literals are supported by refinement checking".into(),
                        literal.span,
                    );
                    return None;
                }
                let term = Term::Number(literal.value as i64);
                let placeholder = Term::Var(self.fresh_name("literal"), Sort::Number);
                Some(Value {
                    term: term.clone(),
                    base: number_type(),
                    declared_base: None,
                    qualifier: Some(Qualifier {
                        value: placeholder.clone(),
                        formula: Term::Eq(Box::new(placeholder), Box::new(term)),
                    }),
                    mutable: true,
                })
            }
            Expression::BooleanLiteral(literal) => {
                let term = Term::Bool(literal.value);
                let placeholder = Term::Var(self.fresh_name("literal"), Sort::Bool);
                Some(Value {
                    term: term.clone(),
                    base: boolean_type(),
                    declared_base: None,
                    qualifier: Some(Qualifier {
                        value: placeholder.clone(),
                        formula: Term::Eq(Box::new(placeholder), Box::new(term)),
                    }),
                    mutable: true,
                })
            }
            Expression::Identifier(identifier) => {
                let name = identifier.name.as_str();
                if state.uninitialized.contains(name) {
                    self.error(
                        format!("Binding '{name}' is used before its declaration"),
                        identifier.span,
                    );
                    return None;
                }
                state.env.get(name).cloned().or_else(|| {
                    self.error(
                        format!("No static type information for '{}'", identifier.name),
                        identifier.span,
                    );
                    None
                })
            }
            Expression::ParenthesizedExpression(parenthesized) => {
                self.infer_expression(&parenthesized.expression, state)
            }
            Expression::UnaryExpression(unary) => {
                let value = self.infer_expression(&unary.argument, state)?;
                let Some(value_sort) = value_sort(&value) else {
                    self.error(
                        "Unary operators require a number or boolean operand in refinement checking"
                            .into(),
                        unary.span,
                    );
                    return None;
                };
                let term = match unary.operator {
                    UnaryOperator::LogicalNot if value_sort == Sort::Bool => {
                        Term::Not(Box::new(value.term))
                    }
                    UnaryOperator::UnaryNegation if value_sort == Sort::Number => {
                        Term::Sub(Box::new(Term::Number(0)), Box::new(value.term))
                    }
                    UnaryOperator::UnaryPlus if value_sort == Sort::Number => value.term,
                    _ => {
                        self.error(
                            "Unsupported unary expression in refinement analysis".into(),
                            unary.span,
                        );
                        return None;
                    }
                };
                Some(Value {
                    base: base_for_sort(term.sort()),
                    term,
                    declared_base: None,
                    qualifier: None,
                    mutable: true,
                })
            }
            Expression::BinaryExpression(binary) => {
                let left = self.infer_expression(&binary.left, state)?;
                let right = self.infer_expression(&binary.right, state)?;
                if value_sort(&left).is_none() || value_sort(&right).is_none() {
                    self.error(
                        "Binary operators require number or boolean operands in refinement checking"
                            .into(),
                        binary.span,
                    );
                    return None;
                }
                let term = binary_term(binary.operator, left.term, right.term)
                    .map_err(|message| {
                        self.error(message, binary.span);
                    })
                    .ok()?;
                Some(Value {
                    base: base_for_sort(term.sort()),
                    term,
                    declared_base: None,
                    qualifier: None,
                    mutable: true,
                })
            }
            Expression::LogicalExpression(logical) => {
                let left = self.infer_expression(&logical.left, state)?;
                let right = self.infer_expression(&logical.right, state)?;
                if value_sort(&left) != Some(Sort::Bool) || value_sort(&right) != Some(Sort::Bool) {
                    self.error(
                        "Logical expressions require boolean operands in refinement checking"
                            .into(),
                        logical.span,
                    );
                    return None;
                }
                let term = match logical.operator {
                    LogicalOperator::And => Term::And(Box::new(left.term), Box::new(right.term)),
                    LogicalOperator::Or => Term::Or(Box::new(left.term), Box::new(right.term)),
                    LogicalOperator::Coalesce => {
                        self.error(
                            "Nullish coalescing is not supported in refinements".into(),
                            logical.span,
                        );
                        return None;
                    }
                };
                Some(Value {
                    term,
                    base: boolean_type(),
                    declared_base: None,
                    qualifier: None,
                    mutable: true,
                })
            }
            Expression::CallExpression(call) => self.infer_call(call, state),
            _ => {
                self.error(
                    "Expression is outside the supported static refinement subset".into(),
                    expression.span(),
                );
                None
            }
        }
    }

    fn infer_call(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        if let Some(root_name) = expression_root_name(&call.callee) {
            if state.uninitialized.contains(&root_name) {
                self.error(
                    format!("Call target '{root_name}' is used before its declaration"),
                    call.callee.span(),
                );
                return None;
            }
            if state.env.contains_key(&root_name) {
                self.error(
                    format!(
                        "Call target '{}' is shadowed by a local binding",
                        expression_name(&call.callee).unwrap_or(root_name)
                    ),
                    call.callee.span(),
                );
                return None;
            }
        }
        let Some(function_name) = expression_name(&call.callee) else {
            self.error(
                "Unsupported call target in refinement analysis".into(),
                call.callee.span(),
            );
            return None;
        };
        if function_name == "console.log" {
            for argument in &call.arguments {
                let Some(expression) = argument.as_expression() else {
                    self.error(
                        "Spread arguments are not supported by refinement checking".into(),
                        argument.span(),
                    );
                    return None;
                };
                self.infer_expression(expression, state)?;
            }
            return Some(Value {
                term: Term::Bool(true),
                base: BaseType::Primitive("void".into()),
                declared_base: None,
                qualifier: None,
                mutable: true,
            });
        }
        let Some(contract) = self.signatures.get(&function_name).cloned() else {
            self.error(
                format!("No refinement signature for function '{function_name}'"),
                call.span,
            );
            return None;
        };
        if call.arguments.len() != contract.params.len() {
            self.error(
                format!(
                    "Function '{function_name}' expects {} arguments, got {}",
                    contract.params.len(),
                    call.arguments.len()
                ),
                call.span,
            );
            return None;
        }
        let mut arguments = Vec::new();
        for argument in &call.arguments {
            let Some(expression) = argument.as_expression() else {
                self.error(
                    "Spread arguments are not supported by refinement checking".into(),
                    argument.span(),
                );
                return None;
            };
            arguments.push(self.infer_expression(expression, state)?);
        }

        let replacements: HashMap<String, Term> = contract
            .params
            .iter()
            .zip(&arguments)
            .map(|((name, _), value)| (name.clone(), value.term.clone()))
            .collect();
        let mut predicate_arguments = HashMap::new();
        for predicate_name in &contract.predicate_params {
            for ((_, param_type), argument) in contract.params.iter().zip(&arguments) {
                if contains_predicate(param_type.predicate.as_ref(), predicate_name) {
                    if let Some(qualifier) = &argument.qualifier {
                        predicate_arguments.insert(predicate_name.clone(), qualifier.clone());
                        break;
                    }
                }
            }
            if !predicate_arguments.contains_key(predicate_name) {
                self.error(
                    format!("Cannot infer refinement predicate '{predicate_name}' at call to '{function_name}'"),
                    call.span,
                );
                return None;
            }
        }

        for (((_, parameter), argument), index) in
            contract.params.iter().zip(&arguments).zip(0usize..)
        {
            self.check_base(&argument.base, &parameter.base, call.span);
            if let Some(predicate) = &parameter.predicate {
                match predicate_term(predicate, &replacements, &predicate_arguments, None) {
                    Ok(goal) => self.prove(
                        &state.assumptions,
                        &goal,
                        format!(
                            "Argument {} to '{function_name}' does not satisfy its refinement",
                            index + 1
                        ),
                        self.location(call.span),
                    ),
                    Err(message) => self.error(message, call.span),
                }
            }
        }

        let result = Term::Var(
            self.fresh_name(&function_name),
            sort_for_base(&contract.ret.base),
        );
        let qualifier = if let Some(predicate) = &contract.ret.predicate {
            let mut result_replacements = replacements;
            result_replacements.insert("$".into(), result.clone());
            match predicate_term(predicate, &result_replacements, &predicate_arguments, None) {
                Ok(formula) => {
                    state.assumptions.push(formula.clone());
                    Some(Qualifier {
                        value: result.clone(),
                        formula,
                    })
                }
                Err(message) => {
                    self.error(message, call.span);
                    None
                }
            }
        } else {
            None
        };
        Some(Value {
            term: result,
            base: contract.ret.base,
            declared_base: None,
            qualifier,
            mutable: true,
        })
    }

    fn check_base(&mut self, actual: &BaseType, expected: &BaseType, span: Span) {
        if !bases_compatible(actual, expected) {
            self.error(
                format!(
                    "Base type mismatch: expected {:?}, found {:?}",
                    expected, actual
                ),
                span,
            );
        }
    }

    fn bind_value(&mut self, name: &str, value: Value, state: &mut State) {
        let symbol = Term::Var(self.fresh_name(name), sort_for_base(&value.base));
        state.assumptions.push(Term::Same(
            Box::new(symbol.clone()),
            Box::new(value.term.clone()),
        ));

        let qualifier = value.qualifier.map(|qualifier| {
            let formula = substitute(&qualifier.formula, &qualifier.value, &symbol);
            state.assumptions.push(formula.clone());
            Qualifier {
                value: symbol.clone(),
                formula,
            }
        });
        state.env.insert(
            name.to_string(),
            Value {
                term: symbol,
                base: value.base,
                declared_base: value.declared_base,
                qualifier,
                mutable: value.mutable,
            },
        );
    }

    fn enter_scope(statements: &[&Statement<'_>], state: &mut State) {
        state.scopes.push(HashMap::new());
        for statement in statements {
            let Statement::VariableDeclaration(declaration) = statement else {
                continue;
            };
            if declaration.kind.is_var() || declaration.kind.is_using() {
                continue;
            }
            for declarator in &declaration.declarations {
                let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
                    continue;
                };
                let name = identifier.name.to_string();
                if state.scopes.last().unwrap().contains_key(&name) {
                    continue;
                }
                let previous = state.env.remove(&name);
                let previously_uninitialized = state.uninitialized.remove(&name);
                state
                    .scopes
                    .last_mut()
                    .unwrap()
                    .insert(name.clone(), (previous, previously_uninitialized));
                state.uninitialized.insert(name);
            }
        }
    }

    fn initialize_name(name: &str, state: &mut State) -> bool {
        if state.scopes.is_empty() {
            state.scopes.push(HashMap::new());
        }
        if state.scopes.last().unwrap().contains_key(name) {
            return state.uninitialized.remove(name);
        }
        let previous = state.env.remove(name);
        let previously_uninitialized = state.uninitialized.remove(name);
        state
            .scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (previous, previously_uninitialized));
        true
    }

    fn leave_scope(state: &mut State) {
        let Some(scope) = state.scopes.pop() else {
            return;
        };
        for (name, (previous, previously_uninitialized)) in scope {
            state.env.remove(&name);
            if let Some(previous) = previous {
                state.env.insert(name.clone(), previous);
            }
            if previously_uninitialized {
                state.uninitialized.insert(name);
            } else {
                state.uninitialized.remove(&name);
            }
        }
    }

    fn narrow_qualifiers(state: &mut State, fact: &Term) {
        for value in state.env.values_mut() {
            if !contains_term(fact, &value.term) {
                continue;
            }
            let formula = match value.qualifier.take() {
                Some(previous) => Term::And(Box::new(previous.formula), Box::new(fact.clone())),
                None => fact.clone(),
            };
            value.qualifier = Some(Qualifier {
                value: value.term.clone(),
                formula,
            });
        }
    }

    fn path_is_reachable(state: &State) -> bool {
        let impossible = Term::Bool(false);
        let constraint = FixpointConstraint {
            assumptions: &state.assumptions,
            consequent: &impossible,
        };
        !matches!(solve_constraint(&constraint), Ok(SatResult::Unsat))
    }

    fn prove(&mut self, assumptions: &[Term], goal: &Term, message: String, loc: SourceLocation) {
        let constraint = FixpointConstraint {
            assumptions,
            consequent: goal,
        };
        match solve_constraint(&constraint) {
            Ok(SatResult::Unsat) => {}
            Ok(SatResult::Sat) => self.errors.push(RtError {
                message,
                loc: Some(loc),
            }),
            Ok(SatResult::Unknown) => self.errors.push(RtError {
                message: format!("Z3 returned unknown while checking: {message}"),
                loc: Some(loc),
            }),
            Err(error) => self.errors.push(RtError {
                message: error,
                loc: Some(loc),
            }),
        }
    }

    fn fresh_name(&mut self, prefix: &str) -> String {
        self.fresh += 1;
        format!("{prefix}#{}", self.fresh)
    }

    fn error(&mut self, message: String, span: Span) {
        self.errors.push(RtError {
            message,
            loc: Some(self.location(span)),
        });
    }

    fn location(&self, span: Span) -> SourceLocation {
        let offset = span.start as usize;
        let prefix = &self.source[..offset.min(self.source.len())];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let column = prefix
            .rsplit_once('\n')
            .map_or(prefix.len(), |(_, tail)| tail.len()) as u32
            + 1;
        SourceLocation {
            file: Some(self.file_name.into()),
            line,
            column,
        }
    }
}

fn solve_constraint(constraint: &FixpointConstraint<'_>) -> Result<SatResult, String> {
    let (assumptions, consequent) = abstract_predicate_applications(constraint)?;
    if assumptions.contains(&consequent) {
        return Ok(SatResult::Unsat);
    }
    let constraint = FixpointConstraint {
        assumptions: &assumptions,
        consequent: &consequent,
    };
    let fixedpoint = Fixedpoint::new();
    let bool_sort = Z3Sort::bool();
    let bad_decl = FuncDecl::new("__rt_bad", &[], &bool_sort);
    fixedpoint.register_relation(&bad_decl);
    let bad = bad_decl.apply(&[]).as_bool().unwrap();

    let mut body = Vec::new();
    for assumption in &assumptions {
        match to_z3(assumption)? {
            ZTerm::Bool(value) => body.push(value),
            ZTerm::Number(_) => return Err("Fixpoint assumption is not boolean".into()),
        }
    }
    let ZTerm::Bool(goal) = to_z3(&consequent)? else {
        return Err("Refinement predicate is not boolean".into());
    };
    body.push(goal.not());
    let body_refs: Vec<&Bool> = body.iter().collect();
    let rule = Bool::and(&body_refs).implies(&bad);

    let variables = constraint_variables(&constraint);
    let number_vars: Vec<Float> = variables
        .iter()
        .filter(|(_, sort)| *sort == Sort::Number)
        .map(|(name, _)| Float::new_const_double(name.as_str()))
        .collect();
    let bool_vars: Vec<Bool> = variables
        .iter()
        .filter(|(_, sort)| *sort == Sort::Bool)
        .map(|(name, _)| Bool::new_const(name.as_str()))
        .collect();
    let mut bounds: Vec<&dyn Ast> = number_vars.iter().map(|var| var as &dyn Ast).collect();
    bounds.extend(bool_vars.iter().map(|var| var as &dyn Ast));
    let rule = ast::forall_const(&bounds, &[], &rule);
    fixedpoint.add_rule(&rule, Some("refinement_violation"));
    Ok(fixedpoint.query(&bad))
}

/// Fixedpoint relations have least-model semantics. Registering an abstract
/// predicate parameter such as `p` without defining rules would therefore
/// make `p` empty and could prove arbitrary consequences from `p(x)`.
///
/// Treat every predicate application as a freely chosen Boolean atom instead.
/// Pairwise congruence constraints preserve the uninterpreted-function law
/// that equal arguments have equal predicate results. The resulting formula
/// is still checked as a Horn rule by `Fixedpoint`, but no empty least model
/// can discharge a polymorphic obligation vacuously.
fn abstract_predicate_applications(
    constraint: &FixpointConstraint<'_>,
) -> Result<(Vec<Term>, Term), String> {
    let mut atoms = Vec::new();
    let mut assumptions: Vec<Term> = constraint
        .assumptions
        .iter()
        .map(|term| replace_predicate_applications(term, &mut atoms))
        .collect();
    let consequent = replace_predicate_applications(constraint.consequent, &mut atoms);

    let mut domains = HashMap::new();
    for (name, argument, _) in &atoms {
        let sort = argument.sort();
        if domains
            .insert(name.clone(), sort)
            .is_some_and(|found| found != sort)
        {
            return Err(format!(
                "Predicate parameter '{name}' is applied to incompatible base types"
            ));
        }
    }

    for left_index in 0..atoms.len() {
        for right_index in (left_index + 1)..atoms.len() {
            let (left_name, left_argument, left_atom) = &atoms[left_index];
            let (right_name, right_argument, right_atom) = &atoms[right_index];
            if left_name != right_name {
                continue;
            }
            let arguments_differ = Term::Not(Box::new(Term::Same(
                Box::new(left_argument.clone()),
                Box::new(right_argument.clone()),
            )));
            let results_match = Term::Eq(Box::new(left_atom.clone()), Box::new(right_atom.clone()));
            assumptions.push(Term::Or(
                Box::new(arguments_differ),
                Box::new(results_match),
            ));
        }
    }

    Ok((assumptions, consequent))
}

fn replace_predicate_applications(term: &Term, atoms: &mut Vec<(String, Term, Term)>) -> Term {
    match term {
        Term::Pred(name, argument) => {
            let argument = replace_predicate_applications(argument, atoms);
            if let Some((_, _, atom)) = atoms.iter().find(|(found_name, found_argument, _)| {
                found_name == name && found_argument == &argument
            }) {
                return atom.clone();
            }
            let atom = Term::Var(format!("@predicate_atom:{}", atoms.len()), Sort::Bool);
            atoms.push((name.clone(), argument, atom.clone()));
            atom
        }
        Term::Add(left, right) => Term::Add(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Sub(left, right) => Term::Sub(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Mul(left, right) => Term::Mul(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Same(left, right) => Term::Same(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Eq(left, right) => Term::Eq(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Ne(left, right) => Term::Ne(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Gt(left, right) => Term::Gt(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Lt(left, right) => Term::Lt(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Ge(left, right) => Term::Ge(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Le(left, right) => Term::Le(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::And(left, right) => Term::And(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Or(left, right) => Term::Or(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Not(inner) => Term::Not(Box::new(replace_predicate_applications(inner, atoms))),
        Term::Number(_) | Term::Bool(_) | Term::Var(_, _) => term.clone(),
    }
}

fn predicate_term(
    predicate: &PredicateExpr,
    replacements: &HashMap<String, Term>,
    predicate_arguments: &HashMap<String, Qualifier>,
    expected: Option<Sort>,
) -> Result<Term, String> {
    match predicate {
        PredicateExpr::Literal(Literal::Number(value))
            if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991_f64 =>
        {
            Ok(Term::Number(*value as i64))
        }
        PredicateExpr::Literal(Literal::Boolean(value)) => Ok(Term::Bool(*value)),
        PredicateExpr::Literal(Literal::String(_)) => {
            Err("String predicates are not supported by the integer/boolean solver".into())
        }
        PredicateExpr::Literal(Literal::Number(_)) => {
            Err("Only safe integer-valued number refinements are supported".into())
        }
        PredicateExpr::Identifier(name) => replacements
            .get(name)
            .cloned()
            .ok_or_else(|| format!("No symbolic value for '{name}'")),
        PredicateExpr::Return => replacements
            .get("$")
            .cloned()
            .ok_or_else(|| "No symbolic return value".into()),
        PredicateExpr::Member(object, property) => {
            let sort = expected.unwrap_or(Sort::Number);
            Ok(Term::Var(format!("{object}.{property}"), sort))
        }
        PredicateExpr::PredicateApply(name, argument) => {
            let argument = predicate_term(argument, replacements, predicate_arguments, None)?;
            if let Some(qualifier) = predicate_arguments.get(name) {
                Ok(substitute(&qualifier.formula, &qualifier.value, &argument))
            } else {
                Ok(Term::Pred(name.clone(), Box::new(argument)))
            }
        }
        PredicateExpr::Not(inner) => {
            let inner = predicate_term(inner, replacements, predicate_arguments, Some(Sort::Bool))?;
            if inner.sort() != Sort::Bool {
                return Err("Logical not requires a boolean predicate".into());
            }
            Ok(Term::Not(Box::new(inner)))
        }
        PredicateExpr::Logical(operator, left, right) => {
            let left = predicate_term(left, replacements, predicate_arguments, Some(Sort::Bool))?;
            let right = predicate_term(right, replacements, predicate_arguments, Some(Sort::Bool))?;
            if left.sort() != Sort::Bool || right.sort() != Sort::Bool {
                return Err("Logical predicates require boolean operands".into());
            }
            Ok(match operator {
                LogicalOp::And => Term::And(Box::new(left), Box::new(right)),
                LogicalOp::Or => Term::Or(Box::new(left), Box::new(right)),
            })
        }
        PredicateExpr::Binary(operator, left, right) => {
            let operand_sort = match operator {
                BinaryOp::EqEqEq | BinaryOp::NotEqEq | BinaryOp::EqEq | BinaryOp::NotEq => {
                    predicate_literal_sort(left)
                        .or_else(|| predicate_literal_sort(right))
                        .unwrap_or(Sort::Number)
                }
                _ => Sort::Number,
            };
            let left = predicate_term(left, replacements, predicate_arguments, Some(operand_sort))?;
            let right =
                predicate_term(right, replacements, predicate_arguments, Some(operand_sort))?;
            predicate_binary(operator, left, right)
        }
    }
}

fn predicate_literal_sort(predicate: &PredicateExpr) -> Option<Sort> {
    match predicate {
        PredicateExpr::Literal(Literal::Boolean(_)) => Some(Sort::Bool),
        PredicateExpr::Literal(Literal::Number(_)) => Some(Sort::Number),
        _ => None,
    }
}

fn predicate_binary(operator: &BinaryOp, left: Term, right: Term) -> Result<Term, String> {
    let left_sort = left.sort();
    let right_sort = right.sort();
    Ok(match operator {
        BinaryOp::EqEqEq | BinaryOp::EqEq if left_sort == right_sort => {
            Term::Eq(Box::new(left), Box::new(right))
        }
        BinaryOp::NotEqEq | BinaryOp::NotEq if left_sort == right_sort => {
            Term::Ne(Box::new(left), Box::new(right))
        }
        BinaryOp::Gt if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Gt(Box::new(left), Box::new(right))
        }
        BinaryOp::Lt if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Lt(Box::new(left), Box::new(right))
        }
        BinaryOp::Gte if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Ge(Box::new(left), Box::new(right))
        }
        BinaryOp::Lte if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Le(Box::new(left), Box::new(right))
        }
        BinaryOp::Add if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Add(Box::new(left), Box::new(right))
        }
        BinaryOp::Sub if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Sub(Box::new(left), Box::new(right))
        }
        BinaryOp::Mul if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Mul(Box::new(left), Box::new(right))
        }
        BinaryOp::Div => {
            return Err("Division is outside the integer refinement subset".into());
        }
        _ => return Err("Ill-typed refinement predicate".into()),
    })
}

fn binary_term(operator: BinaryOperator, left: Term, right: Term) -> Result<Term, String> {
    let left_sort = left.sort();
    let right_sort = right.sort();
    Ok(match operator {
        BinaryOperator::Equality | BinaryOperator::StrictEquality if left_sort == right_sort => {
            Term::Eq(Box::new(left), Box::new(right))
        }
        BinaryOperator::Inequality | BinaryOperator::StrictInequality
            if left_sort == right_sort =>
        {
            Term::Ne(Box::new(left), Box::new(right))
        }
        BinaryOperator::GreaterThan if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Gt(Box::new(left), Box::new(right))
        }
        BinaryOperator::LessThan if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Lt(Box::new(left), Box::new(right))
        }
        BinaryOperator::GreaterEqualThan
            if left_sort == Sort::Number && right_sort == Sort::Number =>
        {
            Term::Ge(Box::new(left), Box::new(right))
        }
        BinaryOperator::LessEqualThan
            if left_sort == Sort::Number && right_sort == Sort::Number =>
        {
            Term::Le(Box::new(left), Box::new(right))
        }
        BinaryOperator::Addition if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Add(Box::new(left), Box::new(right))
        }
        BinaryOperator::Subtraction if left_sort == Sort::Number && right_sort == Sort::Number => {
            Term::Sub(Box::new(left), Box::new(right))
        }
        BinaryOperator::Multiplication
            if left_sort == Sort::Number && right_sort == Sort::Number =>
        {
            Term::Mul(Box::new(left), Box::new(right))
        }
        BinaryOperator::Division => {
            return Err("JavaScript division is outside the integer refinement subset".into());
        }
        _ => {
            return Err(format!(
                "Unsupported or ill-typed binary operator '{}'",
                operator.as_str()
            ));
        }
    })
}

fn substitute(term: &Term, target: &Term, replacement: &Term) -> Term {
    if term == target {
        return replacement.clone();
    }
    match term {
        Term::Add(left, right) => Term::Add(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Sub(left, right) => Term::Sub(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Mul(left, right) => Term::Mul(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Same(left, right) => Term::Same(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Eq(left, right) => Term::Eq(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Ne(left, right) => Term::Ne(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Gt(left, right) => Term::Gt(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Lt(left, right) => Term::Lt(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Ge(left, right) => Term::Ge(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Le(left, right) => Term::Le(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::And(left, right) => Term::And(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Or(left, right) => Term::Or(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Not(inner) => Term::Not(Box::new(substitute(inner, target, replacement))),
        Term::Pred(name, argument) => Term::Pred(
            name.clone(),
            Box::new(substitute(argument, target, replacement)),
        ),
        other => other.clone(),
    }
}

fn contains_term(term: &Term, needle: &Term) -> bool {
    if term == needle {
        return true;
    }
    match term {
        Term::Add(left, right)
        | Term::Sub(left, right)
        | Term::Mul(left, right)
        | Term::Same(left, right)
        | Term::Eq(left, right)
        | Term::Ne(left, right)
        | Term::Gt(left, right)
        | Term::Lt(left, right)
        | Term::Ge(left, right)
        | Term::Le(left, right)
        | Term::And(left, right)
        | Term::Or(left, right) => contains_term(left, needle) || contains_term(right, needle),
        Term::Not(inner) | Term::Pred(_, inner) => contains_term(inner, needle),
        Term::Number(_) | Term::Bool(_) | Term::Var(_, _) => false,
    }
}

fn constraint_variables(constraint: &FixpointConstraint<'_>) -> Vec<(String, Sort)> {
    let mut variables = std::collections::BTreeMap::new();
    for term in constraint
        .assumptions
        .iter()
        .chain(std::iter::once(constraint.consequent))
    {
        collect_variables(term, &mut variables);
    }
    variables.into_iter().collect()
}

fn collect_variables(term: &Term, output: &mut std::collections::BTreeMap<String, Sort>) {
    match term {
        Term::Var(name, sort) => {
            output.insert(name.clone(), *sort);
        }
        Term::Add(left, right)
        | Term::Sub(left, right)
        | Term::Mul(left, right)
        | Term::Same(left, right)
        | Term::Eq(left, right)
        | Term::Ne(left, right)
        | Term::Gt(left, right)
        | Term::Lt(left, right)
        | Term::Ge(left, right)
        | Term::Le(left, right)
        | Term::And(left, right)
        | Term::Or(left, right) => {
            collect_variables(left, output);
            collect_variables(right, output);
        }
        Term::Not(inner) | Term::Pred(_, inner) => collect_variables(inner, output),
        Term::Number(_) | Term::Bool(_) => {}
    }
}

enum ZTerm {
    Number(Float),
    Bool(Bool),
}

fn to_z3(term: &Term) -> Result<ZTerm, String> {
    match term {
        Term::Number(value) => Ok(ZTerm::Number(Float::from_f64(*value as f64))),
        Term::Bool(value) => Ok(ZTerm::Bool(Bool::from_bool(*value))),
        Term::Var(name, Sort::Number) => Ok(ZTerm::Number(Float::new_const_double(name.as_str()))),
        Term::Var(name, Sort::Bool) => Ok(ZTerm::Bool(Bool::new_const(name.as_str()))),
        Term::Pred(name, argument) => match to_z3(argument)? {
            ZTerm::Number(argument) => {
                let domain = Z3Sort::double();
                let range = Z3Sort::bool();
                let predicate = FuncDecl::new(name.as_str(), &[&domain], &range);
                Ok(ZTerm::Bool(
                    predicate.apply(&[&argument]).as_bool().unwrap(),
                ))
            }
            ZTerm::Bool(argument) => {
                let domain = Z3Sort::bool();
                let range = Z3Sort::bool();
                let predicate = FuncDecl::new(name.as_str(), &[&domain], &range);
                Ok(ZTerm::Bool(
                    predicate.apply(&[&argument]).as_bool().unwrap(),
                ))
            }
        },
        Term::Add(left, right) => number_pair(left, right, |a, b| {
            a.add_with_rounding_mode(b, &RoundingMode::round_nearest_ties_to_even())
        }),
        Term::Sub(left, right) => number_pair(left, right, |a, b| {
            a.sub_with_rounding_mode(b, &RoundingMode::round_nearest_ties_to_even())
        }),
        Term::Mul(left, right) => number_pair(left, right, |a, b| {
            a.mul_with_rounding_mode(b, &RoundingMode::round_nearest_ties_to_even())
        }),
        Term::Same(left, right) => structural_equality(left, right),
        Term::Eq(left, right) => equality(left, right, false),
        Term::Ne(left, right) => equality(left, right, true),
        Term::Gt(left, right) => number_compare(left, right, |a, b| a.gt(b)),
        Term::Lt(left, right) => number_compare(left, right, |a, b| a.lt(b)),
        Term::Ge(left, right) => number_compare(left, right, |a, b| a.ge(b)),
        Term::Le(left, right) => number_compare(left, right, |a, b| a.le(b)),
        Term::And(left, right) => bool_pair(left, right, |a, b| Bool::and(&[a, b])),
        Term::Or(left, right) => bool_pair(left, right, |a, b| Bool::or(&[a, b])),
        Term::Not(inner) => match to_z3(inner)? {
            ZTerm::Bool(value) => Ok(ZTerm::Bool(value.not())),
            _ => Err("Logical not requires a boolean".into()),
        },
    }
}

fn number_pair(
    left: &Term,
    right: &Term,
    operation: impl FnOnce(Float, Float) -> Float,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => Ok(ZTerm::Number(operation(left, right))),
        _ => Err("Arithmetic requires number operands".into()),
    }
}

fn number_compare(
    left: &Term,
    right: &Term,
    operation: impl FnOnce(&Float, &Float) -> Bool,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => Ok(ZTerm::Bool(operation(&left, &right))),
        _ => Err("Ordered comparison requires number operands".into()),
    }
}

fn bool_pair(
    left: &Term,
    right: &Term,
    operation: impl FnOnce(&Bool, &Bool) -> Bool,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Bool(left), ZTerm::Bool(right)) => Ok(ZTerm::Bool(operation(&left, &right))),
        _ => Err("Logical operator requires boolean operands".into()),
    }
}

fn equality(left: &Term, right: &Term, negate: bool) -> Result<ZTerm, String> {
    let equality = match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => left.eq_fpa(right),
        (ZTerm::Bool(left), ZTerm::Bool(right)) => left.eq(right),
        _ => return Err("Equality operands have different base types".into()),
    };
    Ok(ZTerm::Bool(if negate { equality.not() } else { equality }))
}

fn structural_equality(left: &Term, right: &Term) -> Result<ZTerm, String> {
    let equality = match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => left.eq(right),
        (ZTerm::Bool(left), ZTerm::Bool(right)) => left.eq(right),
        _ => return Err("SSA equality operands have different base types".into()),
    };
    Ok(ZTerm::Bool(equality))
}

fn expression_name(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.name.to_string()),
        Expression::StaticMemberExpression(member) => expression_name(&member.object)
            .map(|object| format!("{object}.{}", member.property.name)),
        Expression::ParenthesizedExpression(parenthesized) => {
            expression_name(&parenthesized.expression)
        }
        _ => None,
    }
}

fn expression_root_name(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.name.to_string()),
        Expression::StaticMemberExpression(member) => expression_root_name(&member.object),
        Expression::ParenthesizedExpression(parenthesized) => {
            expression_root_name(&parenthesized.expression)
        }
        _ => None,
    }
}

fn contains_predicate(predicate: Option<&PredicateExpr>, name: &str) -> bool {
    match predicate {
        Some(PredicateExpr::PredicateApply(found, argument)) => {
            found == name || contains_predicate(Some(argument), name)
        }
        Some(PredicateExpr::Not(inner)) => contains_predicate(Some(inner), name),
        Some(PredicateExpr::Binary(_, left, right))
        | Some(PredicateExpr::Logical(_, left, right)) => {
            contains_predicate(Some(left), name) || contains_predicate(Some(right), name)
        }
        _ => false,
    }
}

fn validate_predicate_parameter_domains(
    contract: &Contract,
    replacements: &HashMap<String, Term>,
) -> Result<(), String> {
    let mut domains = HashMap::new();
    for predicate in contract
        .params
        .iter()
        .filter_map(|(_, ty)| ty.predicate.as_ref())
        .chain(contract.ret.predicate.iter())
    {
        collect_predicate_parameter_domains(predicate, replacements, &mut domains)?;
    }
    Ok(())
}

fn collect_predicate_parameter_domains(
    predicate: &PredicateExpr,
    replacements: &HashMap<String, Term>,
    domains: &mut HashMap<String, Sort>,
) -> Result<(), String> {
    match predicate {
        PredicateExpr::PredicateApply(name, argument) => {
            let argument = predicate_term(argument, replacements, &HashMap::new(), None)?;
            let sort = argument.sort();
            if domains
                .insert(name.clone(), sort)
                .is_some_and(|found| found != sort)
            {
                return Err(format!(
                    "Predicate parameter '{name}' is applied to incompatible base types"
                ));
            }
            collect_predicate_parameter_domains(argument_expr(predicate), replacements, domains)
        }
        PredicateExpr::Not(inner) => {
            collect_predicate_parameter_domains(inner, replacements, domains)
        }
        PredicateExpr::Binary(_, left, right) | PredicateExpr::Logical(_, left, right) => {
            collect_predicate_parameter_domains(left, replacements, domains)?;
            collect_predicate_parameter_domains(right, replacements, domains)
        }
        PredicateExpr::Identifier(_)
        | PredicateExpr::Member(_, _)
        | PredicateExpr::Literal(_)
        | PredicateExpr::Return => Ok(()),
    }
}

fn argument_expr(predicate: &PredicateExpr) -> &PredicateExpr {
    let PredicateExpr::PredicateApply(_, argument) = predicate else {
        unreachable!()
    };
    argument
}

fn sort_for_base(base: &BaseType) -> Sort {
    match base {
        BaseType::Primitive(name) if name == "boolean" => Sort::Bool,
        _ => Sort::Number,
    }
}

fn bases_compatible(actual: &BaseType, expected: &BaseType) -> bool {
    match expected {
        BaseType::Primitive(name) if name == "any" || name == "unknown" => true,
        _ => actual == expected,
    }
}

fn base_for_sort(sort: Sort) -> BaseType {
    match sort {
        Sort::Number => number_type(),
        Sort::Bool => boolean_type(),
    }
}

fn value_sort(value: &Value) -> Option<Sort> {
    match &value.base {
        BaseType::Primitive(name) if name == "number" => Some(Sort::Number),
        BaseType::Primitive(name) if name == "boolean" => Some(Sort::Bool),
        _ => None,
    }
}

fn is_reserved_runtime_root(name: &str) -> bool {
    matches!(name, "__rt" | "Math" | "Number" | "Array" | "console")
}

fn number_type() -> BaseType {
    BaseType::Primitive("number".into())
}

fn boolean_type() -> BaseType {
    BaseType::Primitive("boolean".into())
}

fn is_void(base: &BaseType) -> bool {
    matches!(base, BaseType::Primitive(name) if name == "void")
}
