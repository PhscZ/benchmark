//! Type representation for the supported C subset.
//!
//! Supported object types: `void`, `char` (signed 8-bit), `unsigned char`,
//! 32-bit `int`, pointers, and fixed-size arrays.  Function types only ever
//! appear as the type of a top-level declaration (function pointers are out of
//! scope), so they carry the parameter list inline.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct FuncTy {
    pub ret: Ty,
    pub params: Vec<Ty>,
    pub variadic: bool,
    /// false for `f()` (no prototype) and for implicitly declared functions
    pub has_proto: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Void,
    Char,
    UChar,
    Int,
    Ptr(Box<Ty>),
    Array(Box<Ty>, usize),
    Func(Box<FuncTy>),
}

impl Ty {
    pub fn ptr_to(t: Ty) -> Ty {
        Ty::Ptr(Box::new(t))
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Ty::Char | Ty::UChar | Ty::Int)
    }

    pub fn is_ptr(&self) -> bool {
        matches!(self, Ty::Ptr(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Ty::Array(..))
    }

    pub fn is_func(&self) -> bool {
        matches!(self, Ty::Func(_))
    }

    pub fn is_void(&self) -> bool {
        matches!(self, Ty::Void)
    }

    pub fn size(&self) -> usize {
        match self {
            Ty::Void => 1,
            Ty::Char | Ty::UChar => 1,
            Ty::Int => 4,
            Ty::Ptr(_) => 8,
            Ty::Array(t, n) => t.size() * n,
            Ty::Func(_) => 8,
        }
    }

    pub fn align(&self) -> usize {
        match self {
            Ty::Void => 1,
            Ty::Char | Ty::UChar => 1,
            Ty::Int => 4,
            Ty::Ptr(_) | Ty::Func(_) => 8,
            Ty::Array(t, _) => t.align(),
        }
    }

    /// Pointee / element type.  `void` has size 1 (GNU extension) so that
    /// `void *` arithmetic and dereference-free indexing behave predictably.
    pub fn elem(&self) -> Ty {
        match self {
            Ty::Ptr(t) => (**t).clone(),
            Ty::Array(t, _) => (**t).clone(),
            _ => Ty::Int,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Void => write!(f, "void"),
            Ty::Char => write!(f, "char"),
            Ty::UChar => write!(f, "unsigned char"),
            Ty::Int => write!(f, "int"),
            Ty::Ptr(t) => write!(f, "{} *", t),
            Ty::Array(t, n) => write!(f, "{}[{}]", t, n),
            Ty::Func(ft) => write!(f, "{} (...)", ft.ret),
        }
    }
}
