use refinejs::{checker, parser, prelude, runtime, transpiler};

use std::env;
use std::fs;
use std::process;

fn run_check(input_path: &str) -> bool {
    let source = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {}: {}", input_path, e);
            return false;
        }
    };

    let mut result = match parser::parse_file(&source, input_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return false;
        }
    };

    prelude::merge_prelude(&mut result.annotations);

    let errors = checker::check_source(&source, input_path, &result.annotations);
    if !errors.is_empty() {
        for e in errors {
            let loc = e
                .loc
                .as_ref()
                .map(|l| {
                    format!(
                        "{}:{}:{}: ",
                        l.file.as_deref().unwrap_or(""),
                        l.line,
                        l.column
                    )
                })
                .unwrap_or_default();
            eprintln!("{}{}", loc, e.message);
        }
        return false;
    }

    println!("No refinement errors found.");
    true
}

fn run_build(input_path: &str, output_path: &str) -> bool {
    let source = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {}: {}", input_path, e);
            return false;
        }
    };

    let mut result = match parser::parse_file(&source, input_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return false;
        }
    };

    prelude::merge_prelude(&mut result.annotations);

    let errors = checker::check_source(&source, input_path, &result.annotations);
    if !errors.is_empty() {
        for e in errors {
            let loc = e
                .loc
                .as_ref()
                .map(|l| {
                    format!(
                        "{}:{}:{}: ",
                        l.file.as_deref().unwrap_or(""),
                        l.line,
                        l.column
                    )
                })
                .unwrap_or_default();
            eprintln!("{}{}", loc, e.message);
        }
        return false;
    }

    let transformed = match transpiler::transpile(&source, &result.annotations) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Transpile error: {}", e);
            return false;
        }
    };

    let output = format!("{}\n\n{}", runtime::runtime_block(), transformed);
    if let Err(e) = fs::write(output_path, output) {
        eprintln!("Failed to write {}: {}", output_path, e);
        return false;
    }

    println!("Wrote {}", output_path);
    true
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: refinejs <check|build> [args]");
        process::exit(1);
    }

    match args[1].as_str() {
        "check" => {
            if args.len() < 3 {
                eprintln!("Usage: refinejs check <file>");
                process::exit(1);
            }
            if !run_check(&args[2]) {
                process::exit(1);
            }
        }
        "build" => {
            if args.len() < 4 {
                eprintln!("Usage: refinejs build <input> <output>");
                process::exit(1);
            }
            if !run_build(&args[2], &args[3]) {
                process::exit(1);
            }
        }
        _ => {
            eprintln!("Unknown command. Use 'check' or 'build'.");
            process::exit(1);
        }
    }
}
