//! Typed AST plus the object table.
//!
//! Every expression node carries its resolved type, so the code generator never
//! has to re-derive one.  Objects (globals, locals, parameters, functions) live
//! in one arena addressed by `ObjId`.

use crate::types::{FuncTy, Ty};

pub type ObjId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    LogAnd,
    LogOr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    Addr,
    Deref,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
}

impl Expr {
    pub fn num(v: i64) -> Expr {
        Expr {
            kind: ExprKind::Num(v),
            ty: Ty::Int,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Num(i64),
    /// Index into the string-literal pool.
    Str(usize),
    Var(ObjId),
    Assign(Box<Expr>, Box<Expr>),
    /// `lhs op= rhs` — the left-hand address is evaluated exactly once.
    CompoundAssign(BinOp, Box<Expr>, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Comma(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Cast(Box<Expr>, Ty),
    PostInc(Box<Expr>),
    PostDec(Box<Expr>),
    PreInc(Box<Expr>),
    PreDec(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Expr(Expr),
    Return(Option<Expr>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    For(Box<Stmt>, Option<Expr>, Option<Expr>, Box<Stmt>),
    Block(Vec<Stmt>),
    Break,
    Continue,
    Empty,
}

/// One directive's worth of static initializer data.
#[derive(Clone, Debug)]
pub enum GInit {
    Byte(i64),
    Int(i64),
    Quad(i64),
    /// Relocatable 8-byte pointer: a symbol (string label or global) plus a
    /// byte offset, for initializers such as `&table[2]`.
    Addr(String, i64),
    /// Raw byte run (used for `char[] = "..."`).
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Default)]
pub struct GlobalInit {
    pub items: Vec<GInit>,
    /// Number of bytes covered by `items`.
    pub filled: usize,
    /// Total object size; the tail is zero-filled.
    pub size: usize,
}

#[derive(Clone, Debug)]
pub struct Obj {
    pub name: String,
    pub ty: Ty,
    /// true for parameters and block-scope variables
    pub is_local: bool,
    pub is_func: bool,
    /// rbp-relative displacement for locals (address is `[rbp - offset]`).
    pub offset: i32,
    pub init: Option<GlobalInit>,
    pub defined: bool,
    pub body: Option<Stmt>,
    pub params: Vec<ObjId>,
    pub stack_size: usize,
    pub variadic: bool,
}

impl Obj {
    pub fn new(name: String, ty: Ty) -> Obj {
        Obj {
            name,
            ty,
            is_local: false,
            is_func: false,
            offset: 0,
            init: None,
            defined: false,
            body: None,
            params: Vec::new(),
            stack_size: 0,
            variadic: false,
        }
    }

    pub fn func_ty(&self) -> Option<FuncTy> {
        match &self.ty {
            Ty::Func(f) => Some((**f).clone()),
            _ => None,
        }
    }
}
