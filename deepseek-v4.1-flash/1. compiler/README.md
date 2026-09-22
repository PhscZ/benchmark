# ccomp — a C subset compiler targeting Windows x64

`ccomp` reads C source from **stdin** and writes **GNU assembler (Intel syntax)**
to **stdout**. The output targets Windows x64, uses the Microsoft x64 calling
convention, and is assembled and linked with MinGW-w64 GCC against its C runtime.

## Build

```sh
cargo build --release
```

Produces `target/release/ccomp.exe` (Rust 1.70+, no dependencies).

## Run

```sh
# compile C (stdin) to assembly (stdout)
./target/release/ccomp.exe < prog.c > prog.s

# assemble and link against the MinGW-w64 C runtime
gcc -o prog.exe prog.s

# run it
./prog.exe < input.bin > output.bin
```

One-liner:

```sh
./target/release/ccomp.exe < prog.c > prog.s && gcc -o prog.exe prog.s && ./prog.exe
```

The generated program needs no headers and no extra object files; it links
against the C runtime that `gcc` supplies by default. The compiler takes no
arguments and has no flags: everything arrives on stdin.

## Tests

```sh
bash run_tests.sh
```

For each `tests/<name>.c` the harness builds two executables — one through
`ccomp` + `gcc`, one with `gcc` alone — then runs both with identical arguments
and identical stdin, and requires **byte-identical stdout, byte-identical
stderr, and an identical exit status**. 33 differential cases and 14
self-checks run in about 25 seconds.

The reference builds are linked with `support/ref_binary.c`, which puts the GCC
build's standard streams into binary mode so the two byte streams are directly
comparable, and are compiled as `-std=gnu17` because C23 rejects implicit
function declarations that this compiler still accepts.

Test programs:

| file | covers |
|---|---|
| `arith.c` | every operator, precedence, short-circuit, compound assignment, `++`/`--` |
| `control.c` | `if`/`else`, `while`, `do`/`while`, `for`, `break`, `continue`, scoping |
| `funcs.c` | prototypes, recursion, mutual recursion, 0–10 parameters, `void` |
| `pointers.c` | `&`, `*`, indexing, pointer arithmetic, `**`, multi-dimensional arrays |
| `types.c` | `char`/`unsigned char`/`int`, promotions, casts, `sizeof` |
| `literals.c` | decimal/hex/octal literals, escapes, comments, string termination |
| `mainargs.c` | `int main(int argc, char **argv)` with real arguments |
| `variadic.c` | variadic `printf` with register and stack arguments |
| `abi.c` | every argument count 1–13, mixed widths, nested calls in argument lists |
| `bytes.c` | raw non-ASCII source bytes; `0xFF`/`0x1A` vs EOF |
| `statics.c` | block-scope and file-scope `static`, static arrays, shadowing |
| `globals.c` | address-valued initializers, pointer arrays, functions returning pointers |
| `implicit.c` | C89-style implicit declarations, mutual recursion by use |
| `stress.c` | large frames, 32×32 arrays, deep block nesting, long expressions |
| `exitcode.c` | `main`'s return value as the process exit status |
| `cat.c`, `wc.c`, `head.c`, `hexdump.c`, `calc.c` | utilities compiled end to end |

## Supported subset

**Types** — `void`, 8-bit `char` (signed), `unsigned char`, 32-bit `int`,
pointers, fixed-size arrays, and pointer-to-pointer. `const`, `volatile`,
`signed`, `unsigned`, `register`, `auto` and `inline` are accepted and ignored.
Function pointers are not supported (and are reported as such).

**Declarations** — local, global and block-scope variables; static and dynamic
initialization, including nested brace initializers, string-literal
initializers for `char` arrays, and address constants such as `&table[2]`;
`static` locals and file-scope statics; array sizes inferred from the
initializer (`char s[] = "..."`); comma-separated declarator lists;
parenthesized declarators (`int (*p)[4]`); prototypes and definitions.

**Expressions** — full C operator precedence; arithmetic, comparison, logical
and bitwise operators; short-circuit `&&`/`||`; assignment, compound assignment
and prefix/postfix `++`/`--`; `?:`, comma, `sizeof`, casts; address-of,
dereference, indexing (in either operand order) and pointer arithmetic scaled by
the pointee size; array-to-pointer decay; integer promotions and
`char`/`unsigned char`/`int` conversions.

**Statements** — `if`/`else`, `while`, `do`/`while`, `for` (including
declarations in the init clause), `break`, `continue`, `return`, and compound
statements with lexical scoping and shadowing.

**Literals** — decimal, hexadecimal and octal integers; character literals;
string literals with `\n`, `\r`, `\t`, `\0`, `\\`, `\"`, `\'`, `\a`, `\b`,
`\f`, `\v`, `\?`, octal (`\101`) and hex (`\x41`) escapes; `//` and `/* */`
comments. String literals are null-terminated. Sources may use LF or CRLF line
endings and may begin with a UTF-8 BOM; non-ASCII bytes pass through unchanged.

**Entry points** — `int main(void)` and `int main(int argc, char **argv)`.

**Known externals** — `getchar`, `putchar` and `printf` are recognised with
their correct signatures without any declaration in the source, including
variadic `printf` calls. `puts`, `exit`, `atoi`, `abs`, `strlen`, `strcmp`,
`memset`, `memcpy`, `malloc`, `calloc`, `realloc` and `free` are recognised as
well. Other functions may be declared explicitly or called after an implicit
`int f()` declaration (C89 style). `getchar` returns `int`, so `EOF` (`-1`)
stays distinct from every input byte.

## Binary-safe standard streams

A generated program calls `_setmode(0, _O_BINARY)` and `_setmode(1, _O_BINARY)`
at the top of `main`, after spilling its parameters. There is therefore no CRLF
translation on output and no Ctrl-Z (`0x1A`) end-of-file on input; programs copy
arbitrary bytes, including NULs. The `cat` test proves this directly: its output
is byte-identical to an 18-byte input containing `0x00`, `0x0D`, `0x0A`,
`0x1A`, `0x7F`, `0x80`, `0xFF` and UTF-8 sequences.

## Code generation

A straightforward stack machine: every expression evaluates into `RAX`, with
intermediate results parked on the machine stack. Only volatile registers
(`RAX`, `RCX`, `RDX`, `R8`–`R11`) are used, so no callee-saved register ever
needs spilling and the frame contains nothing but the locals area.

- **Calling convention** (Microsoft x64): the first four arguments go in
  `RCX`, `RDX`, `R8`, `R9`; further arguments are pushed so that the fifth lands
  at `[rsp+32]`, above the 32-byte shadow space. A callee therefore reads its
  first stack parameter at `[rbp+48]`. `RSP` is kept 16-byte aligned at every
  `call` (an 8-byte pad is inserted when the expression stack has odd depth),
  and `AL` is zeroed before each call because no vector registers are used.
- **Values narrower than 32 bits** are always held sign- or zero-extended in
  `EAX`, so widening is a no-op; only narrowing needs an instruction. Widening
  an `int` to a pointer sign-extends with `cdqe`, which is never applied to a
  live pointer value.
- **Symbols** — user globals and defined functions are emitted with a `__cc_`
  prefix, so an identifier like `sp`, `ax` or `byte` cannot be mistaken for a
  register or a directive by the assembler. `main` and any function left
  undefined (the C runtime entry points) keep their own names.
- **String literals** are emitted into `.data` (writable, like the historical
  behaviour), so writing through a pointer to one does not fault.

## Layout

```
src/main.rs      driver: stdin -> tokenize -> parse -> codegen -> stdout
src/lexer.rs     byte-oriented lexer (never treats input as text)
src/types.rs     type representation, sizes and alignment
src/ast.rs       typed AST and the object table
src/parser.rs    recursive-descent parser; scoping, typing, conversions
src/codegen.rs   x86-64 code generator (Microsoft x64 ABI)
tests/*.c        test programs
tests/stdin/     binary stdin fixtures
support/         helper linked into the GCC reference builds only
run_tests.sh     differential test harness
```
