//! Recursive-descent parser producing a fully typed AST.
//!
//! Scoping, type checking and implicit conversions all happen here so that the
//! code generator can assume a well-formed, explicitly-converted tree.

use crate::ast::*;
use crate::lexer::{Tok, Token};
use crate::types::{FuncTy, Ty};
use std::collections::HashMap;

const ALIGN_UP_MAX: usize = 16;

fn align_up(v: usize, a: usize) -> usize {
    if a <= 1 {
        v
    } else {
        (v + a - 1) / a * a
    }
}

/// Signatures of the runtime entry points the generated programs may call
/// without any header or prototype in the source.
fn builtins() -> Vec<(&'static str, FuncTy)> {
    let f = |ret: Ty, params: Vec<Ty>, variadic: bool| FuncTy {
        ret,
        params,
        variadic,
        has_proto: true,
    };
    let cptr = Ty::ptr_to(Ty::Char);
    vec![
        ("getchar", f(Ty::Int, vec![], false)),
        ("putchar", f(Ty::Int, vec![Ty::Int], false)),
        ("puts", f(Ty::Int, vec![cptr.clone()], false)),
        ("printf", f(Ty::Int, vec![cptr.clone()], true)),
        ("exit", f(Ty::Void, vec![Ty::Int], false)),
        ("atoi", f(Ty::Int, vec![cptr.clone()], false)),
        ("abs", f(Ty::Int, vec![Ty::Int], false)),
        ("strlen", f(Ty::Int, vec![cptr.clone()], false)),
        ("strcmp", f(Ty::Int, vec![cptr.clone(), cptr.clone()], false)),
        ("memset", f(cptr.clone(), vec![Ty::ptr_to(Ty::Void), Ty::Int, Ty::Int], false)),
        ("memcpy", f(cptr.clone(), vec![Ty::ptr_to(Ty::Void), Ty::ptr_to(Ty::Void), Ty::Int], false)),
        ("malloc", f(Ty::ptr_to(Ty::Void), vec![Ty::Int], false)),
        ("calloc", f(Ty::ptr_to(Ty::Void), vec![Ty::Int, Ty::Int], false)),
        ("realloc", f(Ty::ptr_to(Ty::Void), vec![Ty::ptr_to(Ty::Void), Ty::Int], false)),
        ("free", f(Ty::Void, vec![Ty::ptr_to(Ty::Void)], false)),
    ]
}

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    pub objs: Vec<Obj>,
    pub strings: Vec<Vec<u8>>,
    scopes: Vec<HashMap<String, ObjId>>,
    cur_func: Option<ObjId>,
    cur_ret: Ty,
    cur_offset: usize,
    cur_params: Vec<ObjId>,
    cur_variadic: bool,
    loop_depth: usize,
    /// set by `declspec` when it consumes a `static` keyword
    saw_static: bool,
    static_counter: usize,
}

impl Parser {
    pub fn new(toks: Vec<Token>) -> Parser {
        let mut p = Parser {
            toks,
            pos: 0,
            objs: Vec::new(),
            strings: Vec::new(),
            scopes: vec![HashMap::new()],
            cur_func: None,
            cur_ret: Ty::Void,
            cur_offset: 0,
            cur_params: Vec::new(),
            cur_variadic: false,
            loop_depth: 0,
            saw_static: false,
            static_counter: 0,
        };
        for (name, ft) in builtins() {
            let mut o = Obj::new(name.to_string(), Ty::Func(Box::new(ft)));
            o.is_func = true;
            p.objs.push(o);
            let id = p.objs.len() - 1;
            p.scopes[0].insert(name.to_string(), id);
        }
        p
    }

    // ---------------------------------------------------------------- tokens

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].tok
    }

    fn line(&self) -> usize {
        self.toks[self.pos].line
    }

    fn at(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Punct(p) if p == s)
    }

    fn at_kw(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Ident(p) if p == s)
    }

    fn consume(&mut self, s: &str) -> bool {
        if self.at(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn consume_kw(&mut self, s: &str) -> bool {
        if self.at_kw(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> Result<(), String> {
        if self.consume(s) {
            Ok(())
        } else {
            Err(format!(
                "line {}: expected '{}' but found {}",
                self.line(),
                s,
                describe(self.peek())
            ))
        }
    }

    fn is_type_start(&self) -> bool {
        matches!(self.peek(), Tok::Ident(s) if is_type_keyword(s))
    }

    fn is_type_start_at(&self, n: usize) -> bool {
        matches!(self.peek_at(n), Tok::Ident(s) if is_type_keyword(s))
    }

    // ---------------------------------------------------------------- scopes

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn insert_scope(&mut self, name: &str, id: ObjId) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), id);
    }

    fn lookup(&self, name: &str) -> Option<ObjId> {
        for s in self.scopes.iter().rev() {
            if let Some(&id) = s.get(name) {
                return Some(id);
            }
        }
        None
    }

    // ------------------------------------------------------------ utilities

    fn decay(&self, e: Expr) -> Expr {
        if e.ty.is_array() {
            let pt = Ty::ptr_to(e.ty.elem());
            Expr {
                kind: ExprKind::Cast(Box::new(e), pt.clone()),
                ty: pt,
            }
        } else {
            e
        }
    }

    fn cast_to(&self, e: Expr, ty: Ty) -> Expr {
        let e = self.decay(e);
        if e.ty == ty {
            return e;
        }
        Expr {
            kind: ExprKind::Cast(Box::new(e), ty.clone()),
            ty,
        }
    }

    fn is_lvalue(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Var(id) => {
                let o = &self.objs[*id];
                !o.is_func && !o.ty.is_array()
            }
            ExprKind::Unary(UnOp::Deref, _) => !e.ty.is_array() && !e.ty.is_func(),
            _ => false,
        }
    }

    fn var_expr(&self, id: ObjId) -> Expr {
        Expr {
            kind: ExprKind::Var(id),
            ty: self.objs[id].ty.clone(),
        }
    }

    fn mk_assign(&self, lhs: Expr, rhs: Expr) -> Result<Expr, String> {
        if !self.is_lvalue(&lhs) {
            return Err(format!("line {}: expression is not assignable", self.line()));
        }
        let lt = lhs.ty.clone();
        let rhs = self.cast_to(rhs, lt.clone());
        Ok(Expr {
            kind: ExprKind::Assign(Box::new(lhs), Box::new(rhs)),
            ty: lt,
        })
    }

    fn mk_binary(&self, op: BinOp, lhs: Expr, rhs: Expr) -> Result<Expr, String> {
        use BinOp::*;
        let lhs = self.decay(lhs);
        let rhs = self.decay(rhs);
        let lt = lhs.ty.clone();
        let rt = rhs.ty.clone();
        let ints = |l: Expr, r: Expr| -> (Expr, Expr) {
            (self.cast_to(l, Ty::Int), self.cast_to(r, Ty::Int))
        };
        let bin = |op: BinOp, l: Expr, r: Expr, ty: Ty| Expr {
            kind: ExprKind::Binary(op, Box::new(l), Box::new(r)),
            ty,
        };
        match op {
            Add => {
                if lt.is_ptr() && rt.is_integer() {
                    let r = self.cast_to(rhs, Ty::Int);
                    Ok(bin(Add, lhs, r, lt))
                } else if lt.is_integer() && rt.is_ptr() {
                    // Normalize to pointer-on-the-left.
                    let l = self.cast_to(lhs, Ty::Int);
                    Ok(bin(Add, rhs, l, rt))
                } else if lt.is_integer() && rt.is_integer() {
                    let (l, r) = ints(lhs, rhs);
                    Ok(bin(Add, l, r, Ty::Int))
                } else {
                    Err(type_err(self.line(), "invalid operands to +", &lt, &rt))
                }
            }
            Sub => {
                if lt.is_ptr() && rt.is_integer() {
                    let r = self.cast_to(rhs, Ty::Int);
                    Ok(bin(Sub, lhs, r, lt))
                } else if lt.is_ptr() && rt.is_ptr() {
                    Ok(bin(Sub, lhs, rhs, Ty::Int))
                } else if lt.is_integer() && rt.is_integer() {
                    let (l, r) = ints(lhs, rhs);
                    Ok(bin(Sub, l, r, Ty::Int))
                } else {
                    Err(type_err(self.line(), "invalid operands to -", &lt, &rt))
                }
            }
            Mul | Div | Mod | BitAnd | BitOr | BitXor | Shl | Shr => {
                if !lt.is_integer() || !rt.is_integer() {
                    return Err(type_err(self.line(), "invalid operands", &lt, &rt));
                }
                let (l, r) = ints(lhs, rhs);
                Ok(bin(op, l, r, Ty::Int))
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                if lt.is_ptr() || rt.is_ptr() {
                    let target = if lt.is_ptr() { lt.clone() } else { rt.clone() };
                    let l = self.cast_to(lhs, target.clone());
                    let r = self.cast_to(rhs, target);
                    Ok(bin(op, l, r, Ty::Int))
                } else if lt.is_integer() && rt.is_integer() {
                    let (l, r) = ints(lhs, rhs);
                    Ok(bin(op, l, r, Ty::Int))
                } else {
                    Err(type_err(self.line(), "invalid operands to comparison", &lt, &rt))
                }
            }
            LogAnd | LogOr => {
                if !(lt.is_integer() || lt.is_ptr()) || !(rt.is_integer() || rt.is_ptr()) {
                    return Err(type_err(self.line(), "invalid operands to logical operator", &lt, &rt));
                }
                Ok(bin(op, lhs, rhs, Ty::Int))
            }
        }
    }

    fn mk_compound(&self, op: BinOp, lhs: Expr, rhs: Expr) -> Result<Expr, String> {
        if !self.is_lvalue(&lhs) {
            return Err(format!("line {}: expression is not assignable", self.line()));
        }
        let lt = lhs.ty.clone();
        match op {
            BinOp::Add | BinOp::Sub if lt.is_ptr() => {
                let r = self.cast_to(rhs, Ty::Int);
                Ok(Expr {
                    kind: ExprKind::CompoundAssign(op, Box::new(lhs), Box::new(r)),
                    ty: lt,
                })
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::BitAnd
            | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                if !lt.is_integer() {
                    return Err(format!(
                        "line {}: compound assignment requires an arithmetic type",
                        self.line()
                    ));
                }
                let r = self.cast_to(rhs, Ty::Int);
                Ok(Expr {
                    kind: ExprKind::CompoundAssign(op, Box::new(lhs), Box::new(r)),
                    ty: lt,
                })
            }
            _ => Err(format!("line {}: unsupported compound assignment", self.line())),
        }
    }

    fn mk_inc_dec(&self, e: Expr, inc: bool, post: bool) -> Result<Expr, String> {
        if !self.is_lvalue(&e) {
            return Err(format!(
                "line {}: operand of '{}' must be a modifiable lvalue",
                self.line(),
                if inc { "++" } else { "--" }
            ));
        }
        let ty = e.ty.clone();
        let kind = match (post, inc) {
            (false, true) => ExprKind::PreInc(Box::new(e)),
            (false, false) => ExprKind::PreDec(Box::new(e)),
            (true, true) => ExprKind::PostInc(Box::new(e)),
            (true, false) => ExprKind::PostDec(Box::new(e)),
        };
        Ok(Expr { kind, ty })
    }

    fn mk_index(&self, base: Expr, idx: Expr) -> Result<Expr, String> {
        let base = self.decay(base);
        let idx = self.decay(idx);
        // C defines `E1[E2]` as `*((E1) + (E2))`, so either operand may be the
        // pointer: `2[primes]` is the same as `primes[2]`.
        let (base, idx) = if base.ty.is_ptr() {
            (base, idx)
        } else if idx.ty.is_ptr() {
            (idx, base)
        } else {
            return Err(format!(
                "line {}: subscripted value is not an array or pointer",
                self.line()
            ));
        };
        let idx = self.cast_to(idx, Ty::Int);
        let sum = self.mk_binary(BinOp::Add, base, idx)?;
        let elem = sum.ty.elem();
        Ok(Expr {
            kind: ExprKind::Unary(UnOp::Deref, Box::new(sum)),
            ty: elem,
        })
    }

    /// Builds the lvalue `base[i]` for initializer lowering.
    fn mk_index_lvalue(&self, base: Expr, i: usize, elem: &Ty) -> Result<Expr, String> {
        let idx = Expr::num(i as i64);
        let sum = self.mk_binary(BinOp::Add, self.decay(base), idx)?;
        Ok(Expr {
            kind: ExprKind::Unary(UnOp::Deref, Box::new(sum)),
            ty: elem.clone(),
        })
    }

    // ------------------------------------------------------------- top level

    pub fn parse_program(&mut self) -> Result<(), String> {
        while !matches!(self.peek(), Tok::Eof) {
            self.toplevel()?;
        }
        Ok(())
    }

    fn declspec(&mut self) -> Result<Ty, String> {
        let mut unsigned = false;
        let mut base: Option<Ty> = None;
        loop {
            let s = match self.peek() {
                Tok::Ident(s) => s.clone(),
                _ => break,
            };
            match s.as_str() {
                "const" | "volatile" | "register" | "auto" | "inline" => {
                    self.pos += 1;
                }
                "static" => {
                    self.pos += 1;
                    self.saw_static = true;
                }
                "signed" => {
                    self.pos += 1;
                }
                "unsigned" => {
                    self.pos += 1;
                    unsigned = true;
                }
                "int" => {
                    self.pos += 1;
                    base = Some(Ty::Int);
                    break;
                }
                "char" => {
                    self.pos += 1;
                    base = Some(if unsigned { Ty::UChar } else { Ty::Char });
                    break;
                }
                "void" => {
                    self.pos += 1;
                    base = Some(Ty::Void);
                    break;
                }
                _ => break,
            }
        }
        match base {
            Some(t) => Ok(t),
            // Bare `unsigned` / `signed` mean `unsigned int` / `int`; the only
            // 32-bit type available is `int`.
            None if unsigned => Ok(Ty::Int),
            None => Err(format!("line {}: expected a type specifier", self.line())),
        }
    }

    /// Parses `*`s, an optional name and array/function suffixes.
    fn declarator(
        &mut self,
        base: Ty,
    ) -> Result<(Option<String>, Ty, Option<Vec<(Option<String>, Ty)>>), String> {
        let mut ty = base;
        while self.consume("*") {
            while self.at_kw("const") || self.at_kw("volatile") {
                self.pos += 1;
            }
            ty = Ty::ptr_to(ty);
        }
        // A parenthesized declarator binds the pointer first: `int (*p)[4]` is a
        // pointer to an array of 4 ints.  Only the data-pointer form is
        // meaningful here, since function pointers are not supported.
        if self.at("(") && matches!(self.peek_at(1), Tok::Punct(p) if p == "*") {
            self.pos += 1;
            let (name, inner, _) = self.declarator(ty)?;
            self.expect(")")?;
            let (name, out, params) = self.type_suffix(inner, name)?;
            if out.is_func() {
                return Err(format!(
                    "line {}: function pointers are not supported",
                    self.line()
                ));
            }
            return Ok((name, out, params));
        }
        let name = match self.peek().clone() {
            Tok::Ident(s) if !is_type_keyword(&s) => {
                self.pos += 1;
                Some(s)
            }
            _ => None,
        };
        self.type_suffix(ty, name)
    }

    fn type_suffix(
        &mut self,
        ty: Ty,
        name: Option<String>,
    ) -> Result<(Option<String>, Ty, Option<Vec<(Option<String>, Ty)>>), String> {
        if self.consume("(") {
            let (params, variadic, has_proto) = self.func_params()?;
            let ft = Ty::Func(Box::new(FuncTy {
                ret: ty,
                params: params.iter().map(|(_, t)| t.clone()).collect(),
                variadic,
                has_proto,
            }));
            return Ok((name, ft, Some(params)));
        }
        // Collect every dimension first: `int a[2][3]` is an array of 2 arrays
        // of 3 ints, so the type has to be built from the innermost dimension
        // outwards.
        let mut dims: Vec<usize> = Vec::new();
        while self.consume("[") {
            let n = if self.at("]") {
                0
            } else {
                let e = self.cond_expr()?;
                let v = self.eval_const(&e)
                    .ok_or_else(|| format!("line {}: array size is not a constant", self.line()))?;
                if v < 0 {
                    return Err(format!("line {}: array size is negative", self.line()));
                }
                v as usize
            };
            self.expect("]")?;
            dims.push(n);
        }
        if dims.is_empty() {
            return Ok((name, ty, None));
        }
        if self.at("(") {
            return Err(format!(
                "line {}: a function cannot return an array",
                self.line()
            ));
        }
        let mut t = ty;
        for d in dims.iter().rev() {
            t = Ty::Array(Box::new(t), *d);
        }
        Ok((name, t, None))
    }

    fn func_params(&mut self) -> Result<(Vec<(Option<String>, Ty)>, bool, bool), String> {
        let mut params: Vec<(Option<String>, Ty)> = Vec::new();
        let mut variadic = false;
        if self.consume(")") {
            // `f()` — no prototype.
            return Ok((params, false, false));
        }
        if self.at_kw("void") && matches!(self.peek_at(1), Tok::Punct(p) if p == ")") {
            self.pos += 2;
            return Ok((params, false, true));
        }
        loop {
            if self.consume("...") {
                variadic = true;
                break;
            }
            let saved = self.saw_static;
            self.saw_static = false;
            let base = self.declspec()?;
            self.saw_static = saved;
            let (name, mut ty, _) = self.declarator(base)?;
            if ty.is_array() {
                ty = Ty::ptr_to(ty.elem());
            }
            params.push((name, ty));
            if self.consume(",") {
                continue;
            }
            break;
        }
        self.expect(")")?;
        Ok((params, variadic, true))
    }

    fn toplevel(&mut self) -> Result<(), String> {
        self.saw_static = false;
        if self.consume(";") {
            return Ok(());
        }
        let base = self.declspec()?;
        if self.consume(";") {
            return Ok(());
        }
        let (name, ty, params) = self.declarator(base.clone())?;
        let name = name.ok_or_else(|| {
            format!("line {}: declaration without a name is not allowed here", self.line())
        })?;
        if let Ty::Func(ft) = ty.clone() {
            let id = self.declare_func(&name, &ft);
            if self.at("{") {
                self.func_definition(id, &name, *ft, params.unwrap_or_default())?;
            } else {
                self.expect(";")?;
            }
            return Ok(());
        }
        // Global variable, possibly a comma-separated list.
        let mut cur_name = name;
        let mut cur_ty = ty;
        loop {
            let complete = self.complete_array_type(cur_ty)?;
            let id = self.new_global(&cur_name, complete)?;
            if self.consume("=") {
                self.global_init(id)?;
            }
            if self.consume(",") {
                let (n2, t2, _) = self.declarator(base.clone())?;
                cur_name = n2.ok_or_else(|| {
                    format!("line {}: declaration without a name", self.line())
                })?;
                cur_ty = t2;
                continue;
            }
            break;
        }
        self.expect(";")?;
        Ok(())
    }

    fn declare_func(&mut self, name: &str, ft: &FuncTy) -> ObjId {
        if let Some(&id) = self.scopes[0].get(name) {
            if self.objs[id].is_func {
                // A later, more complete declaration wins.
                if ft.has_proto || !self.objs[id].func_ty().map(|f| f.has_proto).unwrap_or(false) {
                    let o = &mut self.objs[id];
                    o.ty = Ty::Func(Box::new(ft.clone()));
                    o.variadic = ft.variadic;
                }
                return id;
            }
            return id;
        }
        let mut o = Obj::new(name.to_string(), Ty::Func(Box::new(ft.clone())));
        o.is_func = true;
        o.variadic = ft.variadic;
        self.objs.push(o);
        let id = self.objs.len() - 1;
        self.scopes[0].insert(name.to_string(), id);
        id
    }

    fn func_definition(
        &mut self,
        id: ObjId,
        name: &str,
        ft: FuncTy,
        params: Vec<(Option<String>, Ty)>,
    ) -> Result<(), String> {
        self.expect("{")?;
        let prev_func = self.cur_func;
        let prev_ret = self.cur_ret.clone();
        let prev_off = self.cur_offset;
        let prev_params = std::mem::take(&mut self.cur_params);
        let prev_variadic = self.cur_variadic;

        self.cur_func = Some(id);
        self.cur_ret = ft.ret.clone();
        self.cur_offset = 0;
        self.cur_variadic = ft.variadic;
        self.push_scope();

        for (pname, pty) in params {
            let pname = match pname {
                Some(n) => n,
                None => format!("<unnamed>"),
            };
            let pid = self.new_local(&pname, pty)?;
            self.insert_scope(&pname, pid);
            self.cur_params.push(pid);
        }

        let mut body = Vec::new();
        while !self.at("}") {
            if matches!(self.peek(), Tok::Eof) {
                return Err(format!("line {}: unexpected end of file in function body", self.line()));
            }
            if self.is_type_start() {
                let d = self.local_declaration()?;
                body.extend(d);
            } else {
                body.push(self.stmt()?);
            }
        }
        self.expect("}")?;
        self.pop_scope();

        let stack_size = align_up(self.cur_offset, ALIGN_UP_MAX);
        let params = std::mem::take(&mut self.cur_params);
        {
            let o = &mut self.objs[id];
            o.defined = true;
            o.body = Some(Stmt::Block(body));
            o.params = params;
            o.stack_size = stack_size;
            o.ty = Ty::Func(Box::new(ft));
            o.variadic = self.cur_variadic;
            o.name = name.to_string();
        }

        self.cur_func = prev_func;
        self.cur_ret = prev_ret;
        self.cur_offset = prev_off;
        self.cur_params = prev_params;
        self.cur_variadic = prev_variadic;
        Ok(())
    }

    fn new_local(&mut self, name: &str, ty: Ty) -> Result<ObjId, String> {
        let size = ty.size();
        if size == 0 {
            return Err(format!("line {}: object has incomplete type", self.line()));
        }
        let align = ty.align().min(8);
        self.cur_offset = align_up(self.cur_offset + size, align);
        let mut o = Obj::new(name.to_string(), ty);
        o.is_local = true;
        o.offset = self.cur_offset as i32;
        self.objs.push(o);
        Ok(self.objs.len() - 1)
    }

    fn new_global(&mut self, name: &str, ty: Ty) -> Result<ObjId, String> {
        if let Some(&id) = self.scopes[0].get(name) {
            if self.objs[id].is_func {
                return Err(format!("line {}: '{}' redeclared as a variable", self.line(), name));
            }
            if self.objs[id].ty == ty {
                return Ok(id);
            }
            return Err(format!("line {}: '{}' redeclared with a different type", self.line(), name));
        }
        let mut o = Obj::new(name.to_string(), ty);
        o.is_local = false;
        self.objs.push(o);
        let id = self.objs.len() - 1;
        self.scopes[0].insert(name.to_string(), id);
        Ok(id)
    }

    // --------------------------------------------------------- declarations

    fn local_declaration(&mut self) -> Result<Vec<Stmt>, String> {
        self.saw_static = false;
        let base = self.declspec()?;
        let is_static = self.saw_static;
        self.saw_static = false;
        let mut out = Vec::new();
        loop {
            let (name, ty, params) = self.declarator(base.clone())?;
            let name = name.ok_or_else(|| {
                format!("line {}: declaration without a name", self.line())
            })?;
            if let Ty::Func(_) = ty {
                // A block-scope prototype is accepted and discarded.
                if params.is_some() && !self.at("=") {
                    if !self.consume(",") {
                        break;
                    }
                    continue;
                }
                return Err(format!(
                    "line {}: nested function definitions are not supported",
                    self.line()
                ));
            }
            let ty = self.complete_array_type(ty)?;
            if is_static {
                self.static_counter += 1;
                let fname = self
                    .cur_func
                    .map(|f| self.objs[f].name.clone())
                    .unwrap_or_else(|| "fn".to_string());
                let mangled = format!("{}.{}.{}", fname, name, self.static_counter);
                let id = self.new_global(&mangled, ty)?;
                self.insert_scope(&name, id);
                if self.consume("=") {
                    self.global_init(id)?;
                }
            } else {
                let id = self.new_local(&name, ty)?;
                self.insert_scope(&name, id);
                if self.consume("=") {
                    out.extend(self.local_init(id)?);
                }
            }
            if self.consume(",") {
                continue;
            }
            break;
        }
        self.expect(";")?;
        Ok(out)
    }

    /// Resolves `T x[]` to a definite length using the initializer that follows.
    fn complete_array_type(&mut self, ty: Ty) -> Result<Ty, String> {
        if let Ty::Array(elem, 0) = &ty {
            if self.at("=") {
                let save = self.pos;
                self.pos += 1;
                let n = match self.peek().clone() {
                    Tok::Str(b) => b.len() + 1,
                    Tok::Punct(p) if p == "{" => self.count_brace_elements()?,
                    _ => 1,
                };
                self.pos = save;
                return Ok(Ty::Array(elem.clone(), n));
            }
            return Err(format!("line {}: array size missing", self.line()));
        }
        Ok(ty)
    }

    fn count_brace_elements(&mut self) -> Result<usize, String> {
        let mut i = self.pos;
        if !matches!(&self.toks[i].tok, Tok::Punct(p) if p == "{") {
            return Ok(1);
        }
        let mut depth = 0i32;
        let mut count = 0usize;
        let mut any = false;
        loop {
            if i >= self.toks.len() {
                return Err("unterminated initializer list".to_string());
            }
            match &self.toks[i].tok {
                Tok::Punct(p) if p == "{" => {
                    depth += 1;
                    if depth == 1 {
                        any = true;
                    }
                }
                Tok::Punct(p) if p == "}" => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Tok::Punct(p) if p == "," && depth == 1 => count += 1,
                Tok::Eof => return Err("unterminated initializer list".to_string()),
                _ => {}
            }
            i += 1;
        }
        Ok(if any { count + 1 } else { 0 })
    }

    fn local_init(&mut self, id: ObjId) -> Result<Vec<Stmt>, String> {
        let ty = self.objs[id].ty.clone();
        let mut stmts = Vec::new();
        let base = self.var_expr(id);
        if ty.is_array() {
            self.init_array_local(base, &ty, &mut stmts)?;
            return Ok(stmts);
        }
        let e = self.assign_expr()?;
        let a = self.mk_assign(base, e)?;
        stmts.push(Stmt::Expr(a));
        Ok(stmts)
    }

    fn init_array_local(
        &mut self,
        base: Expr,
        ty: &Ty,
        stmts: &mut Vec<Stmt>,
    ) -> Result<(), String> {
        let (elem, n) = match ty {
            Ty::Array(e, n) => ((**e).clone(), *n),
            _ => return Err("internal: expected array type".to_string()),
        };
        if let Tok::Str(bytes) = self.peek().clone() {
            if elem.size() != 1 {
                return Err(format!(
                    "line {}: a string literal can only initialize a char array",
                    self.line()
                ));
            }
            self.pos += 1;
            for i in 0..n {
                let idx = self.mk_index_lvalue(base.clone(), i, &elem)?;
                let b = if i < bytes.len() { bytes[i] } else { 0 };
                let v = self.cast_to(Expr::num(b as i64), elem.clone());
                let a = self.mk_assign(idx, v)?;
                stmts.push(Stmt::Expr(a));
            }
            return Ok(());
        }
        self.expect("{")?;
        let mut i = 0usize;
        while !self.at("}") {
            if i >= n {
                return Err(format!("line {}: too many initializers", self.line()));
            }
            let idx = self.mk_index_lvalue(base.clone(), i, &elem)?;
            if elem.is_array() {
                self.init_array_local(idx, &elem, stmts)?;
            } else {
                let e = self.assign_expr()?;
                let a = self.mk_assign(idx, e)?;
                stmts.push(Stmt::Expr(a));
            }
            i += 1;
            if !self.consume(",") {
                break;
            }
        }
        self.expect("}")?;
        for j in i..n {
            let idx = self.mk_index_lvalue(base.clone(), j, &elem)?;
            self.zero_fill_local(idx, &elem, stmts)?;
        }
        Ok(())
    }

    fn zero_fill_local(&self, base: Expr, ty: &Ty, stmts: &mut Vec<Stmt>) -> Result<(), String> {
        if let Ty::Array(elem, n) = ty {
            for j in 0..*n {
                let idx = self.mk_index_lvalue(base.clone(), j, elem)?;
                self.zero_fill_local(idx, elem, stmts)?;
            }
            return Ok(());
        }
        let a = self.mk_assign(base, Expr::num(0))?;
        stmts.push(Stmt::Expr(a));
        Ok(())
    }

    // ------------------------------------------------------ global init data

    fn global_init(&mut self, id: ObjId) -> Result<(), String> {
        let ty = self.objs[id].ty.clone();
        let mut items: Vec<GInit> = Vec::new();
        let mut filled = 0usize;
        match &ty {
            Ty::Array(elem, n) => {
                let elem = (**elem).clone();
                if let Tok::Str(bytes) = self.peek().clone() {
                    if elem.size() != 1 {
                        return Err(format!(
                            "line {}: a string literal can only initialize a char array",
                            self.line()
                        ));
                    }
                    self.pos += 1;
                    let mut data = bytes.clone();
                    data.push(0);
                    data.resize(*n, 0);
                    data.truncate(*n);
                    filled = data.len();
                    items.push(GInit::Bytes(data));
                } else {
                    self.expect("{")?;
                    let mut i = 0usize;
                    while !self.at("}") {
                        if i >= *n {
                            return Err(format!("line {}: too many initializers", self.line()));
                        }
                        self.global_array_elem(&elem, &mut items, &mut filled)?;
                        i += 1;
                        if !self.consume(",") {
                            break;
                        }
                    }
                    self.expect("}")?;
                }
            }
            Ty::Ptr(_) => {
                if let Tok::Str(bytes) = self.peek().clone() {
                    self.pos += 1;
                    let idx = self.strings.len();
                    self.strings.push(bytes);
                    items.push(GInit::Addr(format!(".LC{}", idx), 0));
                    filled = 8;
                } else {
                    let e = self.cond_expr()?;
                    match self.const_ptr(&e) {
                        Some((s, off)) if !s.is_empty() => items.push(GInit::Addr(s, off)),
                        Some((_, v)) => items.push(GInit::Quad(v)),
                        None => {
                            return Err(format!(
                                "line {}: initializer for a pointer is not a constant",
                                self.line()
                            ))
                        }
                    }
                    filled = 8;
                }
            }
            Ty::Char | Ty::UChar => {
                let e = self.cond_expr()?;
                let v = self
                    .eval_const(&e)
                    .ok_or_else(|| format!("line {}: initializer is not a constant", self.line()))?;
                items.push(GInit::Byte(v));
                filled = 1;
            }
            Ty::Int => {
                let e = self.cond_expr()?;
                let v = self
                    .eval_const(&e)
                    .ok_or_else(|| format!("line {}: initializer is not a constant", self.line()))?;
                items.push(GInit::Int(v));
                filled = 4;
            }
            _ => {
                return Err(format!(
                    "line {}: cannot statically initialize an object of type {}",
                    self.line(),
                    ty
                ))
            }
        }
        let size = ty.size();
        self.objs[id].init = Some(GlobalInit {
            items,
            filled,
            size,
        });
        Ok(())
    }

    fn global_array_elem(
        &mut self,
        elem: &Ty,
        items: &mut Vec<GInit>,
        filled: &mut usize,
    ) -> Result<(), String> {
        if let Ty::Array(inner, n) = elem {
            let inner = (**inner).clone();
            if let Tok::Str(bytes) = self.peek().clone() {
                if inner.size() != 1 {
                    return Err(format!("line {}: bad string initializer", self.line()));
                }
                self.pos += 1;
                let mut data = bytes.clone();
                data.push(0);
                data.resize(*n, 0);
                data.truncate(*n);
                *filled += data.len();
                items.push(GInit::Bytes(data));
                return Ok(());
            }
            self.expect("{")?;
            let mut i = 0usize;
            while !self.at("}") {
                if i >= *n {
                    return Err(format!("line {}: too many initializers", self.line()));
                }
                self.global_array_elem(&inner, items, filled)?;
                i += 1;
                if !self.consume(",") {
                    break;
                }
            }
            self.expect("}")?;
            for _ in i..*n {
                let sz = inner.size();
                items.push(GInit::Bytes(vec![0u8; sz]));
                *filled += sz;
            }
            return Ok(());
        }
        match elem {
            Ty::Char | Ty::UChar => {
                let e = self.cond_expr()?;
                let v = self.eval_const(&e).ok_or_else(|| {
                    format!("line {}: initializer is not a constant", self.line())
                })?;
                items.push(GInit::Byte(v));
                *filled += 1;
            }
            Ty::Int => {
                let e = self.cond_expr()?;
                let v = self.eval_const(&e).ok_or_else(|| {
                    format!("line {}: initializer is not a constant", self.line())
                })?;
                items.push(GInit::Int(v));
                *filled += 4;
            }
            Ty::Ptr(_) => {
                if let Tok::Str(bytes) = self.peek().clone() {
                    self.pos += 1;
                    let idx = self.strings.len();
                    self.strings.push(bytes);
                    items.push(GInit::Addr(format!(".LC{}", idx), 0));
                    *filled += 8;
                } else {
                    let e = self.cond_expr()?;
                    match self.const_ptr(&e) {
                        Some((s, off)) if !s.is_empty() => items.push(GInit::Addr(s, off)),
                        Some((_, v)) => items.push(GInit::Quad(v)),
                        None => {
                            return Err(format!(
                                "line {}: initializer is not a constant",
                                self.line()
                            ))
                        }
                    }
                    *filled += 8;
                }
            }
            _ => {
                return Err(format!(
                    "line {}: unsupported initializer element type",
                    self.line()
                ))
            }
        }
        Ok(())
    }

    /// Statically known address of an lvalue, as `(symbol, byte offset)`.
    fn const_lvalue_addr(&self, e: &Expr) -> Option<(String, i64)> {
        match &e.kind {
            ExprKind::Var(id) => {
                if self.objs[*id].is_local {
                    None
                } else {
                    Some((self.objs[*id].name.clone(), 0))
                }
            }
            ExprKind::Unary(UnOp::Deref, inner) => self.const_ptr_value(inner),
            _ => None,
        }
    }

    /// Statically known value of a pointer expression, as `(symbol, offset)`.
    fn const_ptr_value(&self, e: &Expr) -> Option<(String, i64)> {
        match &e.kind {
            ExprKind::Cast(inner, _) => self.const_ptr_value(inner),
            ExprKind::Binary(BinOp::Add, l, r) => {
                if let Some((s, off)) = self.const_ptr_value(l) {
                    let k = self.eval_const(r)?;
                    let esz = l.ty.elem().size() as i64;
                    Some((s, off + k * esz))
                } else {
                    let (s, off) = self.const_ptr_value(r)?;
                    let k = self.eval_const(l)?;
                    let esz = r.ty.elem().size() as i64;
                    Some((s, off + k * esz))
                }
            }
            ExprKind::Binary(BinOp::Sub, l, r) => {
                let (s, off) = self.const_ptr_value(l)?;
                let k = self.eval_const(r)?;
                let esz = l.ty.elem().size() as i64;
                Some((s, off - k * esz))
            }
            _ => self.const_ptr(e),
        }
    }

    /// Statically known pointer value: an address constant, or an integer.
    fn const_ptr(&self, e: &Expr) -> Option<(String, i64)> {
        match &e.kind {
            ExprKind::Unary(UnOp::Addr, inner) => self.const_lvalue_addr(inner),
            // A bare array or function designator decays to its own address.
            ExprKind::Var(id) => {
                let o = &self.objs[*id];
                if !o.is_local && (o.ty.is_array() || o.ty.is_func()) {
                    Some((o.name.clone(), 0))
                } else {
                    None
                }
            }
            ExprKind::Str(i) => Some((format!(".LC{}", i), 0)),
            ExprKind::Cast(inner, _) => self.const_ptr(inner),
            ExprKind::Binary(BinOp::Add, _, _) | ExprKind::Binary(BinOp::Sub, _, _) => {
                self.const_ptr_value(e)
            }
            _ => {
                let v = self.eval_const(e)?;
                Some((String::new(), v))
            }
        }
    }

    fn eval_const(&self, e: &Expr) -> Option<i64> {
        match &e.kind {
            ExprKind::Num(v) => Some(*v),
            ExprKind::Cast(inner, _) => self.eval_const(inner),
            ExprKind::Unary(op, inner) => {
                let v = self.eval_const(inner)?;
                Some(match op {
                    UnOp::Neg => v.wrapping_neg(),
                    UnOp::Not => (v == 0) as i64,
                    UnOp::BitNot => !v,
                    _ => return None,
                })
            }
            ExprKind::Binary(op, l, r) => {
                let a = self.eval_const(l)?;
                let b = self.eval_const(r)?;
                use BinOp::*;
                Some(match op {
                    Add => a.wrapping_add(b),
                    Sub => a.wrapping_sub(b),
                    Mul => a.wrapping_mul(b),
                    Div => {
                        if b == 0 {
                            return None;
                        }
                        a.wrapping_div(b)
                    }
                    Mod => {
                        if b == 0 {
                            return None;
                        }
                        a.wrapping_rem(b)
                    }
                    Eq => (a == b) as i64,
                    Ne => (a != b) as i64,
                    Lt => (a < b) as i64,
                    Le => (a <= b) as i64,
                    Gt => (a > b) as i64,
                    Ge => (a >= b) as i64,
                    BitAnd => a & b,
                    BitOr => a | b,
                    BitXor => a ^ b,
                    Shl => a.wrapping_shl(b as u32),
                    Shr => a.wrapping_shr(b as u32),
                    LogAnd => ((a != 0) && (b != 0)) as i64,
                    LogOr => ((a != 0) || (b != 0)) as i64,
                })
            }
            ExprKind::Cond(c, t, f) => {
                if self.eval_const(c)? != 0 {
                    self.eval_const(t)
                } else {
                    self.eval_const(f)
                }
            }
            ExprKind::Comma(_, r) => self.eval_const(r),
            _ => None,
        }
    }

    // ----------------------------------------------------------- statements

    fn stmt(&mut self) -> Result<Stmt, String> {
        if self.at("{") {
            return self.compound_stmt();
        }
        if self.consume_kw("if") {
            self.expect("(")?;
            let tmp = self.expr()?;
            let c = self.cond_value(tmp);
            self.expect(")")?;
            let then = Box::new(self.stmt()?);
            let els = if self.consume_kw("else") {
                Some(Box::new(self.stmt()?))
            } else {
                None
            };
            return Ok(Stmt::If(c, then, els));
        }
        if self.consume_kw("while") {
            self.expect("(")?;
            let tmp = self.expr()?;
            let c = self.cond_value(tmp);
            self.expect(")")?;
            self.loop_depth += 1;
            let body = self.stmt()?;
            self.loop_depth -= 1;
            return Ok(Stmt::While(c, Box::new(body)));
        }
        if self.consume_kw("do") {
            self.loop_depth += 1;
            let body = self.stmt()?;
            self.loop_depth -= 1;
            if !self.consume_kw("while") {
                return Err(format!("line {}: expected 'while' after do-body", self.line()));
            }
            self.expect("(")?;
            let tmp = self.expr()?;
            let c = self.cond_value(tmp);
            self.expect(")")?;
            self.expect(";")?;
            return Ok(Stmt::DoWhile(Box::new(body), c));
        }
        if self.consume_kw("for") {
            self.expect("(")?;
            self.push_scope();
            let init = if self.consume(";") {
                Stmt::Empty
            } else if self.is_type_start() {
                let decls = self.local_declaration()?;
                Stmt::Block(decls)
            } else {
                let e = self.expr()?;
                self.expect(";")?;
                Stmt::Expr(e)
            };
            let cond = if self.at(";") {
                None
            } else {
                let tmp = self.expr()?;
                Some(self.cond_value(tmp))
            };
            self.expect(";")?;
            let step = if self.at(")") {
                None
            } else {
                Some(self.expr()?)
            };
            self.expect(")")?;
            self.loop_depth += 1;
            let body = self.stmt()?;
            self.loop_depth -= 1;
            self.pop_scope();
            return Ok(Stmt::For(Box::new(init), cond, step, Box::new(body)));
        }
        if self.consume_kw("break") {
            if self.loop_depth == 0 {
                return Err(format!("line {}: 'break' outside of a loop", self.line()));
            }
            self.expect(";")?;
            return Ok(Stmt::Break);
        }
        if self.consume_kw("continue") {
            if self.loop_depth == 0 {
                return Err(format!("line {}: 'continue' outside of a loop", self.line()));
            }
            self.expect(";")?;
            return Ok(Stmt::Continue);
        }
        if self.consume_kw("return") {
            if self.consume(";") {
                return Ok(Stmt::Return(None));
            }
            let e = self.expr()?;
            self.expect(";")?;
            return Ok(Stmt::Return(Some(e)));
        }
        if self.consume(";") {
            return Ok(Stmt::Empty);
        }
        let e = self.expr()?;
        self.expect(";")?;
        Ok(Stmt::Expr(e))
    }

    fn compound_stmt(&mut self) -> Result<Stmt, String> {
        self.expect("{")?;
        self.push_scope();
        let mut stmts = Vec::new();
        while !self.at("}") {
            if matches!(self.peek(), Tok::Eof) {
                return Err(format!("line {}: unexpected end of file", self.line()));
            }
            if self.is_type_start() {
                let d = self.local_declaration()?;
                stmts.extend(d);
            } else {
                stmts.push(self.stmt()?);
            }
        }
        self.expect("}")?;
        self.pop_scope();
        Ok(Stmt::Block(stmts))
    }

    /// Conditions are compared against zero; arrays decay, everything else
    /// keeps its type so the backend can pick a 32- or 64-bit test.
    fn cond_value(&self, e: Expr) -> Expr {
        self.decay(e)
    }

    // ---------------------------------------------------------- expressions

    fn expr(&mut self) -> Result<Expr, String> {
        let mut e = self.assign_expr()?;
        while self.consume(",") {
            let r = self.assign_expr()?;
            let ty = r.ty.clone();
            e = Expr {
                kind: ExprKind::Comma(Box::new(e), Box::new(r)),
                ty,
            };
        }
        Ok(e)
    }

    fn assign_expr(&mut self) -> Result<Expr, String> {
        let lhs = self.cond_expr()?;
        let op = match self.peek() {
            Tok::Punct(p) => match p.as_str() {
                "=" => Some(None),
                "+=" => Some(Some(BinOp::Add)),
                "-=" => Some(Some(BinOp::Sub)),
                "*=" => Some(Some(BinOp::Mul)),
                "/=" => Some(Some(BinOp::Div)),
                "%=" => Some(Some(BinOp::Mod)),
                "&=" => Some(Some(BinOp::BitAnd)),
                "|=" => Some(Some(BinOp::BitOr)),
                "^=" => Some(Some(BinOp::BitXor)),
                "<<=" => Some(Some(BinOp::Shl)),
                ">>=" => Some(Some(BinOp::Shr)),
                _ => None,
            },
            _ => None,
        };
        let op = match op {
            None => return Ok(lhs),
            Some(o) => o,
        };
        self.pos += 1;
        let rhs = self.assign_expr()?;
        match op {
            None => self.mk_assign(lhs, rhs),
            Some(b) => self.mk_compound(b, lhs, rhs),
        }
    }

    fn cond_expr(&mut self) -> Result<Expr, String> {
        let c = self.logor_expr()?;
        if !self.consume("?") {
            return Ok(c);
        }
        let t = self.expr()?;
        self.expect(":")?;
        let f = self.cond_expr()?;
        let c = self.cond_value(c);
        let (t, f, ty) = if t.ty.is_ptr() || f.ty.is_ptr() {
            let target = if t.ty.is_ptr() { t.ty.clone() } else { f.ty.clone() };
            (
                self.cast_to(t, target.clone()),
                self.cast_to(f, target.clone()),
                target,
            )
        } else {
            (
                self.cast_to(t, Ty::Int),
                self.cast_to(f, Ty::Int),
                Ty::Int,
            )
        };
        Ok(Expr {
            kind: ExprKind::Cond(Box::new(c), Box::new(t), Box::new(f)),
            ty,
        })
    }

    fn logor_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.logand_expr()?;
        while self.consume("||") {
            let r = self.logand_expr()?;
            e = self.mk_binary(BinOp::LogOr, e, r)?;
        }
        Ok(e)
    }

    fn logand_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.bitor_expr()?;
        while self.consume("&&") {
            let r = self.bitor_expr()?;
            e = self.mk_binary(BinOp::LogAnd, e, r)?;
        }
        Ok(e)
    }

    fn bitor_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.bitxor_expr()?;
        while self.at("|") {
            self.pos += 1;
            let r = self.bitxor_expr()?;
            e = self.mk_binary(BinOp::BitOr, e, r)?;
        }
        Ok(e)
    }

    fn bitxor_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.bitand_expr()?;
        while self.at("^") {
            self.pos += 1;
            let r = self.bitand_expr()?;
            e = self.mk_binary(BinOp::BitXor, e, r)?;
        }
        Ok(e)
    }

    fn bitand_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.equality_expr()?;
        while self.at("&") {
            self.pos += 1;
            let r = self.equality_expr()?;
            e = self.mk_binary(BinOp::BitAnd, e, r)?;
        }
        Ok(e)
    }

    fn equality_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.relational_expr()?;
        loop {
            let op = if self.consume("==") {
                BinOp::Eq
            } else if self.consume("!=") {
                BinOp::Ne
            } else {
                break;
            };
            let r = self.relational_expr()?;
            e = self.mk_binary(op, e, r)?;
        }
        Ok(e)
    }

    fn relational_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.shift_expr()?;
        loop {
            let op = if self.consume("<=") {
                BinOp::Le
            } else if self.consume(">=") {
                BinOp::Ge
            } else if self.consume("<") {
                BinOp::Lt
            } else if self.consume(">") {
                BinOp::Gt
            } else {
                break;
            };
            let r = self.shift_expr()?;
            e = self.mk_binary(op, e, r)?;
        }
        Ok(e)
    }

    fn shift_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.additive_expr()?;
        loop {
            let op = if self.consume("<<") {
                BinOp::Shl
            } else if self.consume(">>") {
                BinOp::Shr
            } else {
                break;
            };
            let r = self.additive_expr()?;
            e = self.mk_binary(op, e, r)?;
        }
        Ok(e)
    }

    fn additive_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.mul_expr()?;
        loop {
            let op = if self.consume("+") {
                BinOp::Add
            } else if self.consume("-") {
                BinOp::Sub
            } else {
                break;
            };
            let r = self.mul_expr()?;
            e = self.mk_binary(op, e, r)?;
        }
        Ok(e)
    }

    fn mul_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.unary_expr()?;
        loop {
            let op = if self.consume("*") {
                BinOp::Mul
            } else if self.consume("/") {
                BinOp::Div
            } else if self.consume("%") {
                BinOp::Mod
            } else {
                break;
            };
            let r = self.unary_expr()?;
            e = self.mk_binary(op, e, r)?;
        }
        Ok(e)
    }

    fn unary_expr(&mut self) -> Result<Expr, String> {
        if self.consume("+") {
            let e = self.unary_expr()?;
            let e = self.decay(e);
            if !e.ty.is_integer() {
                return Err(format!("line {}: unary '+' requires an arithmetic operand", self.line()));
            }
            return Ok(self.cast_to(e, Ty::Int));
        }
        if self.consume("-") {
            let e = self.unary_expr()?;
            let e = self.decay(e);
            if !e.ty.is_integer() {
                return Err(format!("line {}: unary '-' requires an arithmetic operand", self.line()));
            }
            let e = self.cast_to(e, Ty::Int);
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Neg, Box::new(e)),
                ty: Ty::Int,
            });
        }
        if self.consume("!") {
            let tmp = self.unary_expr()?;
            let e = self.decay(tmp);
            if !(e.ty.is_integer() || e.ty.is_ptr()) {
                return Err(format!("line {}: unary '!' requires a scalar operand", self.line()));
            }
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Not, Box::new(e)),
                ty: Ty::Int,
            });
        }
        if self.consume("~") {
            let e = self.unary_expr()?;
            let e = self.decay(e);
            if !e.ty.is_integer() {
                return Err(format!("line {}: unary '~' requires an integer operand", self.line()));
            }
            let e = self.cast_to(e, Ty::Int);
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::BitNot, Box::new(e)),
                ty: Ty::Int,
            });
        }
        if self.consume("&") {
            let e = self.unary_expr()?;
            if !self.is_lvalue(&e) && !matches!(e.kind, ExprKind::Unary(UnOp::Deref, _)) {
                return Err(format!("line {}: cannot take the address of this expression", self.line()));
            }
            let ty = Ty::ptr_to(e.ty.clone());
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Addr, Box::new(e)),
                ty,
            });
        }
        if self.at("*") {
            self.pos += 1;
            let tmp = self.unary_expr()?;
            let e = self.decay(tmp);
            if !e.ty.is_ptr() {
                return Err(format!("line {}: cannot dereference a non-pointer value", self.line()));
            }
            let ty = e.ty.elem();
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Deref, Box::new(e)),
                ty,
            });
        }
        if self.consume("++") {
            let e = self.unary_expr()?;
            return self.mk_inc_dec(e, true, false);
        }
        if self.consume("--") {
            let e = self.unary_expr()?;
            return self.mk_inc_dec(e, false, false);
        }
        if self.at_kw("sizeof") {
            self.pos += 1;
            if self.consume("(") {
                if self.is_type_start() {
                    let ty = self.type_name()?;
                    self.expect(")")?;
                    return Ok(Expr::num(ty.size() as i64));
                }
                let e = self.expr()?;
                self.expect(")")?;
                return Ok(Expr::num(e.ty.size() as i64));
            }
            let e = self.unary_expr()?;
            return Ok(Expr::num(e.ty.size() as i64));
        }
        self.postfix_expr()
    }

    fn type_name(&mut self) -> Result<Ty, String> {
        let base = self.declspec()?;
        let (_, ty, _) = self.declarator(base)?;
        Ok(ty)
    }

    fn postfix_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.primary_expr()?;
        loop {
            if self.consume("[") {
                let idx = self.expr()?;
                self.expect("]")?;
                e = self.mk_index(e, idx)?;
                continue;
            }
            if self.at("(") {
                self.pos += 1;
                let args = self.call_args()?;
                self.expect(")")?;
                e = self.mk_call(e, args)?;
                continue;
            }
            if self.consume("++") {
                e = self.mk_inc_dec(e, true, true)?;
                continue;
            }
            if self.consume("--") {
                e = self.mk_inc_dec(e, false, true)?;
                continue;
            }
            break;
        }
        if e.ty.is_func() {
            return Err(format!(
                "line {}: a function name can only be used as the target of a call",
                self.line()
            ));
        }
        Ok(e)
    }

    fn call_args(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if self.at(")") {
            return Ok(args);
        }
        loop {
            args.push(self.assign_expr()?);
            if self.consume(",") {
                continue;
            }
            break;
        }
        Ok(args)
    }

    fn mk_call(&self, callee: Expr, args: Vec<Expr>) -> Result<Expr, String> {
        let ft = match &callee.ty {
            Ty::Func(f) => (**f).clone(),
            _ => {
                return Err(format!(
                    "line {}: called object is not a function",
                    self.line()
                ))
            }
        };
        if ft.has_proto {
            let n = args.len();
            if n < ft.params.len() || (!ft.variadic && n > ft.params.len()) {
                return Err(format!(
                    "line {}: wrong number of arguments (expected {}, got {})",
                    self.line(),
                    ft.params.len(),
                    n
                ));
            }
        }
        let mut newargs = Vec::with_capacity(args.len());
        for (i, a) in args.into_iter().enumerate() {
            let a = self.decay(a);
            if i < ft.params.len() {
                newargs.push(self.cast_to(a, ft.params[i].clone()));
            } else {
                // Default argument promotions for the variadic tail.
                newargs.push(self.cast_to(a, Ty::Int));
            }
        }
        Ok(Expr {
            kind: ExprKind::Call(Box::new(callee), newargs),
            ty: ft.ret.clone(),
        })
    }

    fn primary_expr(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Tok::Num(v) => {
                self.pos += 1;
                Ok(Expr::num(v))
            }
            Tok::Str(bytes) => {
                self.pos += 1;
                let idx = self.strings.len();
                let len = bytes.len() + 1;
                self.strings.push(bytes);
                Ok(Expr {
                    kind: ExprKind::Str(idx),
                    ty: Ty::Array(Box::new(Ty::Char), len),
                })
            }
            Tok::Ident(name) => {
                self.pos += 1;
                if let Some(id) = self.lookup(&name) {
                    let e = self.var_expr(id);
                    if e.ty.is_func() && !self.at("(") {
                        return Err(format!(
                            "line {}: '{}' is a function and must be called",
                            self.line(),
                            name
                        ));
                    }
                    return Ok(e);
                }
                if self.at("(") {
                    // C89-style implicit declaration.
                    let ft = FuncTy {
                        ret: Ty::Int,
                        params: Vec::new(),
                        variadic: true,
                        has_proto: false,
                    };
                    let id = self.declare_func(&name, &ft);
                    return Ok(self.var_expr(id));
                }
                Err(format!("line {}: undeclared identifier '{}'", self.line(), name))
            }
            Tok::Punct(p) if p == "(" => {
                if self.is_type_start_at(1) {
                    self.pos += 1;
                    let ty = self.type_name()?;
                    self.expect(")")?;
                    let operand = self.unary_expr()?;
                    return Ok(self.cast_to(operand, ty));
                }
                self.pos += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            other => Err(format!(
                "line {}: expected an expression but found {}",
                self.line(),
                describe(&other)
            )),
        }
    }
}

fn is_type_keyword(s: &str) -> bool {
    matches!(
        s,
        "void"
            | "char"
            | "int"
            | "signed"
            | "unsigned"
            | "const"
            | "volatile"
            | "static"
            | "register"
            | "auto"
            | "inline"
    )
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("'{}'", s),
        Tok::Num(v) => format!("{}", v),
        Tok::Str(_) => "a string literal".to_string(),
        Tok::Punct(p) => format!("'{}'", p),
        Tok::Eof => "end of file".to_string(),
    }
}

fn type_err(line: usize, msg: &str, a: &Ty, b: &Ty) -> String {
    format!("line {}: {} ({} and {})", line, msg, a, b)
}
