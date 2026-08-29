//! Module loader — resolves `import "file.ruspy";` into one merged program.
//!
//! `import "path";` includes another source file (path relative to the importing
//! file), splicing its top-level declarations in place. Recursive, with **cycle
//! detection** (a file that transitively imports itself is an error) and **dedup**
//! (a file pulled in twice contributes its declarations once). Each file is parsed
//! **and type-checked against its own source**, so diagnostics carry the right
//! `file:line:col` even across a multi-file program.
//!
//! `import math;` (a package) is not a file — it is left in the merged program for
//! the checker/interpreter and never touches the filesystem.
//!
//! Known v1 limitation: cross-file *semantic* errors (calling an imported function
//! with the wrong argument types) aren't caught, because ruspy is gradually typed
//! and each file is checked in isolation; such a mismatch surfaces at runtime.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ast::{ImportSpec, Program, StmtKind};
use crate::diagnostics::LineIndex;

/// A load failure: pre-formatted `path:line:col: message` diagnostics.
#[derive(Debug)]
pub struct LoadError(pub Vec<String>);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join("\n"))
    }
}

/// Load and merge a strategy rooted at `path`, resolving file imports recursively.
pub fn load_file(path: &Path) -> Result<Program, LoadError> {
    let mut loaded = HashSet::new();
    let mut stack: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let prog = load_rec(path, &mut loaded, &mut stack, &mut errors);
    if errors.is_empty() {
        Ok(prog)
    } else {
        Err(LoadError(errors))
    }
}

/// A stable key for a path: its canonical form, or the path itself if it doesn't
/// exist yet (so a missing file still dedups/cycle-checks consistently).
fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn load_rec(
    path: &Path,
    loaded: &mut HashSet<PathBuf>,
    stack: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
) -> Program {
    let k = key(path);
    let shown = path.display();
    if stack.contains(&k) {
        errors.push(format!("{shown}: import cycle — this file imports itself (transitively)"));
        return Vec::new();
    }
    if !loaded.insert(k.clone()) {
        return Vec::new(); // already spliced elsewhere; contribute nothing again
    }

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!("{shown}: cannot read file: {e}"));
            return Vec::new();
        }
    };
    let li = LineIndex::new(&src);
    let (prog, diags) = crate::parser::parse_program(&src);
    let mut had_error = false;
    for d in diags.iter().filter(|d| d.is_error()) {
        errors.push(format!("{shown}:{} {d}", li.label(d.span)));
        had_error = true;
    }
    for d in crate::check::check(&prog).iter().filter(|d| d.is_error()) {
        errors.push(format!("{shown}:{} {d}", li.label(d.span)));
        had_error = true;
    }
    if had_error {
        return Vec::new();
    }

    // Splice: replace each `import "file"` with the (recursively loaded) file's
    // declarations; keep everything else (including package imports).
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    stack.push(k);
    let mut out = Vec::new();
    for stmt in prog {
        match &stmt.node {
            StmtKind::Import { spec: ImportSpec::File(rel) } => {
                let child = dir.join(rel);
                out.extend(load_rec(&child, loaded, stack, errors));
            }
            _ => out.push(stmt),
        }
    }
    stack.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ruspy_loader_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::File::create(&p).unwrap().write_all(content.as_bytes()).unwrap();
        p
    }

    fn run_prog(prog: Program) -> Vec<String> {
        let mut interp = crate::interpreter::Interpreter::new();
        let mut host = crate::interpreter::DefaultHost;
        interp.run(&prog, &mut host).expect("merged program runs");
        interp.output().to_vec()
    }

    #[test]
    fn imports_a_file_and_calls_its_function() {
        tmp("lib.ruspy", "fn double(x) { return x * 2; }");
        let main = tmp("main.ruspy", "import \"lib.ruspy\";\nprint double(21);");
        let prog = load_file(&main).expect("loads");
        assert_eq!(run_prog(prog), vec!["42"]);
    }

    #[test]
    fn a_cycle_is_an_error_not_a_hang() {
        tmp("cyc_a.ruspy", "import \"cyc_b.ruspy\";\nfn fa() { return 1; }");
        let b = tmp("cyc_b.ruspy", "import \"cyc_a.ruspy\";\nfn fb() { return 2; }");
        let err = load_file(&b).expect_err("cycle should error");
        assert!(err.0.iter().any(|m| m.contains("cycle")), "got {:?}", err.0);
    }

    #[test]
    fn a_missing_import_reports_the_file() {
        let main = tmp("uses_missing.ruspy", "import \"nope.ruspy\";\nprint 1;");
        let err = load_file(&main).expect_err("missing file");
        assert!(err.0.iter().any(|m| m.contains("cannot read")), "got {:?}", err.0);
    }

    #[test]
    fn a_diamond_import_is_spliced_once() {
        tmp("shared.ruspy", "fn v() { return 7; }");
        tmp("mid.ruspy", "import \"shared.ruspy\";");
        // main pulls `shared` directly AND via `mid` — it must be spliced once.
        let main = tmp("diamond.ruspy", "import \"shared.ruspy\";\nimport \"mid.ruspy\";\nprint v();");
        let prog = load_file(&main).expect("loads");
        assert_eq!(run_prog(prog), vec!["7"]);
    }

    #[test]
    fn a_parse_error_in_an_imported_file_names_that_file() {
        tmp("broken.ruspy", "fn f( { return 1; }"); // malformed params
        let main = tmp("uses_broken.ruspy", "import \"broken.ruspy\";\nprint 1;");
        let err = load_file(&main).expect_err("bad import");
        assert!(err.0.iter().any(|m| m.contains("broken.ruspy")), "names the file: {:?}", err.0);
    }
}
