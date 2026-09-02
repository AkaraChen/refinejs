use refinejs::{checker, parser, prelude, runtime, syntax::Annotation, transpiler};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn flux_fixtures(suffix: &str) -> Vec<PathBuf> {
    let fixtures_dir = fixture_path("");
    let mut paths: Vec<_> = fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixtures_dir.display()))
        .map(|entry| {
            entry
                .expect("failed to read fixture directory entry")
                .path()
        })
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("flux_") && name.ends_with(suffix))
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no flux fixtures matched *{suffix}");
    paths
}

fn parse_with_prelude(path: &Path) -> (String, Vec<Annotation>) {
    let file_name = path.display().to_string();
    let source = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {file_name}: {error}"));
    let mut parsed = parser::parse_file(&source, &file_name)
        .unwrap_or_else(|error| panic!("failed to parse {file_name}: {error}"));
    prelude::merge_prelude(&mut parsed.annotations);
    (source, parsed.annotations)
}

fn assert_statically_valid_and_runs(path: &Path) {
    let file_name = path.display().to_string();
    let (source, annotations) = parse_with_prelude(path);

    let errors = checker::check_source(&source, &file_name, &annotations);
    assert!(
        errors.is_empty(),
        "expected {file_name} to verify statically, got:\n{errors:#?}"
    );

    let transformed = transpiler::transpile(&source, &annotations)
        .unwrap_or_else(|error| panic!("failed to transpile {file_name}: {error}"));
    assert!(
        transformed.contains("__rt.assert"),
        "transpiled {file_name} did not preserve runtime refinement assertions"
    );

    let executable = format!("{}\n\n{transformed}", runtime::runtime_block());
    let output = Command::new("node")
        .args(["-e", &executable])
        .output()
        .unwrap_or_else(|error| panic!("failed to execute Node.js for {file_name}: {error}"));
    assert!(
        output.status.success(),
        "Node.js execution failed for {file_name}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn flux_positive_fixtures_verify_and_run_without_assertion_failures() {
    for path in flux_fixtures("_positive.js") {
        assert_statically_valid_and_runs(&path);
    }
}

#[test]
fn flux_negative_fixtures_are_rejected_statically() {
    for path in flux_fixtures("_negative.js") {
        let file_name = path.display().to_string();
        let (source, annotations) = parse_with_prelude(&path);
        let errors = checker::check_source(&source, &file_name, &annotations);
        assert!(
            !errors.is_empty(),
            "expected {file_name} to be rejected statically"
        );
        assert!(
            errors
                .iter()
                .all(|error| !error.message.contains("Z3 returned unknown")),
            "{file_name} was rejected only because the solver returned unknown: {errors:#?}"
        );
    }
}

#[test]
fn soundness_regressions_have_definite_diagnostics() {
    let cases = [
        ("flux_polymorphism_vacuity_negative.js", "Return value"),
        ("flux_polymorphism_body_negative.js", "Return value"),
        ("flux_float_rounding_negative.js", "Return value"),
        ("flux_predicate_kind_negative.js", "incompatible base types"),
        (
            "flux_parameter_shadow_negative.js",
            "shadows refined parameter",
        ),
        (
            "flux_uninitialized_local_negative.js",
            "requires an initializer",
        ),
        (
            "flux_destructuring_negative.js",
            "Destructuring declarations",
        ),
        ("flux_var_declaration_negative.js", "Only let and const"),
        ("flux_const_assignment_negative.js", "immutable binding"),
        (
            "flux_callee_shadow_negative.js",
            "shadows a refined function signature",
        ),
        ("flux_async_function_negative.js", "Async and generator"),
        ("flux_generator_function_negative.js", "Async and generator"),
        ("flux_ill_typed_predicate_negative.js", "boolean operands"),
        ("flux_void_value_negative.js", "boolean operands"),
        ("flux_runtime_binding_negative.js", "reserved"),
        ("flux_default_parameter_negative.js", "Default parameters"),
        (
            "flux_nested_variable_annotation_negative.js",
            "outside a statically checked scope",
        ),
        (
            "flux_orphan_parameter_annotation_negative.js",
            "requires a function signature",
        ),
        ("flux_unused_predicate_negative.js", "must occur"),
        ("flux_tdz_negative.js", "before its declaration"),
        ("flux_console_spread_negative.js", "Spread arguments"),
        ("flux_prelude_shadow_negative.js", "reserved"),
    ];

    for (fixture, expected) in cases {
        let path = fixture_path(fixture);
        let file_name = path.display().to_string();
        let (source, annotations) = parse_with_prelude(&path);
        let errors = checker::check_source(&source, &file_name, &annotations);
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected {file_name} to report {expected:?}, got {errors:#?}"
        );
    }
}

#[test]
fn transpiler_preserves_parameter_return_and_variable_assertions_hygienically() {
    let core_path = fixture_path("flux_core_positive.js");
    let (core_source, core_annotations) = parse_with_prelude(&core_path);
    let core_output = transpiler::transpile(&core_source, &core_annotations).unwrap();
    assert_eq!(core_output.matches("__rt.assert").count(), 10);
    assert!(core_output.contains("parameter"));
    assert!(core_output.contains("return value"));
    assert!(core_output.contains("variable"));

    let hygiene_path = fixture_path("flux_hygiene_positive.js");
    let (hygiene_source, hygiene_annotations) = parse_with_prelude(&hygiene_path);
    let hygiene_output = transpiler::transpile(&hygiene_source, &hygiene_annotations).unwrap();
    assert_eq!(hygiene_output.matches("__rt.assert").count(), 3);
    assert!(hygiene_output.contains("__rt_return_1"));
    assert!(hygiene_output.contains("__rt_v_1"));

    let unicode_path = fixture_path("flux_unicode_hygiene_positive.js");
    let (unicode_source, unicode_annotations) = parse_with_prelude(&unicode_path);
    let unicode_output = transpiler::transpile(&unicode_source, &unicode_annotations).unwrap();
    assert_eq!(unicode_output.matches("__rt.assert").count(), 2);
    assert!(unicode_output.contains("__rt_return_1"));
    assert!(unicode_output.contains("__rt_v_1"));
}

#[test]
fn existing_sqrt_fixture_verifies_and_runs() {
    assert_statically_valid_and_runs(&fixture_path("sqrt.js"));
}
