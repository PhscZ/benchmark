#!/usr/bin/env bash
#
# Differential test harness for ccomp.
#
# For every tests/<name>.c the harness builds two executables:
#   * ccomp:  ccomp < src  ->  .s  ->  gcc -> exe
#   * gcc:    the same source compiled directly by GCC
# Both are then run with identical arguments and identical stdin, and their
# stdout bytes and exit status must match exactly.
#
# Usage:  bash run_tests.sh
#
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CC="${CC:-$ROOT/target/release/ccomp.exe}"
GCC="${GCC:-gcc}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ccomp-test.XXXXXX")"
FIX="$ROOT/tests/stdin"

trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0
failures=()

# Build both executables for one test source.  Returns non-zero on failure.
build_pair() {
    local name="$1"
    local src="$ROOT/tests/$name.c"

    if ! "$CC" < "$src" > "$WORK/$name.s" 2> "$WORK/$name.ccomp.log"; then
        echo "  FAIL [$name] ccomp rejected the source:"
        sed 's/^/    /' "$WORK/$name.ccomp.log"
        return 1
    fi
    if ! "$GCC" -o "$WORK/${name}_cc.exe" "$WORK/$name.s" > "$WORK/$name.as.log" 2>&1; then
        echo "  FAIL [$name] gcc could not assemble/link the ccomp output:"
        sed 's/^/    /' "$WORK/$name.as.log"
        return 1
    fi
    # The reference is built as C17: the subset under test is C89-flavoured, and
    # C23 rejects implicit function declarations and a bare `return;` in a
    # non-void function, both of which this compiler still accepts.
    if ! "$GCC" -std=gnu17 -w -fno-builtin \
            -Wno-error=implicit-function-declaration -Wno-error=implicit-int \
            -o "$WORK/${name}_gcc.exe" "$src" \
            "$ROOT/support/ref_binary.c" > "$WORK/$name.gcc.log" 2>&1; then
        echo "  FAIL [$name] gcc reference build failed:"
        sed 's/^/    /' "$WORK/$name.gcc.log"
        return 1
    fi
    return 0
}

# Run one (test, stdin fixture) pair against both executables.
run_case() {
    local name="$1"
    local fixture="$2"
    local label="$name/$fixture"
    local in="$FIX/$fixture"

    timeout 30 "$WORK/${name}_cc.exe" alpha beta < "$in" > "$WORK/$name.cc.out" 2> "$WORK/$name.cc.err"
    local rc_cc=$?
    timeout 30 "$WORK/${name}_gcc.exe" alpha beta < "$in" > "$WORK/$name.gcc.out" 2> "$WORK/$name.gcc.err"
    local rc_gcc=$?

    if [ "$rc_cc" != "$rc_gcc" ]; then
        echo "  FAIL [$label] exit status: ccomp=$rc_cc gcc=$rc_gcc"
        failures+=("$label: exit status $rc_cc vs $rc_gcc")
        fail=$((fail + 1))
        return
    fi
    if ! cmp -s "$WORK/$name.cc.out" "$WORK/$name.gcc.out"; then
        echo "  FAIL [$label] stdout differs (ccomp $(wc -c < "$WORK/$name.cc.out") bytes, gcc $(wc -c < "$WORK/$name.gcc.out") bytes)"
        diff <(od -c "$WORK/$name.gcc.out") <(od -c "$WORK/$name.cc.out") | head -20 | sed 's/^/    /'
        failures+=("$label: stdout mismatch")
        fail=$((fail + 1))
        return
    fi
    if ! cmp -s "$WORK/$name.cc.err" "$WORK/$name.gcc.err"; then
        echo "  FAIL [$label] stderr differs"
        failures+=("$label: stderr mismatch")
        fail=$((fail + 1))
        return
    fi
    pass=$((pass + 1))
    printf '  ok   [%s] %d bytes, exit %d\n' "$label" "$(wc -c < "$WORK/$name.cc.out")" "$rc_cc"
}

# Run one test over several stdin fixtures.
check() {
    local name="$1"
    shift
    echo "== $name"
    if ! build_pair "$name"; then
        fail=$((fail + 1))
        failures+=("$name: build")
        return
    fi
    for fixture in "$@"; do
        run_case "$name" "$fixture"
    done
}

check hello      empty.bin
check arith      empty.bin
check control    empty.bin
check funcs      empty.bin
check pointers   empty.bin
check types      empty.bin
check literals   empty.bin
check mainargs   empty.bin
check variadic   empty.bin
check abi        empty.bin
check bytes      binary.bin empty.bin words.txt
check statics    empty.bin
check globals    empty.bin
check implicit   empty.bin
check stress     empty.bin
check exitcode   empty.bin x.bin a.bin
check cat        binary.bin empty.bin words.txt
check wc         words.txt binary.bin empty.bin
check head       words.txt binary.bin empty.bin
check hexdump    binary.bin empty.bin words.txt
check calc       exprs.txt empty.bin

# ---------------------------------------------------------------------------
# Self-checks: properties that hold independently of the GCC comparison.
# ---------------------------------------------------------------------------
echo "== self-checks"
self_pass=0
self_fail=0

self_ok() {
    self_pass=$((self_pass + 1))
    printf '  ok   %s\n' "$1"
}

self_bad() {
    self_fail=$((self_fail + 1))
    failures+=("self-check: $1")
    printf '  FAIL %s\n' "$1"
}

# A generated program must copy stdin to stdout byte for byte: no CRLF
# translation on output, no Ctrl-Z (0x1A) end-of-file, NULs preserved.
"$WORK/cat_cc.exe" < "$FIX/binary.bin" > "$WORK/self_binary.out"
if cmp -s "$WORK/self_binary.out" "$FIX/binary.bin"; then
    self_ok "cat output is byte-identical to its binary input ($(wc -c < "$FIX/binary.bin") bytes)"
else
    self_bad "cat output differs from its input"
fi

# Same, but with stdin that is *only* 0x1A and 0xFF: neither may be treated as
# end of file, and EOF must still be detected.
printf '\032\377\032' > "$WORK/sub.bin"
"$WORK/cat_cc.exe" < "$WORK/sub.bin" > "$WORK/sub.out"
if cmp -s "$WORK/sub.out" "$WORK/sub.bin"; then
    self_ok "0x1A and 0xFF pass through stdin unchanged"
else
    self_bad "0x1A or 0xFF mishandled on stdin"
fi

# An empty stdin must produce an empty stdout and a clean exit.
"$WORK/cat_cc.exe" < "$FIX/empty.bin" > "$WORK/self_empty.out"
if [ ! -s "$WORK/self_empty.out" ]; then
    self_ok "empty stdin produces empty output"
else
    self_bad "empty stdin produced output"
fi

# The compiler must accept empty input and emit assembleable output.
if "$CC" < /dev/null > "$WORK/empty.s" 2> "$WORK/empty.log" \
   && "$GCC" -c -o "$WORK/empty.o" "$WORK/empty.s" 2>> "$WORK/empty.log"; then
    self_ok "empty source compiles to assembleable output"
else
    self_bad "empty source rejected"
fi

# Comments and whitespace only.
printf '/* nothing here */\n// nor here\n\n' > "$WORK/comments.c"
if "$CC" < "$WORK/comments.c" > "$WORK/comments.s" 2> "$WORK/comments.log" \
   && "$GCC" -c -o "$WORK/comments.o" "$WORK/comments.s" 2>> "$WORK/comments.log"; then
    self_ok "comment-only source compiles"
else
    self_bad "comment-only source rejected"
fi

# A program with only globals and no main must still assemble.
printf 'int a;\nchar b = 7;\nchar *c = "x";\n' > "$WORK/nomain.c"
if "$CC" < "$WORK/nomain.c" > "$WORK/nomain.s" 2> "$WORK/nomain.log" \
   && "$GCC" -c -o "$WORK/nomain.o" "$WORK/nomain.s" 2>> "$WORK/nomain.log"; then
    self_ok "declarations without main compile"
else
    self_bad "declarations without main rejected"
fi

# A source file with Windows CRLF line endings must compile to the same
# assembly as the LF version.
printf 'int main(void) {\r\n    return 0;\r\n}\r\n' > "$WORK/crlf.c"
printf 'int main(void) {\n    return 0;\n}\n' > "$WORK/lf.c"
"$CC" < "$WORK/crlf.c" > "$WORK/crlf.s" 2> "$WORK/crlf.log"
"$CC" < "$WORK/lf.c" > "$WORK/lf.s" 2> "$WORK/lf.log"
if cmp -s "$WORK/crlf.s" "$WORK/lf.s"; then
    self_ok "CRLF and LF sources compile identically"
else
    self_bad "CRLF source compiles differently from LF source"
fi

# A source with a UTF-8 BOM must still compile.
printf '\357\273\277int main(void) { return 0; }\n' > "$WORK/bom.c"
if "$CC" < "$WORK/bom.c" > "$WORK/bom.s" 2> "$WORK/bom.log"; then
    self_ok "source with a UTF-8 BOM compiles"
else
    self_bad "source with a UTF-8 BOM rejected"
fi

# Errors must be reported on stderr with a non-zero exit status.
check_rejects() {
    local label="$1"
    local code="$2"
    printf '%s' "$code" > "$WORK/bad.c"
    if "$CC" < "$WORK/bad.c" > "$WORK/bad.s" 2> "$WORK/bad.log"; then
        self_bad "accepted invalid source: $label"
    elif [ -s "$WORK/bad.log" ]; then
        self_ok "rejects $label"
    else
        self_bad "rejected $label without a diagnostic"
    fi
}

check_rejects "an undeclared identifier" 'int main(void) { return zzz; }'
check_rejects "a missing semicolon" 'int main(void) { int x = 1 return x; }'
check_rejects "an unterminated string" 'int main(void) { printf("x); }'
check_rejects "an unterminated comment" 'int main(void) { return 0; } /* oops'
check_rejects "an unbalanced brace" 'int main(void) { if (1) { return 0; }'
check_rejects "a bad operand type" 'int main(void) { int *p; return p * 2; }'

echo
echo "passed: $pass   failed: $fail"
if [ "$fail" -ne 0 ]; then
    printf '%s\n' "${failures[@]}"
    exit 1
fi
exit 0
