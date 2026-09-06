//! IDL data model — flat semantic view used by the diff engine.
//!
//! **On-disk input is Codama-first.** Yellowstone Vixen's `include_vixen_parser!`
//! deserializes a Codama `RootNode` (`kind: "rootNode"`, `standard: "codama"`)
//! via `codama-nodes`. [`Idl::from_json`] detects that shape, runs the same
//! string-error-code coercion Vixen does, and normalizes into this flat model
//! (see [`crate::codama_convert`]).
//!
//! Codama → flat field map (the shapes that differ from Anchor/Solana IDL spec):
//! | Codama | Flat `Idl` |
//! |---|---|
//! | `program.publicKey` | `address` |
//! | `program.name` / `program.version` | `metadata.{name,version}` |
//! | `program.definedTypes` | `types` |
//! | `program.events[]` (`eventNode`) | `events[]` (`Event`) |
//! | `instruction.arguments` | `instruction.args` |
//! | `isWritable` / `isSigner` / `isOptional` | `writable` / `signer` / `optional` |
//! | `numberTypeNode{format}` etc. | `Type::{U64,Pubkey,…}` |
//! | disc nodes + `bytesValueNode` | `discriminator: Vec<u8>` |
//!
//! Anchor / Solana IDL-spec JSON (test fixtures, `anchor idl fetch`) still
//! deserializes directly into these structs.
//!
//! The key property: every optional/default field uses `#[serde(default)]`
//! and the structs derive `PartialEq`. So an IDL that omits `writable: false`
//! and one that writes it explicitly deserialize to *identical* values.

use serde::Deserialize;
use std::collections::BTreeMap;

pub type Discriminator = Vec<u8>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Idl {
    pub address: String,
    pub metadata: Metadata,
    #[serde(default)]
    pub instructions: Vec<Instruction>,
    #[serde(default)]
    pub accounts: Vec<NamedDisc>,
    /// Top-level event list (Codama: `program.events`). Each entry carries a
    /// content discriminator plus an optional self-CPI envelope (Vixen's
    /// `cpi_event_discriminator` / `cpi_event_payload_offset`).
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub errors: Vec<ErrorCode>,
    #[serde(default)]
    pub types: Vec<TypeDef>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Metadata {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub spec: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Instruction {
    pub name: String,
    /// Discriminator bytes. Optional because native / SPL-style programs
    /// dispatch by a 1-byte tag or by data length and may omit it entirely
    /// (as Agave's own `transaction-status` parsers and the SPL token parser
    /// do). Anchor uses 8 bytes; Quasar uses 1. Empty means "no explicit
    /// discriminator — dispatch is by convention."
    #[serde(default)]
    pub discriminator: Discriminator,
    #[serde(default)]
    pub accounts: Vec<AccountItem>,
    #[serde(default)]
    pub args: Vec<Field>,
}

/// Accounts can be a single account or a nested composite group (untagged:
/// distinguished by the presence of an `accounts` array).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AccountItem {
    Composite {
        name: String,
        accounts: Vec<AccountItem>,
    },
    Single(Account),
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Account {
    pub name: String,
    #[serde(default)]
    pub writable: bool,
    #[serde(default)]
    pub signer: bool,
    #[serde(default)]
    pub optional: bool,
}

/// Account types: a name + a discriminator.
/// The discriminator is optional (native/SPL account types may lack one and
/// be distinguished by data length, as Agave's SPL parser does via
/// `SplMint::LEN` / `SplAccount::LEN`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct NamedDisc {
    pub name: String,
    #[serde(default)]
    pub discriminator: Discriminator,
}

/// A program event. Content discriminator identifies the event payload; the
/// optional envelope models Vixen's self-CPI wrap (Anchor's 8-byte event-ix
/// tag vs Pinocchio's 1-byte custom envelope). Empty envelope fields mean
/// "not specified in the IDL" and are skipped by the envelope diff rule.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Event {
    pub name: String,
    /// Event content discriminator (e.g. Anchor event sighash).
    #[serde(default)]
    pub discriminator: Discriminator,
    /// Self-CPI instruction envelope discriminator bytes
    /// (`cpi_event_discriminator`). Anchor default is 8 bytes; Pinocchio
    /// programs often use 1. Empty = unspecified.
    #[serde(default)]
    pub envelope_discriminator: Discriminator,
    /// Byte offset where event payload decoding starts
    /// (`cpi_event_payload_offset`). `None` = unspecified (Vixen defaults it
    /// to `envelope_discriminator.len()` when the envelope is set).
    #[serde(default)]
    pub envelope_payload_offset: Option<usize>,
}

impl Event {
    /// Effective payload offset: explicit value, else envelope length when
    /// an envelope is present.
    #[must_use]
    pub fn effective_payload_offset(&self) -> Option<usize> {
        self.envelope_payload_offset
            .or((!self.envelope_discriminator.is_empty())
                .then_some(self.envelope_discriminator.len()))
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ErrorCode {
    pub code: u32,
    pub name: String,
    #[serde(default)]
    pub msg: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Type,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TypeDef {
    pub name: String,
    #[serde(default = "default_serialization")]
    pub serialization: String,
    #[serde(rename = "type")]
    pub ty: TypeDefTy,
}

fn default_serialization() -> String {
    "borsh".to_string()
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TypeDefTy {
    Struct {
        #[serde(default)]
        fields: Vec<Field>,
    },
    Enum {
        #[serde(default)]
        variants: Vec<EnumVariant>,
    },
    /// A type alias, e.g. `{ "kind": "type", "alias": "u64" }`.
    Type { alias: Type },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    /// Variant payload. Anchor emits either named fields
    /// (`[{"name":"x","type":"u64"}]`) or tuple fields (`["u64"]`).
    #[serde(default)]
    pub fields: EnumFields,
}

/// Enum variant field list — named (struct-like) or tuple.
///
/// `Tuple` is tried first: Anchor emits bare type strings (`["u64"]`,
/// `["bool"]`) for tuple variants, and serde's untagged recovery after a
/// failed `Vec<Field>` element is unreliable.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum EnumFields {
    Tuple(Vec<Type>),
    Named(Vec<Field>),
}

impl Default for EnumFields {
    fn default() -> Self {
        Self::Tuple(Vec::new())
    }
}

/// The IDL type system. `Defined` references a name in the top-level `types[]`,
/// which is what forces transitive resolution during diffing.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Type {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    U128,
    I128,
    Bytes,
    String,
    Pubkey,
    Option(Box<Type>),
    Vec(Box<Type>),
    Array(Box<Type>, ArrayLen),
    /// Borsh `HashMap` / `BTreeMap`: `u32` len + `(key, value)*`. Key and value
    /// types are part of the layout — changing either is a byte-level break.
    Map(Box<Type>, Box<Type>),
    /// Borsh set: `u32` len + `item*`. Element type is layout-bearing.
    Set(Box<Type>),
    /// Product type: elements serialized in order with no length prefix.
    /// Element types and order participate in equality.
    Tuple(Vec<Type>),
    /// Reference to a user-defined type in `types[]`. Carries generic
    /// arguments: `Wrapper<u64>` and `Wrapper<u32>` are DIFFERENT types with
    /// different Borsh layouts, so `generics` must participate in equality —
    /// dropping it (Blocker #14) silently misses breaking generic-arg changes.
    Defined {
        name: String,
        #[serde(default)]
        generics: Vec<GenericArg>,
    },
    Generic(String),
}

/// A generic argument supplied when instantiating a `Defined` type.
/// Mirrors `IdlGenericArg` from the Solana IDL spec.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GenericArg {
    /// A type argument, e.g. the `u64` in `Wrapper<u64>`.
    Type {
        #[serde(rename = "type")]
        ty: Box<Type>,
    },
    /// A const argument, e.g. the `8` in `Buffer<8>`.
    Const { value: String },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ArrayLen {
    Value(usize),
    Generic(String),
}

impl Idl {
    /// Parse an IDL from JSON text.
    ///
    /// Accepts:
    /// - **Codama** `RootNode` JSON (`kind: "rootNode"`) — the shape Vixen loads
    /// - **Anchor / Solana IDL-spec** JSON — used by unit-test fixtures and
    ///   `anchor idl fetch` (including legacy Anchor 0.x shapes)
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let mut value: serde_json::Value = serde_json::from_str(s)?;
        if value.get("kind").and_then(|k| k.as_str()) == Some("rootNode") {
            crate::codama_convert::fix_string_error_codes(&mut value);
            let root: codama_nodes::RootNode = serde_json::from_value(value)?;
            Ok(crate::codama_convert::from_codama(root))
        } else {
            normalize_legacy_anchor(&mut value);
            serde_json::from_value(value)
        }
    }

    /// Index the top-level `types[]` by name for transitive `Defined` lookups.
    #[must_use]
    pub fn type_map(&self) -> BTreeMap<String, &TypeDef> {
        self.types.iter().map(|t| (t.name.clone(), t)).collect()
    }
}

/// Coerce legacy Anchor IDL JSON (`anchor idl fetch` on older programs) into
/// the Solana IDL-spec shape our structs deserialize.
///
/// Handles: top-level `name`/`version` → `metadata`; missing `address`;
/// `publicKey` → `pubkey`; `isMut`/`isSigner` → `writable`/`signer`;
/// account/event inline `type`/`fields` promoted into `types[]`.
fn normalize_legacy_anchor(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    if !obj.contains_key("metadata") && (obj.contains_key("name") || obj.contains_key("version")) {
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let version = obj
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0")
            .to_string();
        obj.insert(
            "metadata".into(),
            serde_json::json!({
                "name": name,
                "version": version,
                "spec": "legacy-anchor",
            }),
        );
        // Legacy Anchor IDL often omits address; allow empty so fetch+stamp works.
        if !obj.contains_key("address") {
            obj.insert("address".into(), serde_json::Value::String(String::new()));
        }
    }

    let mut extra_types: Vec<serde_json::Value> = Vec::new();

    if let Some(accounts) = obj.get_mut("accounts").and_then(|a| a.as_array_mut()) {
        for acct in accounts.iter_mut() {
            let Some(aobj) = acct.as_object_mut() else {
                continue;
            };
            if let Some(ty) = aobj.remove("type") {
                let name = aobj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("UnknownAccount")
                    .to_string();
                extra_types.push(serde_json::json!({
                    "name": name,
                    "type": ty,
                }));
            }
            aobj.entry("discriminator".to_string())
                .or_insert_with(|| serde_json::json!([]));
        }
    }

    if let Some(events) = obj.get_mut("events").and_then(|a| a.as_array_mut()) {
        for ev in events.iter_mut() {
            let Some(eobj) = ev.as_object_mut() else {
                continue;
            };
            if let Some(fields) = eobj.remove("fields") {
                let name = eobj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("UnknownEvent")
                    .to_string();
                // Drop Anchor event field metadata (`index`) — not part of layout.
                let clean_fields: Vec<serde_json::Value> = fields
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|f| {
                                let fo = f.as_object()?;
                                Some(serde_json::json!({
                                    "name": fo.get("name")?,
                                    "type": fo.get("type")?,
                                }))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                extra_types.push(serde_json::json!({
                    "name": name,
                    "type": { "kind": "struct", "fields": clean_fields },
                }));
            }
            eobj.entry("discriminator".to_string())
                .or_insert_with(|| serde_json::json!([]));
        }
    }

    if !extra_types.is_empty() {
        let types = obj
            .entry("types".to_string())
            .or_insert_with(|| serde_json::json!([]));
        if let Some(arr) = types.as_array_mut() {
            arr.extend(extra_types);
        }
    }

    rewrite_legacy_keys(value);
    normalize_tuple_struct_fields(value);
}

/// Anchor sometimes emits newtype/tuple structs as `"fields": ["bool"]`
/// (bare type strings). Our model expects named `Field`s — invent `_0`, `_1`, …
fn normalize_tuple_struct_fields(value: &mut serde_json::Value) {
    let Some(types) = value.get_mut("types").and_then(|t| t.as_array_mut()) else {
        return;
    };
    for td in types {
        let Some(ty) = td.get_mut("type") else {
            continue;
        };
        let Some(kind) = ty.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        if kind != "struct" {
            continue;
        }
        let Some(fields) = ty.get_mut("fields").and_then(|f| f.as_array_mut()) else {
            continue;
        };
        if fields.is_empty() || fields.iter().all(serde_json::Value::is_object) {
            continue;
        }
        let rewritten: Vec<serde_json::Value> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if f.is_object() {
                    f.clone()
                } else {
                    serde_json::json!({ "name": format!("_{i}"), "type": f })
                }
            })
            .collect();
        *fields = rewritten;
    }
}

fn rewrite_legacy_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            // Account flags (instruction accounts).
            if let Some(v) = map.remove("isMut") {
                map.entry("writable".to_string()).or_insert(v);
            }
            if let Some(v) = map.remove("isSigner") {
                map.entry("signer".to_string()).or_insert(v);
            }
            // Old Anchor shorthand: `{"defined": "Foo"}` → `{"defined":{"name":"Foo"}}`.
            if let Some(serde_json::Value::String(name)) = map.get("defined").cloned() {
                map.insert(
                    "defined".into(),
                    serde_json::json!({ "name": name, "generics": [] }),
                );
            }
            for v in map.values_mut() {
                rewrite_legacy_keys(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                rewrite_legacy_keys(v);
            }
        }
        serde_json::Value::String(s) if s == "publicKey" => {
            *s = "pubkey".into();
        }
        _ => {}
    }
}
