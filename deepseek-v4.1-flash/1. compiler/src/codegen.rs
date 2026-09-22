//! x86-64 code generation for Windows (Microsoft x64 ABI), emitted as GNU
//! assembler Intel-syntax source.
//!
//! Strategy: a simple stack machine.  Every expression evaluates into RAX, and
//! sub-results are parked on the machine stack.  Only volatile registers are
//! used (RAX, RCX, RDX, R8-R11), so nothing needs saving across calls and the
//! frame consists purely of the locals area.
//!
//! Stack discipline: after the prologue RSP is 16-byte aligned and `depth`
//! counts outstanding 8-byte pushes.  `depth` is even exactly when RSP is
//! 16-byte aligned, which is what the ABI requires at a `call`.

use crate::ast::*;
use crate::types::Ty;

const ARG_REGS: [&str; 4] = ["rcx", "rdx", "r8", "r9"];
const ARG_REGS32: [&str; 4] = ["ecx", "edx", "r8d", "r9d"];

/// `_O_BINARY` from the Microsoft C runtime.
const O_BINARY: u32 = 0x8000;

pub struct Codegen {
    out: String,
    depth: usize,
    label: usize,
    objs: Vec<Obj>,
    strings: Vec<Vec<u8>>,
    breaks: Vec<String>,
    conts: Vec<String>,
    ret_label: String,
    cur_ret: Ty,
    /// User globals/functions are emitted under a mangled name so that an
    /// identifier like `sp`, `ax` or `byte` cannot be mistaken for a register,
    /// a size directive, or a runtime symbol by the assembler.
    mangled: std::collections::HashMap<String, String>,
}

impl Codegen {
    pub fn new(objs: Vec<Obj>, strings: Vec<Vec<u8>>) -> Codegen {
        let mut mangled = std::collections::HashMap::new();
        for o in objs.iter() {
            if o.is_local {
                continue;
            }
            if o.is_func {
                // A definition is internal to this translation unit (except
                // `main`); anything left undefined is an external symbol that
                // the C runtime has to resolve, so it keeps its own name.
                if o.defined && o.name != "main" {
                    mangled.insert(o.name.clone(), format!("__cc_{}", o.name));
                }
            } else {
                mangled.insert(o.name.clone(), format!("__cc_{}", o.name));
            }
        }
        Codegen {
            out: String::new(),
            depth: 0,
            label: 0,
            objs,
            strings,
            breaks: Vec::new(),
            conts: Vec::new(),
            ret_label: String::new(),
            cur_ret: Ty::Void,
            mangled,
        }
    }

    fn sym_name(&self, id: ObjId) -> String {
        let name = &self.objs[id].name;
        match self.mangled.get(name) {
            Some(m) => m.clone(),
            None => name.clone(),
        }
    }

    /// Maps a name stored in static initializer data (a global variable or a
    /// string-literal label) to its emitted symbol.
    fn data_sym(&self, name: &str) -> String {
        match self.mangled.get(name) {
            Some(m) => m.clone(),
            None => name.to_string(),
        }
    }

    pub fn generate(mut self) -> String {
        self.emit(".intel_syntax noprefix");
        self.emit(".text");
        for i in 0..self.objs.len() {
            if self.objs[i].is_func && self.objs[i].defined {
                self.gen_func(i);
            }
        }
        self.gen_static_data();
        self.out
    }

    // ------------------------------------------------------------- plumbing

    fn emit(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn new_label(&mut self) -> String {
        self.label += 1;
        format!(".L{}", self.label)
    }

    fn push(&mut self) {
        self.emit("  push rax");
        self.depth += 1;
    }

    fn pop(&mut self, reg: &str) {
        self.emit(&format!("  pop {}", reg));
        self.depth -= 1;
    }

    // ------------------------------------------------------------- functions

    fn gen_func(&mut self, id: ObjId) {
        let name = self.sym_name(id);
        let stack = self.objs[id].stack_size;
        let params = self.objs[id].params.clone();
        let body = self.objs[id].body.clone().unwrap();
        let ret_ty = self.objs[id].func_ty().map(|f| f.ret).unwrap_or(Ty::Void);

        self.emit("");
        self.emit(&format!(".globl {}", name));
        self.emit(&format!("{}:", name));
        self.emit("  push rbp");
        self.emit("  mov rbp, rsp");
        if stack > 0 {
            self.emit(&format!("  sub rsp, {}", stack));
        }
        self.depth = 0;
        self.ret_label = self.new_label();
        self.cur_ret = ret_ty;

        // Spill incoming arguments into their stack slots.
        for (i, &pid) in params.iter().enumerate() {
            let ty = self.objs[pid].ty.clone();
            let off = self.objs[pid].offset;
            if i < 4 {
                match &ty {
                    Ty::Char | Ty::UChar => {
                        self.emit(&format!("  mov rax, {}", ARG_REGS[i]));
                        self.emit(&format!("  mov byte ptr [rbp - {}], al", off));
                    }
                    Ty::Ptr(_) => {
                        self.emit(&format!("  mov qword ptr [rbp - {}], {}", off, ARG_REGS[i]));
                    }
                    _ => {
                        self.emit(&format!("  mov dword ptr [rbp - {}], {}", off, ARG_REGS32[i]));
                    }
                }
            } else {
                // Above the 32-byte home area, which the callee sees at
                // [rbp+16]..[rbp+48); the first stack argument is at [rbp+48].
                let src = 16 + 8 * i;
                match &ty {
                    Ty::Char | Ty::UChar => {
                        self.emit(&format!("  mov al, byte ptr [rbp + {}]", src));
                        self.emit(&format!("  mov byte ptr [rbp - {}], al", off));
                    }
                    Ty::Ptr(_) => {
                        self.emit(&format!("  mov rax, qword ptr [rbp + {}]", src));
                        self.emit(&format!("  mov qword ptr [rbp - {}], rax", off));
                    }
                    _ => {
                        self.emit(&format!("  mov eax, dword ptr [rbp + {}]", src));
                        self.emit(&format!("  mov dword ptr [rbp - {}], eax", off));
                    }
                }
            }
        }

        // Generated programs must read and write stdin/stdout as raw bytes:
        // no CRLF translation, and Ctrl-Z must not terminate input.  Both
        // standard streams are switched to binary mode on entry to main.
        if name == "main" {
            self.emit("  sub rsp, 32");
            self.emit("  mov ecx, 0");
            self.emit(&format!("  mov edx, {}", O_BINARY));
            self.emit("  call _setmode");
            self.emit("  mov ecx, 1");
            self.emit(&format!("  mov edx, {}", O_BINARY));
            self.emit("  call _setmode");
            self.emit("  add rsp, 32");
        }

        self.gen_stmt(&body);

        // Falling off the end of `main` yields status 0 (C99 5.1.2.2.3).
        if name == "main" {
            self.emit("  mov eax, 0");
        }
        let lbl = self.ret_label.clone();
        self.emit(&format!("{}:", lbl));
        self.emit("  mov rsp, rbp");
        self.emit("  pop rbp");
        self.emit("  ret");
    }

    // ------------------------------------------------------------ statements

    fn gen_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Empty => {}
            Stmt::Expr(e) => self.gen_expr(e),
            Stmt::Block(list) => {
                for st in list {
                    self.gen_stmt(st);
                }
            }
            Stmt::Return(e) => {
                match e {
                    None => {
                        if !self.cur_ret.is_void() {
                            self.emit("  mov eax, 0");
                        }
                    }
                    Some(x) => {
                        if self.cur_ret.is_void() {
                            self.gen_expr(x);
                        } else {
                            self.gen_expr(x);
                            let from = x.ty.clone();
                            let to = self.cur_ret.clone();
                            self.cast_value(&from, &to);
                        }
                    }
                }
                let lbl = self.ret_label.clone();
                self.emit(&format!("  jmp {}", lbl));
            }
            Stmt::If(c, then, els) => {
                let l_else = self.new_label();
                let l_end = self.new_label();
                self.gen_expr(c);
                self.gen_test(c);
                self.emit(&format!("  je {}", l_else));
                self.gen_stmt(then);
                if els.is_some() {
                    self.emit(&format!("  jmp {}", l_end));
                }
                self.emit(&format!("{}:", l_else));
                if let Some(e) = els {
                    self.gen_stmt(e);
                    self.emit(&format!("{}:", l_end));
                }
            }
            Stmt::While(c, body) => {
                let l_begin = self.new_label();
                let l_brk = self.new_label();
                self.breaks.push(l_brk.clone());
                self.conts.push(l_begin.clone());
                self.emit(&format!("{}:", l_begin));
                self.gen_expr(c);
                self.gen_test(c);
                self.emit(&format!("  je {}", l_brk));
                self.gen_stmt(body);
                self.emit(&format!("  jmp {}", l_begin));
                self.emit(&format!("{}:", l_brk));
                self.breaks.pop();
                self.conts.pop();
            }
            Stmt::DoWhile(body, c) => {
                let l_begin = self.new_label();
                let l_cont = self.new_label();
                let l_brk = self.new_label();
                self.breaks.push(l_brk.clone());
                self.conts.push(l_cont.clone());
                self.emit(&format!("{}:", l_begin));
                self.gen_stmt(body);
                self.emit(&format!("{}:", l_cont));
                self.gen_expr(c);
                self.gen_test(c);
                self.emit(&format!("  jne {}", l_begin));
                self.emit(&format!("{}:", l_brk));
                self.breaks.pop();
                self.conts.pop();
            }
            Stmt::For(init, cond, step, body) => {
                let l_begin = self.new_label();
                let l_cont = self.new_label();
                let l_brk = self.new_label();
                self.gen_stmt(init);
                self.breaks.push(l_brk.clone());
                self.conts.push(l_cont.clone());
                self.emit(&format!("{}:", l_begin));
                if let Some(c) = cond {
                    self.gen_expr(c);
                    self.gen_test(c);
                    self.emit(&format!("  je {}", l_brk));
                }
                self.gen_stmt(body);
                self.emit(&format!("{}:", l_cont));
                if let Some(st) = step {
                    self.gen_expr(st);
                }
                self.emit(&format!("  jmp {}", l_begin));
                self.emit(&format!("{}:", l_brk));
                self.breaks.pop();
                self.conts.pop();
            }
            Stmt::Break => {
                if let Some(l) = self.breaks.last().cloned() {
                    self.emit(&format!("  jmp {}", l));
                }
            }
            Stmt::Continue => {
                if let Some(l) = self.conts.last().cloned() {
                    self.emit(&format!("  jmp {}", l));
                }
            }
        }
    }

    // ----------------------------------------------------------- expressions

    fn gen_test(&mut self, e: &Expr) {
        if e.ty.is_ptr() {
            self.emit("  test rax, rax");
        } else {
            self.emit("  test eax, eax");
        }
    }

    /// Converts the value in RAX from `from` to `to`.
    ///
    /// Integers narrower than 32 bits are always held sign- or zero-extended in
    /// EAX, so widening is a no-op.  Widening to a pointer must sign-extend into
    /// the full 64-bit register.
    fn cast_value(&mut self, from: &Ty, to: &Ty) {
        if to.is_void() || from.is_void() {
            return;
        }
        match to {
            Ty::Char => self.emit("  movsx eax, al"),
            Ty::UChar => self.emit("  movzx eax, al"),
            Ty::Int => {}
            Ty::Ptr(_) => {
                if from.is_integer() {
                    self.emit("  cdqe");
                }
            }
            _ => {}
        }
    }

    /// Ensures RAX holds a sign-extended 64-bit integer, for use as an offset.
    fn widen_int(&mut self) {
        self.emit("  cdqe");
    }

    fn gen_addr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Var(id) => {
                if self.objs[*id].is_local {
                    let off = self.objs[*id].offset;
                    self.emit(&format!("  lea rax, [rbp - {}]", off));
                } else {
                    let name = self.sym_name(*id);
                    self.emit(&format!("  lea rax, [rip + {}]", name));
                }
            }
            ExprKind::Unary(UnOp::Deref, inner) => self.gen_expr(inner),
            ExprKind::Str(i) => {
                let i = *i;
                self.emit(&format!("  lea rax, [rip + .LC{}]", i));
            }
            _ => self.emit("  ; internal error: expression is not addressable"),
        }
    }

    fn load_from(&mut self, ty: &Ty, reg: &str) {
        match ty {
            Ty::Char => self.emit(&format!("  movsx eax, byte ptr [{}]", reg)),
            Ty::UChar => self.emit(&format!("  movzx eax, byte ptr [{}]", reg)),
            Ty::Int => self.emit(&format!("  mov eax, dword ptr [{}]", reg)),
            Ty::Ptr(_) => self.emit(&format!("  mov rax, qword ptr [{}]", reg)),
            _ => self.emit("  ; internal error: cannot load this type"),
        }
    }

    fn store_to(&mut self, ty: &Ty, reg: &str) {
        match ty {
            Ty::Char | Ty::UChar => self.emit(&format!("  mov byte ptr [{}], al", reg)),
            Ty::Int => self.emit(&format!("  mov dword ptr [{}], eax", reg)),
            Ty::Ptr(_) => self.emit(&format!("  mov qword ptr [{}], rax", reg)),
            _ => self.emit("  ; internal error: cannot store this type"),
        }
    }

    fn gen_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Num(v) => {
                self.emit(&format!("  mov eax, {}", *v as i32));
            }
            ExprKind::Str(i) => {
                let i = *i;
                self.emit(&format!("  lea rax, [rip + .LC{}]", i));
            }
            ExprKind::Var(id) => {
                let id = *id;
                if self.objs[id].ty.is_array() {
                    self.gen_addr(e);
                } else {
                    self.gen_addr(e);
                    let ty = self.objs[id].ty.clone();
                    self.load_from(&ty, "rax");
                }
            }
            ExprKind::Cast(inner, to) => {
                let to = to.clone();
                if inner.ty.is_array() {
                    self.gen_addr(inner);
                } else {
                    self.gen_expr(inner);
                    let from = inner.ty.clone();
                    self.cast_value(&from, &to);
                }
            }
            ExprKind::Assign(lhs, rhs) => {
                self.gen_addr(lhs);
                self.push();
                self.gen_expr(rhs);
                self.pop("rcx");
                let ty = lhs.ty.clone();
                self.store_to(&ty, "rcx");
            }
            ExprKind::CompoundAssign(op, lhs, rhs) => {
                self.gen_addr(lhs);
                self.emit("  mov rcx, rax");
                self.push();
                let lty = lhs.ty.clone();
                self.load_from(&lty, "rcx");
                self.push();
                self.gen_expr(rhs);
                self.pop("rcx");
                let int = Ty::Int;
                self.gen_arith(*op, &lty, &int);
                let lty2 = lhs.ty.clone();
                if lty2.is_integer() {
                    self.cast_value(&int, &lty2);
                }
                self.pop("rcx");
                let lty3 = lhs.ty.clone();
                self.store_to(&lty3, "rcx");
            }
            ExprKind::Unary(op, inner) => match op {
                UnOp::Neg => {
                    self.gen_expr(inner);
                    self.emit("  neg eax");
                }
                UnOp::BitNot => {
                    self.gen_expr(inner);
                    self.emit("  not eax");
                }
                UnOp::Not => {
                    self.gen_expr(inner);
                    self.gen_test(inner);
                    self.emit("  sete al");
                    self.emit("  movzx eax, al");
                }
                UnOp::Addr => self.gen_addr(inner),
                UnOp::Deref => {
                    self.gen_expr(inner);
                    let ty = e.ty.clone();
                    if !ty.is_array() && !ty.is_func() {
                        self.load_from(&ty, "rax");
                    }
                }
            },
            ExprKind::Binary(op, lhs, rhs) => self.gen_binary(*op, lhs, rhs),
            ExprKind::Cond(c, t, f) => {
                let l_else = self.new_label();
                let l_end = self.new_label();
                self.gen_expr(c);
                self.gen_test(c);
                self.emit(&format!("  je {}", l_else));
                self.gen_expr(t);
                self.emit(&format!("  jmp {}", l_end));
                self.emit(&format!("{}:", l_else));
                self.gen_expr(f);
                self.emit(&format!("{}:", l_end));
            }
            ExprKind::Comma(l, r) => {
                self.gen_expr(l);
                self.gen_expr(r);
            }
            ExprKind::Call(callee, args) => {
                let name = match &callee.kind {
                    ExprKind::Var(id) => self.sym_name(*id),
                    _ => {
                        self.emit("  ; internal error: indirect call");
                        return;
                    }
                };
                self.gen_call(&name, args);
            }
            ExprKind::PreInc(inner) => self.gen_inc_dec(inner, true, false),
            ExprKind::PreDec(inner) => self.gen_inc_dec(inner, false, false),
            ExprKind::PostInc(inner) => self.gen_inc_dec(inner, true, true),
            ExprKind::PostDec(inner) => self.gen_inc_dec(inner, false, true),
        }
    }

    fn gen_inc_dec(&mut self, inner: &Expr, inc: bool, post: bool) {
        let ty = inner.ty.clone();
        self.gen_addr(inner);
        self.emit("  mov rcx, rax");
        self.push();
        self.load_from(&ty, "rcx");
        if post {
            self.emit("  mov r11, rax");
        }
        let step = if ty.is_ptr() { ty.elem().size() } else { 1 };
        if ty.is_ptr() {
            // The pointer already occupies the full 64-bit register; applying an
            // integer conversion here would truncate it.
            if inc {
                self.emit(&format!("  add rax, {}", step));
            } else {
                self.emit(&format!("  sub rax, {}", step));
            }
        } else {
            if inc {
                self.emit("  add eax, 1");
            } else {
                self.emit("  sub eax, 1");
            }
            let int = Ty::Int;
            self.cast_value(&int, &ty);
        }
        self.pop("rcx");
        let ty2 = inner.ty.clone();
        self.store_to(&ty2, "rcx");
        if post {
            self.emit("  mov rax, r11");
        }
    }

    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) {
        match op {
            BinOp::LogAnd => {
                let l_false = self.new_label();
                let l_end = self.new_label();
                self.gen_expr(lhs);
                self.gen_test(lhs);
                self.emit(&format!("  je {}", l_false));
                self.gen_expr(rhs);
                self.gen_test(rhs);
                self.emit(&format!("  je {}", l_false));
                self.emit("  mov eax, 1");
                self.emit(&format!("  jmp {}", l_end));
                self.emit(&format!("{}:", l_false));
                self.emit("  mov eax, 0");
                self.emit(&format!("{}:", l_end));
                return;
            }
            BinOp::LogOr => {
                let l_true = self.new_label();
                let l_end = self.new_label();
                self.gen_expr(lhs);
                self.gen_test(lhs);
                self.emit(&format!("  jne {}", l_true));
                self.gen_expr(rhs);
                self.gen_test(rhs);
                self.emit(&format!("  jne {}", l_true));
                self.emit("  mov eax, 0");
                self.emit(&format!("  jmp {}", l_end));
                self.emit(&format!("{}:", l_true));
                self.emit("  mov eax, 1");
                self.emit(&format!("{}:", l_end));
                return;
            }
            _ => {}
        }

        self.gen_expr(lhs);
        self.push();
        self.gen_expr(rhs);
        self.pop("rcx");
        let lty = lhs.ty.clone();
        let rty = rhs.ty.clone();
        self.gen_arith(op, &lty, &rty);
    }

    /// Combines LHS (in RCX) and RHS (in RAX) for a non-short-circuit operator,
    /// leaving the result in RAX.  `lty`/`rty` are the operand types.
    fn gen_arith(&mut self, op: BinOp, lty: &Ty, rty: &Ty) {
        use BinOp::*;
        let esz = if lty.is_ptr() { lty.elem().size() } else { 1 };
        match op {
            Add if lty.is_ptr() => {
                self.widen_int();
                if esz != 1 {
                    self.emit(&format!("  imul rax, {}", esz));
                }
                self.emit("  add rax, rcx");
            }
            Sub if lty.is_ptr() && rty.is_ptr() => self.gen_ptr_diff(esz),
            Sub if lty.is_ptr() => {
                self.widen_int();
                if esz != 1 {
                    self.emit(&format!("  imul rax, {}", esz));
                }
                self.emit("  sub rcx, rax");
                self.emit("  mov rax, rcx");
            }
            Add => self.emit("  add eax, ecx"),
            Sub => {
                self.emit("  sub ecx, eax");
                self.emit("  mov eax, ecx");
            }
            Mul => self.emit("  imul eax, ecx"),
            Div => {
                self.emit("  mov r11d, eax");
                self.emit("  mov eax, ecx");
                self.emit("  cdq");
                self.emit("  idiv r11d");
            }
            Mod => {
                self.emit("  mov r11d, eax");
                self.emit("  mov eax, ecx");
                self.emit("  cdq");
                self.emit("  idiv r11d");
                self.emit("  mov eax, edx");
            }
            BitAnd => self.emit("  and eax, ecx"),
            BitOr => self.emit("  or eax, ecx"),
            BitXor => self.emit("  xor eax, ecx"),
            Shl | Shr => {
                self.emit("  mov r11d, eax");
                self.emit("  mov eax, ecx");
                self.emit("  mov ecx, r11d");
                if op == Shl {
                    self.emit("  shl eax, cl");
                } else {
                    self.emit("  sar eax, cl");
                }
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                if lty.is_ptr() {
                    self.emit("  cmp rcx, rax");
                } else {
                    self.emit("  cmp ecx, eax");
                }
                let cc = match op {
                    Eq => "sete",
                    Ne => "setne",
                    Lt => "setl",
                    Le => "setle",
                    Gt => "setg",
                    _ => "setge",
                };
                self.emit(&format!("  {} al", cc));
                self.emit("  movzx eax, al");
            }
            LogAnd | LogOr => unreachable!(),
        }
    }

    /// Pointer difference: RCX - RAX then divide by the element size.
    fn gen_ptr_diff(&mut self, elem_size: usize) {
        self.emit("  sub rcx, rax");
        self.emit("  mov rax, rcx");
        if elem_size > 1 {
            if elem_size.is_power_of_two() {
                self.emit(&format!("  sar rax, {}", elem_size.trailing_zeros()));
            } else {
                self.emit(&format!("  mov r11, {}", elem_size));
                self.emit("  cqo");
                self.emit("  idiv r11");
            }
        }
    }

    /// Emits a call following the Microsoft x64 calling convention.
    ///
    /// Layout built at the call site (ascending addresses):
    ///   [rsp+0 .. rsp+32)  shadow space
    ///   [rsp+32 + 8*i]     stack arguments 5..n
    fn gen_call(&mut self, name: &str, args: &[Expr]) {
        let n = args.len();
        let k = n.saturating_sub(4);
        let pad = if (self.depth + k + 4) % 2 != 0 { 8 } else { 0 };

        if pad != 0 {
            self.emit(&format!("  sub rsp, {}", pad));
            self.depth += 1;
        }
        for i in (4..n).rev() {
            self.gen_expr(&args[i]);
            self.push();
        }
        self.emit("  sub rsp, 32");
        self.depth += 4;
        let m = n.min(4);
        for i in (0..m).rev() {
            self.gen_expr(&args[i]);
            self.push();
        }
        for i in 0..m {
            self.pop(ARG_REGS[i]);
        }
        // Microsoft x64 requires AL to hold the number of vector registers used
        // by a variadic call; this backend never passes any.
        self.emit("  mov al, 0");
        self.emit(&format!("  call {}", name));
        let total = 32 + pad + 8 * k;
        self.emit(&format!("  add rsp, {}", total));
        self.depth -= total / 8;
    }

    // ------------------------------------------------------------ static data

    fn gen_static_data(&mut self) {
        let mut data_globals: Vec<usize> = Vec::new();
        let mut bss_globals: Vec<usize> = Vec::new();
        for (i, o) in self.objs.iter().enumerate() {
            if o.is_func || o.is_local {
                continue;
            }
            if o.init.is_some() {
                data_globals.push(i);
            } else {
                bss_globals.push(i);
            }
        }

        if !self.strings.is_empty() || !data_globals.is_empty() {
            self.emit("");
            self.emit(".data");
            for i in 0..self.strings.len() {
                let bytes = self.strings[i].clone();
                self.emit(&format!(".LC{}:", i));
                let mut line: Vec<String> = Vec::new();
                for b in bytes.iter() {
                    line.push(b.to_string());
                    if line.len() == 16 {
                        self.emit(&format!("  .byte {}", line.join(",")));
                        line.clear();
                    }
                }
                line.push("0".to_string());
                self.emit(&format!("  .byte {}", line.join(",")));
            }
            for &i in &data_globals {
                let name = self.sym_name(i);
                let align = self.objs[i].ty.align().min(8);
                let init = self.objs[i].init.clone().unwrap();
                self.emit(&format!(".balign {}", align.max(1)));
                self.emit(&format!(".globl {}", name));
                self.emit(&format!("{}:", name));
                for item in &init.items {
                    match item {
                        GInit::Byte(v) => self.emit(&format!("  .byte {}", *v as u8)),
                        GInit::Int(v) => self.emit(&format!("  .long {}", *v as i32)),
                        GInit::Quad(v) => self.emit(&format!("  .quad {}", *v as i64)),
                        GInit::Addr(s, off) => {
                            let sym = self.data_sym(s);
                            if *off == 0 {
                                self.emit(&format!("  .quad {}", sym));
                            } else if *off > 0 {
                                self.emit(&format!("  .quad {}+{}", sym, off));
                            } else {
                                self.emit(&format!("  .quad {}{}", sym, off));
                            }
                        }
                        GInit::Bytes(b) => {
                            let mut line: Vec<String> = Vec::new();
                            for x in b.iter() {
                                line.push(x.to_string());
                                if line.len() == 16 {
                                    self.emit(&format!("  .byte {}", line.join(",")));
                                    line.clear();
                                }
                            }
                            if !line.is_empty() {
                                self.emit(&format!("  .byte {}", line.join(",")));
                            }
                        }
                    }
                }
                if init.filled < init.size {
                    self.emit(&format!("  .zero {}", init.size - init.filled));
                }
            }
        }

        if !bss_globals.is_empty() {
            self.emit("");
            self.emit(".bss");
            for &i in &bss_globals {
                let name = self.sym_name(i);
                let size = self.objs[i].ty.size();
                let align = self.objs[i].ty.align().min(8);
                self.emit(&format!(".balign {}", align.max(1)));
                self.emit(&format!(".globl {}", name));
                self.emit(&format!("{}:", name));
                self.emit(&format!("  .zero {}", size));
            }
        }
    }
}
