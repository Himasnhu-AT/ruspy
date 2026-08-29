//! `ruspy` CLI — lex + parse + type-check + run a `.ruspy` file.
//!
//! A thin binary over the `ruspy` library crate (so lib-only APIs aren't reported
//! as dead code here).

use clap::{ArgAction, Parser as ClapParser};
use ruspy::diagnostics::LineIndex;
use ruspy::interpreter::{DefaultHost, Interpreter};
use std::fs;
use std::process;

#[derive(ClapParser)]
#[command(version, about = "Ruspy — a small compiled language (interpreter front end)")]
struct Cli {
    /// Print the parsed AST before running.
    #[arg(short = 'd', long = "debug", action = ArgAction::SetTrue)]
    debug: bool,

    /// Type-check only; do not run.
    #[arg(short = 'c', long = "check", action = ArgAction::SetTrue)]
    check_only: bool,

    /// Execute via the bytecode VM (falls back to the interpreter for features the
    /// VM does not yet compile — arrays, structs).
    #[arg(long = "vm", action = ArgAction::SetTrue)]
    vm: bool,

    /// Ahead-of-time compile to a standalone native executable at this path,
    /// instead of running. Requires a build with `--features jit`.
    #[arg(long = "build", value_name = "OUT")]
    build: Option<String>,

    /// Entry function for `--build` (a zero-arg function returning a number).
    #[arg(long = "entry", default_value = "main")]
    entry: String,

    /// Compile to a protected `.ruspyc` artifact at this path (needs `--key`).
    #[arg(long = "pack", value_name = "OUT")]
    pack: Option<String>,

    /// Run a protected `.ruspyc` artifact (the input `file`) instead of source.
    #[arg(long = "run-protected", action = ArgAction::SetTrue)]
    run_protected: bool,

    /// 64 hex chars (32 bytes) — the key for `--pack` / `--run-protected`.
    #[arg(long = "key", value_name = "HEX")]
    key: Option<String>,

    /// Source file to run.
    file: String,
}

/// Parse a 64-hex-character string into a 32-byte key. No fake key-derivation:
/// the caller supplies the raw key (in production it comes from activation).
fn parse_key(hex: &Option<String>) -> [u8; 32] {
    let hex = hex.as_deref().unwrap_or_else(|| {
        eprintln!("error: --key <64 hex chars> is required for this operation");
        process::exit(1);
    });
    let hex = hex.trim();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        eprintln!("error: --key must be exactly 64 hex characters (32 bytes)");
        process::exit(1);
    }
    let mut key = [0u8; 32];
    for i in 0..32 {
        key[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    key
}

fn main() {
    let cli = Cli::parse();

    // Running a protected artifact reads raw bytes, not source — handle it first.
    if cli.run_protected {
        let key = parse_key(&cli.key);
        let bytes = match fs::read(&cli.file) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error: cannot read `{}`: {e}", cli.file);
                process::exit(1);
            }
        };
        match ruspy::vm::protect::run_protected(&bytes, &key) {
            Ok(Ok(lines)) => {
                for line in lines {
                    println!("{line}");
                }
            }
            Ok(Err(diags)) => {
                for d in &diags {
                    eprintln!("runtime error: {d}");
                }
                process::exit(1);
            }
            Err(e) => {
                eprintln!("error: {}", e.0);
                process::exit(1);
            }
        }
        return;
    }

    // Resolve `import "file.ruspy";` into one program — each file is parsed AND
    // type-checked against its own source, so diagnostics carry the right
    // file:line:col. A LineIndex of the root source labels runtime errors.
    let root_src = fs::read_to_string(&cli.file).unwrap_or_default();
    let li = LineIndex::new(&root_src);
    let program = match ruspy::loader::load_file(std::path::Path::new(&cli.file)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    };

    if cli.debug {
        eprintln!("{program:#?}");
    }

    // The loader already parsed + type-checked every file.
    if cli.check_only {
        return;
    }

    // Compile to a protected `.ruspyc` artifact and stop.
    if let Some(out) = &cli.pack {
        let key = parse_key(&cli.key);
        match ruspy::vm::protect::compile_and_pack(&program, &key) {
            Ok(bytes) => {
                if let Err(e) = fs::write(out, &bytes) {
                    eprintln!("error: writing `{out}`: {e}");
                    process::exit(1);
                }
                eprintln!("packed protected bytecode -> {out} ({} bytes)", bytes.len());
            }
            Err(e) => {
                eprintln!("error: pack failed: {}", e.0);
                eprintln!("       (the protected format wraps VM bytecode; features the VM can't compile — arrays/structs — aren't supported yet)");
                process::exit(1);
            }
        }
        return;
    }

    // Ahead-of-time compile to a native executable and stop.
    if let Some(out) = &cli.build {
        build_native(&program, &cli.entry, out);
        return;
    }

    // Choose the engine. `--vm` compiles to bytecode; if the program uses a feature
    // the VM doesn't compile yet, transparently fall back to the interpreter.
    let result: Result<Vec<String>, ruspy::diagnostics::Diag> = if cli.vm {
        match ruspy::vm::compile(&program) {
            Ok(bc) => {
                let mut vm = ruspy::vm::Vm::new_public(&bc);
                let mut host = DefaultHost;
                vm.run_public(&mut host).map(|()| vm.take_output())
            }
            Err(_unsupported) => run_interpreter(&program),
        }
    } else {
        run_interpreter(&program)
    };

    match result {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Err(d) => {
            eprintln!("{}: {}", li.label(d.span), d);
            process::exit(1);
        }
    }
}

fn run_interpreter(program: &ruspy::ast::Program) -> Result<Vec<String>, ruspy::diagnostics::Diag> {
    let mut interp = Interpreter::new();
    let mut host = DefaultHost;
    interp.run(program, &mut host).map(|()| interp.output().to_vec())
}

/// AOT-compile `program` to a standalone native executable at `out`, using `entry`
/// (a zero-arg numeric function) as the program's entry point. The compiled object
/// is linked with a tiny C runtime that calls the entry and prints its result.
#[cfg(feature = "jit")]
fn build_native(program: &ruspy::ast::Program, entry: &str, out: &str) {
    use ruspy::ast::StmtKind;
    use std::io::Write;
    use std::process::Command;

    // The entry must exist and take no arguments (nothing to feed it from the shell).
    let found = program.iter().find_map(|s| match &s.node {
        StmtKind::Fn { name, params, .. } if name == entry => Some(params.len()),
        _ => None,
    });
    match found {
        Some(0) => {}
        Some(n) => {
            eprintln!("error: entry `{entry}` takes {n} args; a native entry must take none");
            process::exit(1);
        }
        None => {
            eprintln!("error: no function named `{entry}` to use as the entry point");
            process::exit(1);
        }
    }

    let (obj, names) = match ruspy::jit::compile_object(program, "ruspy_program") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: native compile failed: {}", e.0);
            eprintln!("       (the native backend supports the scalar core; other code must run via the VM/interpreter)");
            process::exit(1);
        }
    };
    debug_assert!(names.iter().any(|n| n == entry));

    // Stage the object + a C shim in a temp dir, then link with the system `cc`.
    let dir = std::env::temp_dir().join(format!("ruspy_build_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let obj_path = dir.join("program.o");
    let shim_path = dir.join("shim.c");
    if let Err(e) = std::fs::write(&obj_path, &obj) {
        eprintln!("error: writing object: {e}");
        process::exit(1);
    }
    let shim = format!(
        "#include <stdio.h>\nextern double ruspy_{entry}(void);\nint main(void) {{\n    printf(\"%.17g\\n\", ruspy_{entry}());\n    return 0;\n}}\n"
    );
    if let Ok(mut f) = std::fs::File::create(&shim_path) {
        let _ = f.write_all(shim.as_bytes());
    }

    let status = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .arg(&obj_path)
        .arg(&shim_path)
        .arg("-o")
        .arg(out)
        .status();
    match status {
        Ok(s) if s.success() => {
            eprintln!("compiled {} functions to native code -> {out}", names.len());
        }
        Ok(s) => {
            eprintln!("error: linker exited with {s}");
            process::exit(1);
        }
        Err(e) => {
            eprintln!("error: could not run the C linker (`cc`): {e}");
            process::exit(1);
        }
    }
}

/// Stub when built without the native backend.
#[cfg(not(feature = "jit"))]
fn build_native(_program: &ruspy::ast::Program, _entry: &str, _out: &str) {
    eprintln!("error: `--build` needs the native backend; rebuild with `--features jit`");
    process::exit(1);
}
