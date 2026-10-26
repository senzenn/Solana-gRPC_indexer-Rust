//! Normalize a Codama `RootNode` into the flat semantic [`Idl`](crate::model::Idl)
//! the diff engine operates on.
//!
//! Yellowstone Vixen's `include_vixen_parser!` loads the same shape via
//! `codama_nodes::RootNode` and pulls `program.events` out separately. We keep
//! events on the flat `Idl.events` list (name + discriminator bytes) so
//! `diff_named_disc("event", …)` continues to apply.

use crate::model::{
    Account, AccountItem, ArrayLen, Discriminator, EnumFields, EnumVariant, ErrorCode, Event,
    Field, Idl, Instruction, Metadata, NamedDisc, Type, TypeDef, TypeDefTy,
};
use codama_nodes::{
    AccountNode, BytesEncoding, BytesValueNode, CountNode, DefaultValueStrategy, DefinedTypeNode,
    DiscriminatorNode, Endian, EnumVariantTypeNode, ErrorNode, EventNode, InstructionAccountNode,
    InstructionInputValueNode, InstructionNode, IsAccountSigner, NestedTypeNodeTrait, NumberFormat,
    NumberTypeNode, OptionTypeNode, ProgramNode, RootNode, StructFieldTypeNode, StructTypeNode,
    TypeNode, ValueNode,
};

/// Convert a Codama root (and its `program.events`) into the flat semantic IDL.
#[must_use]
pub fn from_codama(root: RootNode) -> Idl {
    // Mirror Vixen: events live on the program node and are handled as their
    // own list (cloned out of the root for the render path).
    let events = root.program.events.clone();
    from_program(root.program, &events)
}

fn from_program(program: ProgramNode, events: &[EventNode]) -> Idl {
    let mut types: Vec<TypeDef> = program
        .defined_types
        .iter()
        .map(convert_defined_type)
        .collect();

    let accounts: Vec<NamedDisc> = program
        .accounts
        .iter()
        .map(|a| {
            let disc = extract_account_discriminator(a);
            // Account data is inline on the Codama node; promote it into
            // `types[]` under the account name so reachability + field diff work.
            types.push(account_as_type_def(a));
            NamedDisc {
                name: a.name.to_string(),
                discriminator: disc,
            }
        })
        .collect();

    let event_discs: Vec<Event> = events
        .iter()
        .map(|e| {
            types.push(event_as_type_def(e));
            // Content disc = EventNode.discriminators[] (constantDiscriminatorNode).
            // Envelope (Vixen cpi_event_*) = HiddenPrefixTypeNode.prefix bytes when
            // event.data is wrapped — NOT discarded by unwrap_wrappers (that strip
            // is only for payload struct fields). Bare structTypeNode ⇒ no envelope.
            let (envelope_discriminator, envelope_payload_offset) = extract_event_envelope(e);
            Event {
                name: e.name.to_string(),
                discriminator: extract_event_discriminator(e),
                envelope_discriminator,
                envelope_payload_offset,
            }
        })
        .collect();

    Idl {
        address: program.public_key,
        metadata: Metadata {
            name: program.name.to_string(),
            version: program.version,
            spec: String::new(),
        },
        instructions: program
            .instructions
            .iter()
            .map(convert_instruction)
            .collect(),
        accounts,
        events: event_discs,
        errors: program.errors.iter().map(convert_error).collect(),
        types,
    }
}

fn convert_error(e: &ErrorNode) -> ErrorCode {
    ErrorCode {
        code: u32::try_from(e.code).unwrap_or(u32::MAX),
        name: e.name.to_string(),
        msg: if e.message.is_empty() {
            None
        } else {
            Some(e.message.clone())
        },
    }
}

fn convert_instruction(ix: &InstructionNode) -> Instruction {
    let discriminator = extract_ix_discriminator(ix);
    let accounts = ix.accounts.iter().map(convert_ix_account).collect();
    // Skip omitted args (typically the discriminator field itself).
    let args = ix
        .arguments
        .iter()
        .filter(|a| a.default_value_strategy != Some(DefaultValueStrategy::Omitted))
        .map(|a| Field {
            name: a.name.to_string(),
            ty: convert_type(&a.r#type),
        })
        .collect();
    Instruction {
        name: ix.name.to_string(),
        discriminator,
        accounts,
        args,
    }
}

fn convert_ix_account(a: &InstructionAccountNode) -> AccountItem {
    AccountItem::Single(Account {
        name: a.name.to_string(),
        writable: a.is_writable,
        signer: matches!(a.is_signer, IsAccountSigner::True | IsAccountSigner::Either),
        optional: a.is_optional,
    })
}

fn convert_defined_type(dt: &DefinedTypeNode) -> TypeDef {
    TypeDef {
        name: dt.name.to_string(),
        serialization: default_serialization(),
        ty: convert_type_def_ty(&dt.r#type),
    }
}

fn account_as_type_def(a: &AccountNode) -> TypeDef {
    let st = a.data.get_nested_type_node();
    TypeDef {
        name: a.name.to_string(),
        serialization: default_serialization(),
        ty: TypeDefTy::Struct {
            fields: struct_fields_skipping_omitted(st),
        },
    }
}

fn event_as_type_def(e: &EventNode) -> TypeDef {
    let TypeNode::Struct(st) = unwrap_wrappers(&e.data) else {
        return TypeDef {
            name: e.name.to_string(),
            serialization: default_serialization(),
            ty: TypeDefTy::Struct { fields: vec![] },
        };
    };
    TypeDef {
        name: e.name.to_string(),
        serialization: default_serialization(),
        ty: TypeDefTy::Struct {
            fields: struct_fields_skipping_omitted(st),
        },
    }
}

fn struct_fields_skipping_omitted(st: &StructTypeNode) -> Vec<Field> {
    st.fields
        .iter()
        .filter(|f| f.default_value_strategy != Some(DefaultValueStrategy::Omitted))
        .map(|f| Field {
            name: f.name.to_string(),
            ty: convert_type(&f.r#type),
        })
        .collect()
}

fn convert_type_def_ty(t: &TypeNode) -> TypeDefTy {
    match unwrap_wrappers(t) {
        TypeNode::Struct(st) => TypeDefTy::Struct {
            fields: struct_fields_skipping_omitted(st),
        },
        TypeNode::Enum(en) => TypeDefTy::Enum {
            variants: en.variants.iter().map(convert_enum_variant).collect(),
        },
        // Convert the original node: the wrappers carry the size layout.
        _ => TypeDefTy::Type {
            alias: convert_type(t),
        },
    }
}

fn convert_enum_variant(v: &EnumVariantTypeNode) -> EnumVariant {
    match v {
        EnumVariantTypeNode::Empty(e) => EnumVariant {
            name: e.name.to_string(),
            fields: EnumFields::default(),
        },
        EnumVariantTypeNode::Struct(s) => EnumVariant {
            name: s.name.to_string(),
            fields: EnumFields::Named(struct_fields_skipping_omitted(
                s.r#struct.get_nested_type_node(),
            )),
        },
        EnumVariantTypeNode::Tuple(t) => {
            let items = t.tuple.get_nested_type_node();
            EnumVariant {
                name: t.name.to_string(),
                fields: EnumFields::Tuple(items.items.iter().map(convert_type).collect()),
            }
        }
    }
}

fn default_serialization() -> String {
    "borsh".to_string()
}

/// Strip Codama wrapper nodes that don't change Borsh layout identity for diffing.
fn unwrap_wrappers(t: &TypeNode) -> &TypeNode {
    match t {
        TypeNode::SizePrefix(sp) => unwrap_wrappers(sp.r#type.as_ref()),
        TypeNode::FixedSize(fs) => unwrap_wrappers(fs.r#type.as_ref()),
        TypeNode::HiddenPrefix(hp) => unwrap_wrappers(hp.r#type.as_ref()),
        TypeNode::HiddenSuffix(hs) => unwrap_wrappers(hs.r#type.as_ref()),
        TypeNode::PreOffset(p) => unwrap_wrappers(p.r#type.as_ref()),
        TypeNode::PostOffset(p) => unwrap_wrappers(p.r#type.as_ref()),
        TypeNode::Sentinel(s) => unwrap_wrappers(s.r#type.as_ref()),
        other => other,
    }
}

fn convert_type(t: &TypeNode) -> Type {
    match t {
        // Borsh `String` / `Bytes` carry a `u32` LE byte-size prefix. Other
        // size prefixes have no Borsh type here, so keep them unmapped.
        TypeNode::SizePrefix(sp) => match sp.r#type.as_ref() {
            TypeNode::String(_) if is_borsh_len(sp.prefix.get_nested_type_node()) => Type::String,
            TypeNode::Bytes(_) if is_borsh_len(sp.prefix.get_nested_type_node()) => Type::Bytes,
            _ => Type::Generic("sizePrefixTypeNode".to_string()),
        },
        // Fixed-size text is raw bytes on the wire with no length prefix.
        TypeNode::FixedSize(fs) => match fs.r#type.as_ref() {
            TypeNode::Bytes(_) | TypeNode::String(_) => {
                Type::Array(Box::new(Type::U8), ArrayLen::Value(fs.size))
            }
            inner => convert_type(inner),
        },
        TypeNode::Array(arr) => match &arr.count {
            CountNode::Fixed(fc) => {
                Type::Array(Box::new(convert_type(&arr.item)), ArrayLen::Value(fc.value))
            }
            CountNode::Prefixed(p) if is_borsh_len(p.prefix.get_nested_type_node()) => {
                Type::Vec(Box::new(convert_type(&arr.item)))
            }
            // A `u8`/`u16` count or a remainder array is not a Borsh `Vec`.
            CountNode::Prefixed(_) => Type::Generic("prefixedCountNode".to_string()),
            CountNode::Remainder(_) => Type::Generic("remainderCountNode".to_string()),
        },
        TypeNode::Boolean(_) => Type::Bool,
        TypeNode::Number(n) => number_format(n.format),
        TypeNode::PublicKey(_) => Type::Pubkey,
        TypeNode::Bytes(_) => Type::Bytes,
        TypeNode::String(_) => Type::String,
        TypeNode::Option(o) if is_borsh_option(o) => Type::Option(Box::new(convert_type(&o.item))),
        TypeNode::Option(_) => Type::Generic("optionTypeNode".to_string()),
        // These options have no `u8` tag on the wire, unlike Borsh `Option`.
        TypeNode::RemainderOption(_) => Type::Generic("remainderOptionTypeNode".to_string()),
        TypeNode::ZeroableOption(_) => Type::Generic("zeroableOptionTypeNode".to_string()),
        TypeNode::Link(link) => Type::Defined {
            name: link.name.to_string(),
            generics: vec![],
        },
        TypeNode::Amount(a) => number_format(a.number.get_nested_type_node().format),
        TypeNode::SolAmount(a) => number_format(a.number.get_nested_type_node().format),
        TypeNode::DateTime(d) => number_format(d.number.get_nested_type_node().format),
        TypeNode::HiddenPrefix(hp) => convert_type(hp.r#type.as_ref()),
        TypeNode::HiddenSuffix(hs) => convert_type(hs.r#type.as_ref()),
        TypeNode::PreOffset(p) => convert_type(p.r#type.as_ref()),
        TypeNode::PostOffset(p) => convert_type(p.r#type.as_ref()),
        TypeNode::Sentinel(s) => convert_type(s.r#type.as_ref()),
        // Map/Set/Tuple: key, value, and element types ARE Borsh layout — model
        // them structurally so inner widenings compare unequal (not Generic).
        TypeNode::Map(m) => Type::Map(
            Box::new(convert_type(unwrap_wrappers(m.key.as_ref()))),
            Box::new(convert_type(unwrap_wrappers(m.value.as_ref()))),
        ),
        TypeNode::Set(s) => Type::Set(Box::new(convert_type(unwrap_wrappers(s.item.as_ref())))),
        TypeNode::Tuple(t) => Type::Tuple(
            t.items
                .iter()
                .map(|item| convert_type(unwrap_wrappers(item)))
                .collect(),
        ),
        // Residual known gap: inline struct/enum used *as a field type*
        // (anonymous composite). Top-level account data / definedTypes already
        // promote to TypeDef via convert_defined_type / account_as_type_def.
        // Representing anonymous inlines needs Type::InlineStruct/InlineEnum
        // (or synthetic Defined names) — left Generic until that lands. Real
        // Vixen Codama IDLs almost always Link to definedTypes instead.
        TypeNode::Struct(_) => Type::Generic("structTypeNode".to_string()),
        TypeNode::Enum(_) => Type::Generic("enumTypeNode".to_string()),
    }
}

/// Borsh writes collection lengths as `u32` little-endian.
fn is_borsh_len(n: &NumberTypeNode) -> bool {
    n.format == NumberFormat::U32 && n.endian == Endian::Little
}

/// Borsh `Option`: a `u8` tag, and no payload when the tag is 0.
fn is_borsh_option(o: &OptionTypeNode) -> bool {
    !o.fixed && o.prefix.get_nested_type_node().format == NumberFormat::U8
}

fn number_format(f: NumberFormat) -> Type {
    match f {
        NumberFormat::U8 => Type::U8,
        NumberFormat::U16 => Type::U16,
        NumberFormat::U32 => Type::U32,
        NumberFormat::U64 => Type::U64,
        NumberFormat::U128 => Type::U128,
        NumberFormat::I8 => Type::I8,
        NumberFormat::I16 => Type::I16,
        NumberFormat::I32 => Type::I32,
        NumberFormat::I64 => Type::I64,
        NumberFormat::I128 => Type::I128,
        NumberFormat::F32 => Type::F32,
        NumberFormat::F64 => Type::F64,
        // Not a Borsh scalar we model; opaque by design.
        NumberFormat::ShortU16 => Type::Generic(format!("{f:?}").to_lowercase()),
    }
}

// --- discriminators -------------------------------------------------------

fn extract_ix_discriminator(ix: &InstructionNode) -> Discriminator {
    extract_discriminator(&ix.discriminators, |name| {
        let arg = ix.arguments.iter().find(|a| a.name.as_str() == name)?;
        match arg.default_value.as_ref()? {
            InstructionInputValueNode::Bytes(b) => Some(decode_bytes(b)),
            InstructionInputValueNode::Number(n) => number_to_bytes(n, &arg.r#type),
            InstructionInputValueNode::Constant(c) => {
                value_to_bytes(c.value.as_ref(), c.r#type.as_ref())
            }
            _ => None,
        }
    })
}

fn extract_account_discriminator(a: &AccountNode) -> Discriminator {
    let st = a.data.get_nested_type_node();
    extract_discriminator(&a.discriminators, |name| {
        let field = st.fields.iter().find(|f| f.name.as_str() == name)?;
        field_default_bytes(field)
    })
}

fn extract_event_discriminator(e: &EventNode) -> Discriminator {
    let fields: &[StructFieldTypeNode] = match unwrap_wrappers(&e.data) {
        TypeNode::Struct(st) => st.fields.as_slice(),
        _ => &[],
    };
    extract_discriminator(&e.discriminators, |name| {
        let field = fields.iter().find(|f| f.name.as_str() == name)?;
        field_default_bytes(field)
    })
}

/// Capture Vixen self-CPI event envelope from Codama `hiddenPrefixTypeNode`.
///
/// - `discriminator` (caller): `EventNode.discriminators[]` content sighash.
/// - `envelope_discriminator`: concatenation of `HiddenPrefixTypeNode.prefix`
///   [`ConstantValueNode`] bytes (`prefix` field on the nestable node).
/// - `envelope_payload_offset`: `Some(prefix_len)` when a non-empty prefix exists.
///
/// Payload layout still uses [`unwrap_wrappers`] so the prefix is not part of
/// the promoted event struct fields.
fn extract_event_envelope(e: &EventNode) -> (Discriminator, Option<usize>) {
    let TypeNode::HiddenPrefix(hp) = &e.data else {
        return (Vec::new(), None);
    };
    let mut bytes = Vec::new();
    for constant in &hp.prefix {
        if let Some(chunk) = value_to_bytes(constant.value.as_ref(), constant.r#type.as_ref()) {
            bytes.extend(chunk);
        }
    }
    if bytes.is_empty() {
        (Vec::new(), None)
    } else {
        let len = bytes.len();
        (bytes, Some(len))
    }
}

fn field_default_bytes(field: &StructFieldTypeNode) -> Option<Vec<u8>> {
    match field.default_value.as_ref()? {
        ValueNode::Bytes(b) => Some(decode_bytes(b)),
        ValueNode::Number(n) => number_to_bytes(n, &field.r#type),
        ValueNode::Constant(c) => value_to_bytes(c.value.as_ref(), c.r#type.as_ref()),
        _ => None,
    }
}

fn extract_discriminator(
    discs: &[DiscriminatorNode],
    resolve_field: impl Fn(&str) -> Option<Vec<u8>>,
) -> Discriminator {
    let Some(d) = discs.first() else {
        return Vec::new();
    };
    match d {
        DiscriminatorNode::Constant(c) => {
            value_to_bytes(c.constant.value.as_ref(), c.constant.r#type.as_ref())
                .unwrap_or_default()
        }
        DiscriminatorNode::Field(f) => resolve_field(f.name.as_str()).unwrap_or_default(),
        DiscriminatorNode::Size(_) => Vec::new(),
    }
}

fn value_to_bytes(v: &ValueNode, ty: &TypeNode) -> Option<Vec<u8>> {
    match v {
        ValueNode::Bytes(b) => Some(decode_bytes(b)),
        ValueNode::Number(n) => number_to_bytes(n, ty),
        _ => None,
    }
}

/// Encode a numeric discriminator at the width and byte order of its number
/// type. A value that does not fit gives no discriminator.
fn number_to_bytes(n: &codama_nodes::NumberValueNode, ty: &TypeNode) -> Option<Vec<u8>> {
    let v = match n.number {
        codama_nodes::Number::UnsignedInteger(v) => i128::from(v),
        codama_nodes::Number::SignedInteger(v) => i128::from(v),
        codama_nodes::Number::Float(_) => return None,
    };
    // Without a number type, keep the 1-byte tag of native programs.
    let (format, endian) = match unwrap_wrappers(ty) {
        TypeNode::Number(nt) => (nt.format, nt.endian),
        _ => (NumberFormat::U8, Endian::Little),
    };
    let mut bytes = match format {
        NumberFormat::U8 => u8::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::U16 => u16::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::U32 => u32::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::U64 => u64::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::U128 => u128::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::I8 => i8::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::I16 => i16::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::I32 => i32::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::I64 => i64::try_from(v).ok()?.to_le_bytes().to_vec(),
        NumberFormat::I128 => v.to_le_bytes().to_vec(),
        // compact-u16 is one byte below 0x80.
        NumberFormat::ShortU16 => vec![u8::try_from(v).ok().filter(|b| *b < 0x80)?],
        NumberFormat::F32 | NumberFormat::F64 => return None,
    };
    if endian == Endian::Big {
        bytes.reverse();
    }
    Some(bytes)
}

/// Decode a Codama [`BytesValueNode`] to its underlying bytes.
///
/// Encodings must normalize to the same `Vec<u8>`: a constant discriminator
/// written as base16 in one IDL version and base58/base64 in another (e.g.
/// after Vixen PR #277 / a Codama bump) is the *same* value. Storing the
/// encoded string's UTF-8 bytes for base58/base64 caused false-positive
/// "discriminator changed" findings on self-equivalent IDLs.
///
/// Never panics: malformed input falls back to the raw string bytes.
#[must_use]
pub fn decode_bytes(b: &BytesValueNode) -> Vec<u8> {
    match b.encoding {
        BytesEncoding::Base16 => decode_hex(&b.data),
        BytesEncoding::Base58 => bs58::decode(&b.data)
            .into_vec()
            .unwrap_or_else(|_| b.data.as_bytes().to_vec()),
        BytesEncoding::Base64 => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            STANDARD
                .decode(&b.data)
                .unwrap_or_else(|_| b.data.as_bytes().to_vec())
        }
        BytesEncoding::Utf8 => b.data.as_bytes().to_vec(),
    }
}

/// Test/tooling helper: decode `data` under a Codama encoding name
/// (`"base16"` / `"base58"` / `"base64"` / `"utf8"`).
#[must_use]
pub fn decode_encoded_bytes(encoding: &str, data: &str) -> Vec<u8> {
    let node = match encoding {
        "base16" => BytesValueNode::base16(data),
        "base58" => BytesValueNode::base58(data),
        "base64" => BytesValueNode::base64(data),
        _ => BytesValueNode::utf8(data),
    };
    decode_bytes(&node)
}

fn decode_hex(s: &str) -> Vec<u8> {
    let s = s.trim();
    let padded = if s.len() % 2 == 1 {
        format!("0{s}")
    } else {
        s.to_string()
    };
    // Work on bytes: a str slice at an odd offset panics on non-ASCII input.
    let nibble = |b: u8| char::from(b).to_digit(16);
    padded
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| match pair {
            [hi, lo] => u8::try_from(nibble(*hi)? * 16 + nibble(*lo)?).ok(),
            _ => None,
        })
        .collect()
}

/// Coerce `"code": "6000"` → `6000` in `program.errors[]` (same as Vixen).
pub fn fix_string_error_codes(value: &mut serde_json::Value) {
    let Some(errors) = value
        .get_mut("program")
        .and_then(|p| p.get_mut("errors"))
        .and_then(|e| e.as_array_mut())
    else {
        return;
    };
    for error in errors {
        if let Some(code) = error.get_mut("code") {
            if let Some(s) = code.as_str().and_then(|s| s.parse::<u64>().ok()) {
                *code = serde_json::Value::Number(s.into());
            }
        }
    }
}
