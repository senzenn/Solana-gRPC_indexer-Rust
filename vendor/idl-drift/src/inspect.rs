//! Coverage report: find Codama shapes that fell through to `Type::Generic`,
//! generic array lengths, and references to types missing from `types[]`.
//! A decoder cannot know how many bytes any of these use.

use std::collections::BTreeSet;

use crate::diff::reachable_types;
use crate::model::{ArrayLen, EnumFields, Idl, Type, TypeDefTy};

/// One field (or ix arg) whose resolved type is / contains `Type::Generic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmappedField {
    /// Display path, e.g. `MyAccount.owner` or `ix:swap.amount`.
    pub path: String,
    /// The opaque kind string inside `Type::Generic(...)`, or
    /// `arrayLen:<name>` / `undefined:<name>`.
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
    let defined: BTreeSet<&str> = idl.types.iter().map(|t| t.name.as_str()).collect();
    let mut unmapped = Vec::new();

    for ix in &idl.instructions {
        for arg in &ix.args {
            collect_generic(
                &format!("ix:{}.{}", ix.name, arg.name),
                &arg.ty,
                &defined,
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
                    collect_generic(
                        &format!("{}.{}", td.name, f.name),
                        &f.ty,
                        &defined,
                        &mut unmapped,
                    );
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
                                    &defined,
                                    &mut unmapped,
                                );
                            }
                        }
                        EnumFields::Tuple(tys) => {
                            for (i, ty) in tys.iter().enumerate() {
                                collect_generic(
                                    &format!("{}.{}.{}", td.name, v.name, i),
                                    ty,
                                    &defined,
                                    &mut unmapped,
                                );
                            }
                        }
                    }
                }
            }
            TypeDefTy::Type { alias } => {
                collect_generic(&td.name, alias, &defined, &mut unmapped);
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

fn collect_generic(path: &str, ty: &Type, defined: &BTreeSet<&str>, out: &mut Vec<UnmappedField>) {
    let unmapped = |kind: String| UnmappedField {
        path: path.to_string(),
        kind,
    };
    match ty {
        Type::Generic(kind) => out.push(unmapped(kind.clone())),
        Type::Array(inner, len) => {
            if let ArrayLen::Generic(name) = len {
                out.push(unmapped(format!("arrayLen:{name}")));
            }
            collect_generic(path, inner, defined, out);
        }
        Type::Option(inner) | Type::Vec(inner) | Type::Set(inner) => {
            collect_generic(path, inner, defined, out);
        }
        Type::Map(key, value) => {
            collect_generic(path, key, defined, out);
            collect_generic(path, value, defined, out);
        }
        Type::Tuple(items) => {
            for item in items {
                collect_generic(path, item, defined, out);
            }
        }
        Type::Defined { name, generics } => {
            if !defined.contains(name.as_str()) {
                out.push(unmapped(format!("undefined:{name}")));
            }
            for g in generics {
                if let crate::model::GenericArg::Type { ty } = g {
                    collect_generic(path, ty, defined, out);
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
        | Type::F32
        | Type::F64
        | Type::Bytes
        | Type::String
        | Type::Pubkey => {}
    }
}
