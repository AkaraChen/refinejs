use crate::syntax::*;
use oxc_allocator::Allocator;
use oxc_ast::ast::{BindingPattern, Expression, FormalParameter, Function, Program, ReturnStatement, Statement, VariableDeclaration};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use std::collections::HashMap;

const RUNTIME_IDENT: &str = "__rt";
const RETURN_TEMP: &str = "__rt_return";
const VALUE_TEMP: &str = "__rt_v";

#[derive(Debug, Clone)]
struct Replacement {
    start: usize,
    end: usize,
    text: String,
}

pub fn transpile(source: &str, annotations: &[Annotation]) -> Result<String, String> {
    let allocator = Allocator::default();
    let source_type = SourceType::default().with_module(true);
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(oxc_parser::ParseOptions::default())
        .parse();

    if !ret.diagnostics.is_empty() {
        return Err(format!("Parse errors: {:?}", ret.diagnostics));
    }

    let mut collector = TranspileCollector::new(source, annotations);
    collector.visit_program(&ret.program);

    // Sort replacements by start descending so positions remain valid.
    collector.replacements.sort_by(|a, b| b.start.cmp(&a.start));

    let mut result = source.to_string();
    for repl in collector.replacements {
        result.replace_range(repl.start..repl.end, &repl.text);
    }

    Ok(result)
}

struct TranspileCollector<'s> {
    source: &'s str,
    by_function: HashMap<String, Vec<&'s Annotation>>,
    by_variable: HashMap<String, Vec<&'s Annotation>>,
    current_function: Vec<String>,
    replacements: Vec<Replacement>,
}

impl<'s> TranspileCollector<'s> {
    fn new(source: &'s str, annotations: &'s [Annotation]) -> Self {
        let mut by_function: HashMap<String, Vec<&Annotation>> = HashMap::new();
        let mut by_variable: HashMap<String, Vec<&Annotation>> = HashMap::new();
        for a in annotations {
            match &a.target {
                AnnotationTarget::Param { function_name, .. } | AnnotationTarget::Return { function_name } => {
                    by_function.entry(function_name.clone()).or_default().push(a);
                }
                AnnotationTarget::Variable { name } => {
                    by_variable.entry(name.clone()).or_default().push(a);
                }
            }
        }
        Self {
            source,
            by_function,
            by_variable,
            current_function: Vec::new(),
            replacements: Vec::new(),
        }
    }

    fn function_name(func: &Function) -> String {
        func.id.as_ref().map(|id| id.name.to_string()).unwrap_or_else(|| "<anonymous>".into())
    }

    fn param_name(param: &FormalParameter) -> Option<String> {
        match &param.pattern {
            BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
            BindingPattern::AssignmentPattern(ap) => match &ap.left {
                BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
                _ => None,
            },
            _ => None,
        }
    }

    fn source_text(&self, span: Span) -> &str {
        &self.source[span.start as usize..span.end as usize]
    }

    fn predicate_to_js(&self, pred: &PredicateExpr, self_name: &str) -> String {
        match pred {
            PredicateExpr::Literal(lit) => match lit {
                Literal::Number(n) => n.to_string(),
                Literal::String(s) => format!("\"{}\"", s),
                Literal::Boolean(b) => b.to_string(),
            },
            PredicateExpr::Identifier(name) => name.clone(),
            PredicateExpr::Member(obj, prop) => format!("{}.{}", obj, prop),
            PredicateExpr::Return => RETURN_TEMP.to_string(),
            PredicateExpr::Not(expr) => format!("!({})", self.predicate_to_js(expr, self_name)),
            PredicateExpr::Logical(op, left, right) => {
                let op_str = match op {
                    LogicalOp::And => "&&",
                    LogicalOp::Or => "||",
                };
                format!(
                    "({} {} {})",
                    self.predicate_to_js(left, self_name),
                    op_str,
                    self.predicate_to_js(right, self_name)
                )
            }
            PredicateExpr::Binary(op, left, right) => {
                let op_str = match op {
                    BinaryOp::EqEqEq => "===",
                    BinaryOp::NotEqEq => "!==",
                    BinaryOp::EqEq => "==",
                    BinaryOp::NotEq => "!=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                };
                format!(
                    "({} {} {})",
                    self.predicate_to_js(left, self_name),
                    op_str,
                    self.predicate_to_js(right, self_name)
                )
            }
        }
    }

    fn predicate_to_string(&self, pred: &PredicateExpr) -> String {
        match pred {
            PredicateExpr::Literal(lit) => match lit {
                Literal::Number(n) => n.to_string(),
                Literal::String(s) => format!("\"{}\"", s),
                Literal::Boolean(b) => b.to_string(),
            },
            PredicateExpr::Identifier(name) => name.clone(),
            PredicateExpr::Member(obj, prop) => format!("{}.{}", obj, prop),
            PredicateExpr::Return => "$".to_string(),
            PredicateExpr::Not(expr) => format!("!({})", self.predicate_to_string(expr)),
            PredicateExpr::Logical(op, left, right) => {
                let op_str = match op {
                    LogicalOp::And => "&&",
                    LogicalOp::Or => "||",
                };
                format!(
                    "({} {} {})",
                    self.predicate_to_string(left),
                    op_str,
                    self.predicate_to_string(right)
                )
            }
            PredicateExpr::Binary(op, left, right) => {
                let op_str = match op {
                    BinaryOp::EqEqEq => "===",
                    BinaryOp::NotEqEq => "!==",
                    BinaryOp::EqEq => "==",
                    BinaryOp::NotEq => "!=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                };
                format!(
                    "({} {} {})",
                    self.predicate_to_string(left),
                    op_str,
                    self.predicate_to_string(right)
                )
            }
        }
    }

    fn build_param_assert(&self, function_name: &str, param_name: &str, predicate: &PredicateExpr) -> String {
        let expr = self.predicate_to_js(predicate, param_name);
        let msg = format!(
            "{} parameter '{}' violates refinement: {}",
            function_name,
            param_name,
            self.predicate_to_string(predicate)
        );
        format!(
            "{}.assert({}, \"{}\", {{ {}: {} }});",
            RUNTIME_IDENT, expr, msg, param_name, param_name
        )
    }

    fn build_return_assert(&self, function_name: &str, predicate: &PredicateExpr) -> String {
        let expr = self.predicate_to_js(predicate, "$");
        let msg = format!(
            "{} return value violates refinement: {}",
            function_name,
            self.predicate_to_string(predicate)
        );
        format!(
            "{}.assert({}, \"{}\", {{ value: {} }});",
            RUNTIME_IDENT, expr, msg, RETURN_TEMP
        )
    }

    fn build_variable_assert(&self, name: &str, predicate: &PredicateExpr, value_name: &str) -> String {
        let renamed = rename_identifier(predicate, name, value_name);
        let expr = self.predicate_to_js(&renamed, value_name);
        let msg = format!(
            "variable '{}' violates refinement: {}",
            name,
            self.predicate_to_string(predicate)
        );
        format!(
            "{}.assert({}, \"{}\", {{ value: {} }});",
            RUNTIME_IDENT, expr, msg, value_name
        )
    }

    fn process_function(&mut self, func: &Function<'s>) {
        let name = Self::function_name(func);
        let anns = match self.by_function.get(&name) {
            Some(a) => a.clone(),
            None => return,
        };

        // Param assertions at body start.
        if let Some(body) = &func.body {
            let body_start = body.span.start as usize + 1; // after '{'
            let mut asserts: Vec<String> = Vec::new();
            for a in &anns {
                if let AnnotationTarget::Param { param_name, index, .. } = &a.target {
                    if let Some(pred) = &a.ty.predicate {
                        if let Some(param) = func.params.items.get(*index) {
                            if Self::param_name(param).as_deref() == Some(param_name) {
                                asserts.push(self.build_param_assert(&name, param_name, pred));
                            }
                        }
                    }
                }
            }
            if !asserts.is_empty() {
                let text = format!("\n  {}\n", asserts.join("\n  "));
                self.replacements.push(Replacement {
                    start: body_start,
                    end: body_start,
                    text,
                });
            }
        }

        // Enter function scope for return wrapping.
        self.current_function.push(name.clone());

        if let Some(body) = &func.body {
            for stmt in &body.statements {
                self.visit_statement(stmt);
            }
        }

        self.current_function.pop();
    }

    fn process_return(&mut self, ret: &ReturnStatement) {
        let function_name = match self.current_function.last() {
            Some(n) => n.clone(),
            None => return,
        };

        let anns = match self.by_function.get(&function_name) {
            Some(a) => a.clone(),
            None => return,
        };

        let return_predicates: Vec<&PredicateExpr> = anns
            .iter()
            .filter_map(|a| match &a.target {
                AnnotationTarget::Return { .. } => a.ty.predicate.as_ref(),
                _ => None,
            })
            .collect();

        if return_predicates.is_empty() {
            return;
        }

        let arg = match &ret.argument {
            Some(arg) => arg,
            None => return,
        };

        // Skip if this return already returns our temp.
        if let Some(name) = expression_identifier(arg) {
            if name == RETURN_TEMP {
                return;
            }
        }

        let arg_text = self.source_text(arg.span());
        let asserts = return_predicates
            .iter()
            .map(|p| self.build_return_assert(&function_name, p))
            .collect::<Vec<_>>()
            .join("\n  ");

        let replacement = format!(
            "{{\n  const {} = {};\n  {}\n  return {};\n}}",
            RETURN_TEMP, arg_text, asserts, RETURN_TEMP
        );

        self.replacements.push(Replacement {
            start: ret.span.start as usize,
            end: ret.span.end as usize,
            text: replacement,
        });
    }

    fn process_variable_declaration(&mut self, decl: &VariableDeclaration) {
        for d in &decl.declarations {
            let name = match &d.id {
                BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                _ => continue,
            };
            let anns = match self.by_variable.get(&name) {
                Some(a) => a.clone(),
                None => continue,
            };
            let predicate = anns.iter().find_map(|a| a.ty.predicate.as_ref());
            let Some(predicate) = predicate else { continue };
            let Some(init) = &d.init else { continue };

            let init_text = self.source_text(init.span());
            let assert_text = self.build_variable_assert(&name, predicate, VALUE_TEMP);
            let replacement = format!(
                "{} = (() => {{\n  const {} = {};\n  {}\n  return {};\n}})()",
                name, VALUE_TEMP, init_text, assert_text, VALUE_TEMP
            );

            // Replace from start of declarator id to end of init.
            let start = d.id.span().start as usize;
            let end = init.span().end as usize;
            self.replacements.push(Replacement {
                start,
                end,
                text: replacement,
            });
        }
    }
}

fn rename_identifier(pred: &PredicateExpr, from: &str, to: &str) -> PredicateExpr {
    match pred {
        PredicateExpr::Identifier(name) => {
            PredicateExpr::Identifier(if name == from { to.to_string() } else { name.clone() })
        }
        PredicateExpr::Member(obj, prop) => {
            PredicateExpr::Member(if obj == from { to.to_string() } else { obj.clone() }, prop.clone())
        }
        PredicateExpr::Not(expr) => {
            PredicateExpr::Not(Box::new(rename_identifier(expr, from, to)))
        }
        PredicateExpr::Logical(op, left, right) => {
            PredicateExpr::Logical(op.clone(), Box::new(rename_identifier(left, from, to)), Box::new(rename_identifier(right, from, to)))
        }
        PredicateExpr::Binary(op, left, right) => {
            PredicateExpr::Binary(op.clone(), Box::new(rename_identifier(left, from, to)), Box::new(rename_identifier(right, from, to)))
        }
        other => other.clone(),
    }
}

fn expression_identifier<'a>(expr: &'a Expression<'a>) -> Option<&'a str> {
    match expr {
        Expression::Identifier(id) => Some(id.name.as_str()),
        _ => None,
    }
}

impl<'s> Visit<'s> for TranspileCollector<'s> {
    fn visit_program(&mut self, program: &Program<'s>) {
        for stmt in &program.body {
            self.visit_statement(stmt);
        }
    }

    fn visit_statement(&mut self, stmt: &Statement<'s>) {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                self.process_function(func);
            }
            Statement::VariableDeclaration(decl) => {
                self.process_variable_declaration(decl);
                for d in &decl.declarations {
                    if let Some(init) = &d.init {
                        self.visit_expression(init);
                    }
                }
            }
            _ => oxc_ast_visit::walk::walk_statement(self, stmt),
        }
    }

    fn visit_return_statement(&mut self, ret: &ReturnStatement<'s>) {
        self.process_return(ret);
        if let Some(arg) = &ret.argument {
            self.visit_expression(arg);
        }
    }
}
