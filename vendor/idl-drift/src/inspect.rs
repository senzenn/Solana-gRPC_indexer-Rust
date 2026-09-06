//! Coverage report: find Codama shapes that fell through to `Type::Generic`.

use crate::diff::reachable_types;
use crate::model::{EnumFields, Idl, Type, TypeDefTy};

/// One field (or ix arg) whose resolved type is / contains `Type::Generic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmappedField {
    /// Display path, e.g. `MyAccount.owner` or `ix:swap.amount`.
    pub path: String,
    /// The opaque kind string inside `Type::Generic(...)`.
    pub kind: String,
}

/// Coverage summary for one parsed IDL.
#[derive(Debug, Clone)]
pub struct CoverageReport {
    pub instructions: usize,
    pub accounts: usize,
    pub events: usize,
    pub types: usize,
    pub unmapped: Vec<UnmappedField>,
}

impl CoverageReport {
    /// `true` when every reachable field resolved to a concrete layout type.
    #[must_use]
    pub fn fully_mapped(&self) -> bool {
        self.unmapped.is_empty()
    }
}

/// Scan an IDL for reachable fields that resolved to `Type::Generic`.
#[must_use]
pub fn coverage(idl: &Idl) -> CoverageReport {
    let reached = reachable_types(idl);
    let mut unmapped = Vec::new();

    for ix in &idl.instructions {
        for arg in &ix.args {
            collect_generic(
                &format!("ix:{}.{}", ix.name, arg.name),
                &arg.ty,
                &mut unmapped,
            );
        }
    }

    for td in &idl.types {
        if !reached.contains(&td.name) {
            continue;
        }
        match &td.ty {
            TypeDefTy::Struct { fields } => {
                for f in fields {
                    collect_generic(&format!("{}.{}", td.name, f.name), &f.ty, &mut unmapped);
                }
            }
            TypeDefTy::Enum { variants } => {
                for v in variants {
                    match &v.fields {
                        EnumFields::Named(fields) => {
                            for f in fields {
                                collect_generic(
                                    &format!("{}.{}.{}", td.name, v.name, f.name),
                                    &f.ty,
                                    &mut unmapped,
                                );
                            }
                        }
                        EnumFields::Tuple(tys) => {
                            for (i, ty) in tys.iter().enumerate() {
                                collect_generic(
                                    &format!("{}.{}.{}", td.name, v.name, i),
                                    ty,
                                    &mut unmapped,
                                );
                            }
                        }
                    }
                }
            }
            TypeDefTy::Type { alias } => {
                collect_generic(&td.name, alias, &mut unmapped);
            }
        }
    }

    CoverageReport {
        instructions: idl.instructions.len(),
        accounts: idl.accounts.len(),
        events: idl.events.len(),
        types: idl.types.len(),
        unmapped,
    }
}

fn collect_generic(path: &str, ty: &Type, out: &mut Vec<UnmappedField>) {
    match ty {
        Type::Generic(kind) => {
            out.push(UnmappedField {
                path: path.to_string(),
                kind: kind.clone(),
            });
        }
        Type::Option(inner) | Type::Vec(inner) | Type::Array(inner, _) | Type::Set(inner) => {
            collect_generic(path, inner, out);
        }
        Type::Map(key, value) => {
            collect_generic(path, key, out);
            collect_generic(path, value, out);
        }
        Type::Tuple(items) => {
            for item in items {
                collect_generic(path, item, out);
            }
        }
        Type::Defined { generics, .. } => {
            for g in generics {
                if let crate::model::GenericArg::Type { ty } = g {
                    collect_generic(path, ty, out);
                }
            }
        }
        Type::Bool
        | Type::U8
        | Type::I8
        | Type::U16
        | Type::I16
        | Type::U32
        | Type::I32
        | Type::U64
        | Type::I64
        | Type::U128
        | Type::I128
        | Type::Bytes
        | Type::String
        | Type::Pubkey => {}
    }
}
