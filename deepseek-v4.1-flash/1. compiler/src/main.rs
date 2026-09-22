//! ccomp — a compiler for a subset of C targeting Windows x64.
//!
//! Reads C source from stdin, writes GNU assembler (Intel syntax) to stdout.

mod ast;
mod codegen;
mod lexer;
mod parser;
mod types;

use std::io::{Read, Write};

fn main() {
    let mut src = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut src) {
        eprintln!("ccomp: cannot read stdin: {}", e);
        std::process::exit(1);
    }

    match compile(&src) {
        Ok(asm) => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            if lock.write_all(asm.as_bytes()).is_err() {
                std::process::exit(1);
            }
            let _ = lock.flush();
        }
        Err(e) => {
            eprintln!("ccomp: error: {}", e);
            std::process::exit(1);
        }
    }
}

fn compile(src: &[u8]) -> Result<String, String> {
    let toks = lexer::tokenize(src)?;
    let mut p = parser::Parser::new(toks);
    p.parse_program()?;
    let cg = codegen::Codegen::new(p.objs, p.strings);
    Ok(cg.generate())
}
