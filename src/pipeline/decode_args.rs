use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use idl_drift::model::{ArrayLen, EnumFields, Idl, Instruction, Type, TypeDef, TypeDefTy};
use serde_json::{Value, json};

/// Type nesting limit. A self-referential IDL type must fail here, not
/// overflow the stack.
const MAX_DEPTH: usize = 64;

/// Work limit per instruction: 1 per decoded node plus the length of each
/// emitted name or string. An IDL can make output grow much faster than input.
const MAX_WORK: usize = 1 << 20;

/// IDL types by name. Build it once per program, not per instruction.
pub type TypeMap = HashMap<String, TypeDef>;

pub fn type_map(idl: &Idl) -> TypeMap {
    let mut map = TypeMap::new();
    for def in &idl.types {
        map.entry(def.name.clone()).or_insert_with(|| def.clone());
    }
    map
}

pub fn decode_args(types: &TypeMap, ix: &Instruction, data: &[u8]) -> Value {
    // Args start after the discriminator. Anchor uses 8 bytes; Codama and
    // Quasar programs can use other lengths.
    let cursor = ix.discriminator.len();
    if data.len() < cursor {
        return json!({});
    }
    let mut decoder = Decoder {
        types,
        data,
        cursor,
        budget: MAX_WORK,
    };
    let mut out = serde_json::Map::new();

    for arg in &ix.args {
        let decoded = decoder
            .charge(arg.name.len())
            .and_then(|()| decoder.value(&arg.ty, 0));
        match decoded {
            Ok(v) => {
                out.insert(arg.name.clone(), v);
            }
            Err(_) => break,
        }
    }

    Value::Object(out)
}

struct Decoder<'a> {
    types: &'a TypeMap,
    data: &'a [u8],
    cursor: usize,
    budget: usize,
}

impl Decoder<'_> {
    fn charge(&mut self, cost: usize) -> Result<()> {
        match self.budget.checked_sub(cost) {
            Some(left) if left > 0 => {
                self.budget = left;
                Ok(())
            }
            _ => bail!("decode work limit {MAX_WORK} reached"),
        }
    }

    fn text(&mut self, s: String) -> Result<Value> {
        self.charge(s.len())?;
        Ok(Value::String(s))
    }

    fn value(&mut self, ty: &Type, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            bail!("type nesting deeper than {MAX_DEPTH}");
        }
        self.charge(1)?;
        let depth = depth + 1;
        let data = self.data;
        let cursor = &mut self.cursor;
        match ty {
            Type::Bool => Ok(json!(read_bool(data, cursor)?)),
            Type::U8 => Ok(json!(read_u8(data, cursor)?)),
            Type::U16 => Ok(json!(read_u16(data, cursor)?)),
            Type::U32 => Ok(json!(read_u32(data, cursor)?)),
            Type::U64 => Ok(json!(read_u64(data, cursor)?)),
            Type::U128 => {
                let v = read_u128(data, cursor)?;
                self.text(v.to_string())
            }
            Type::I8 => Ok(json!(read_i8(data, cursor)?)),
            Type::I16 => Ok(json!(read_i16(data, cursor)?)),
            Type::I32 => Ok(json!(read_i32(data, cursor)?)),
            Type::I64 => Ok(json!(read_i64(data, cursor)?)),
            Type::I128 => {
                let v = read_i128(data, cursor)?;
                self.text(v.to_string())
            }
            Type::F32 => Ok(float_value(f64::from(read_f32(data, cursor)?))),
            Type::F64 => Ok(float_value(read_f64(data, cursor)?)),
            Type::Pubkey => {
                let bytes = read_bytes(data, cursor, 32)?;
                self.text(bs58::encode(bytes).into_string())
            }
            Type::String => {
                let len = read_u32(data, cursor)? as usize;
                let bytes = read_bytes(data, cursor, len)?;
                self.text(String::from_utf8_lossy(bytes).into_owned())
            }
            Type::Bytes => {
                let len = read_u32(data, cursor)? as usize;
                let bytes = read_bytes(data, cursor, len)?;
                self.text(bs58::encode(bytes).into_string())
            }
            Type::Option(inner) => {
                let tag = read_u8(data, cursor)?;
                if tag == 0 {
                    Ok(Value::Null)
                } else {
                    self.value(inner, depth)
                }
            }
            Type::Vec(inner) | Type::Set(inner) => {
                let len = read_count(data, cursor)?;
                Ok(Value::Array(self.items(inner, len, depth)?))
            }
            Type::Array(inner, len) => {
                let n = match len {
                    ArrayLen::Value(v) => check_count(*v, data, *cursor)?,
                    ArrayLen::Generic(name) => bail!("array length is generic parameter {name}"),
                };
                Ok(Value::Array(self.items(inner, n, depth)?))
            }
            Type::Tuple(items) => {
                let mut out = Vec::new();
                for item in items {
                    out.push(self.value(item, depth)?);
                }
                Ok(Value::Array(out))
            }
            Type::Defined { name, .. } => self.defined(name, depth),
            Type::Map(key, value) => {
                let len = read_count(data, cursor)?;
                let mut map = serde_json::Map::new();
                for _ in 0..len {
                    let start = self.cursor;
                    let k = self.value(key, depth)?;
                    let v = self.value(value, depth)?;
                    if self.cursor == start {
                        bail!("map entry uses zero bytes");
                    }
                    let k = match k {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    map.insert(k, v);
                }
                Ok(Value::Object(map))
            }
            Type::Generic(name) => bail!("unresolved generic type parameter {name}"),
        }
    }

    /// Decode `n` elements. An element that uses zero bytes makes the count
    /// meaningless, so stop with an error.
    fn items(&mut self, inner: &Type, n: usize, depth: usize) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        for _ in 0..n {
            let start = self.cursor;
            items.push(self.value(inner, depth)?);
            if self.cursor == start {
                bail!("sequence element uses zero bytes");
            }
        }
        Ok(items)
    }

    fn defined(&mut self, name: &str, depth: usize) -> Result<Value> {
        let types = self.types;
        let Some(def) = types.get(name) else {
            bail!("type {name} is not defined in the IDL");
        };
        match &def.ty {
            TypeDefTy::Struct { fields } => {
                let mut map = serde_json::Map::new();
                for field in fields {
                    self.charge(field.name.len())?;
                    let v = self.value(&field.ty, depth)?;
                    map.insert(field.name.clone(), v);
                }
                Ok(Value::Object(map))
            }
            TypeDefTy::Enum { variants } => {
                let disc = read_u8(self.data, &mut self.cursor)?;
                let Some(variant) = variants.get(disc as usize) else {
                    return Ok(json!({ "variant_index": disc }));
                };
                self.charge(variant.name.len())?;
                let mut map = serde_json::Map::new();
                map.insert("variant".into(), json!(variant.name));
                match &variant.fields {
                    EnumFields::Named(fields) => {
                        for field in fields {
                            self.charge(field.name.len())?;
                            let v = self.value(&field.ty, depth)?;
                            map.insert(field.name.clone(), v);
                        }
                    }
                    EnumFields::Tuple(types_) => {
                        let mut arr = Vec::new();
                        for t in types_ {
                            arr.push(self.value(t, depth)?);
                        }
                        map.insert("fields".into(), Value::Array(arr));
                    }
                }
                Ok(Value::Object(map))
            }
            TypeDefTy::Type { alias } => self.value(alias, depth),
        }
    }
}

/// Read a Borsh `u32` element count.
fn read_count(data: &[u8], cursor: &mut usize) -> Result<usize> {
    let len = read_u32(data, cursor)? as usize;
    check_count(len, data, *cursor)
}

/// Each element uses at least one byte, so a count above the remaining
/// bytes is not valid.
fn check_count(n: usize, data: &[u8], cursor: usize) -> Result<usize> {
    let left = data.len().saturating_sub(cursor);
    if n > left {
        bail!("element count {n} exceeds {left} remaining bytes");
    }
    Ok(n)
}

fn read_bool(data: &[u8], cursor: &mut usize) -> Result<bool> {
    Ok(read_u8(data, cursor)? != 0)
}

fn read_u8(data: &[u8], cursor: &mut usize) -> Result<u8> {
    Ok(*read_bytes(data, cursor, 1)?.first().context("empty")?)
}

fn read_u16(data: &[u8], cursor: &mut usize) -> Result<u16> {
    Ok(u16::from_le_bytes(read_bytes(data, cursor, 2)?.try_into()?))
}

fn read_u32(data: &[u8], cursor: &mut usize) -> Result<u32> {
    Ok(u32::from_le_bytes(read_bytes(data, cursor, 4)?.try_into()?))
}

fn read_u64(data: &[u8], cursor: &mut usize) -> Result<u64> {
    Ok(u64::from_le_bytes(read_bytes(data, cursor, 8)?.try_into()?))
}

fn read_u128(data: &[u8], cursor: &mut usize) -> Result<u128> {
    Ok(u128::from_le_bytes(
        read_bytes(data, cursor, 16)?.try_into()?,
    ))
}

fn read_i8(data: &[u8], cursor: &mut usize) -> Result<i8> {
    Ok(read_u8(data, cursor)? as i8)
}

fn read_i16(data: &[u8], cursor: &mut usize) -> Result<i16> {
    Ok(i16::from_le_bytes(read_bytes(data, cursor, 2)?.try_into()?))
}

fn read_i32(data: &[u8], cursor: &mut usize) -> Result<i32> {
    Ok(i32::from_le_bytes(read_bytes(data, cursor, 4)?.try_into()?))
}

fn read_i64(data: &[u8], cursor: &mut usize) -> Result<i64> {
    Ok(i64::from_le_bytes(read_bytes(data, cursor, 8)?.try_into()?))
}

fn read_i128(data: &[u8], cursor: &mut usize) -> Result<i128> {
    Ok(i128::from_le_bytes(
        read_bytes(data, cursor, 16)?.try_into()?,
    ))
}

fn read_f32(data: &[u8], cursor: &mut usize) -> Result<f32> {
    Ok(f32::from_le_bytes(read_bytes(data, cursor, 4)?.try_into()?))
}

fn read_f64(data: &[u8], cursor: &mut usize) -> Result<f64> {
    Ok(f64::from_le_bytes(read_bytes(data, cursor, 8)?.try_into()?))
}

/// JSON has no NaN or Infinity. On-chain bytes are untrusted, so fall back to
/// the Rust text form rather than failing the whole argument map.
fn float_value(v: f64) -> Value {
    serde_json::Number::from_f64(v).map_or_else(|| json!(v.to_string()), Value::Number)
}

fn read_bytes<'a>(data: &'a [u8], cursor: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = cursor
        .checked_add(len)
        .filter(|e| *e <= data.len())
        .context("buffer underflow")?;
    let slice = &data[*cursor..end];
    *cursor = end;
    Ok(slice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use idl_drift::model::Idl;

    fn decode(idl: &Idl, name: &str, data: &[u8]) -> Value {
        let ix = idl.instructions.iter().find(|i| i.name == name).unwrap();
        decode_args(&type_map(idl), ix, data)
    }

    const SWAP_OLD: &str = r#"{
      "address": "Drift1111111111111111111111111111111111111",
      "metadata": {"name": "demo", "version": "0.1.0", "spec": "0.1.0"},
      "instructions": [{
        "name": "swap",
        "discriminator": [1,2,3,4,5,6,7,8],
        "accounts": [{"name": "pool", "writable": true}],
        "args": [{"name": "amount", "type": "u64"}]
      }]
    }"#;

    #[test]
    fn decodes_u64_arg_after_discriminator() {
        let idl = Idl::from_json(SWAP_OLD).unwrap();
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&42u64.to_le_bytes());
        let args = decode(&idl, "swap", &data);
        assert_eq!(args["amount"], json!(42));
    }

    /// Marginfi and other pre-0.30 Anchor programs declare `f64` args.
    const FLOAT_IDL: &str = r#"{
      "address": "MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA",
      "metadata": {"name": "demo", "version": "0.1.0", "spec": "0.1.0"},
      "instructions": [{
        "name": "configure",
        "discriminator": [1,2,3,4,5,6,7,8],
        "accounts": [],
        "args": [
          {"name": "ratio", "type": "f64"},
          {"name": "weight", "type": "f32"}
        ]
      }]
    }"#;

    #[test]
    fn decodes_float_args() {
        let idl = Idl::from_json(FLOAT_IDL).unwrap();
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&0.75f64.to_le_bytes());
        data.extend_from_slice(&1.5f32.to_le_bytes());
        let args = decode(&idl, "configure", &data);
        assert_eq!(args["ratio"], json!(0.75));
        assert_eq!(args["weight"], json!(1.5));
    }

    #[test]
    fn non_finite_float_falls_back_to_text() {
        let idl = Idl::from_json(FLOAT_IDL).unwrap();
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&f64::NAN.to_le_bytes());
        data.extend_from_slice(&1.0f32.to_le_bytes());
        let args = decode(&idl, "configure", &data);
        assert_eq!(args["ratio"], json!("NaN"));
        assert_eq!(args["weight"], json!(1.0));
    }

    #[test]
    fn returns_partial_on_truncated_payload() {
        let idl = Idl::from_json(SWAP_OLD).unwrap();
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 1, 2];
        let args = decode(&idl, "swap", &data);
        assert!(args.as_object().unwrap().is_empty());
    }

    /// One instruction `ix` with the given args and type definitions.
    fn idl_with(args: &str, types: &str) -> Idl {
        Idl::from_json(&format!(
            r#"{{
              "address": "Drift1111111111111111111111111111111111111",
              "metadata": {{"name": "demo", "version": "0.1.0", "spec": "0.1.0"}},
              "instructions": [{{
                "name": "ix",
                "discriminator": [1,2,3,4,5,6,7,8],
                "accounts": [],
                "args": {args}
              }}],
              "types": {types}
            }}"#
        ))
        .unwrap()
    }

    fn with_disc(payload: &[u8]) -> Vec<u8> {
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(payload);
        data
    }

    const EMPTY_STRUCT: &str = r#"[{"name": "Empty", "type": {"kind": "struct", "fields": []}}]"#;

    #[test]
    fn vec_count_above_remaining_bytes_is_rejected() {
        let idl = idl_with(
            r#"[{"name": "items", "type": {"vec": {"defined": {"name": "Empty"}}}}]"#,
            EMPTY_STRUCT,
        );
        let args = decode(&idl, "ix", &with_disc(&[5, 0, 0, 0]));
        assert!(args.get("items").is_none(), "{args}");
    }

    #[test]
    fn zero_size_vec_element_stops_decode() {
        let idl = idl_with(
            r#"[{"name": "items", "type": {"vec": {"defined": {"name": "Empty"}}}},
                {"name": "flag", "type": "u8"}]"#,
            EMPTY_STRUCT,
        );
        let args = decode(&idl, "ix", &with_disc(&[1, 0, 0, 0, 7]));
        assert!(args.as_object().unwrap().is_empty(), "{args}");
    }

    #[test]
    fn self_referential_alias_fails_without_stack_overflow() {
        let idl = idl_with(
            r#"[{"name": "x", "type": {"defined": {"name": "Loop"}}}]"#,
            r#"[{"name": "Loop", "type": {"kind": "type", "alias": {"defined": {"name": "Loop"}}}}]"#,
        );
        let args = decode(&idl, "ix", &with_disc(&[0; 16]));
        assert!(args.as_object().unwrap().is_empty());
    }

    #[test]
    fn zero_width_placeholders_stop_later_args() {
        for placeholder in [
            r#"{"defined": {"name": "Missing"}}"#,
            r#"{"generic": "T"}"#,
            r#"{"array": ["u8", "N"]}"#,
        ] {
            let idl = idl_with(
                &format!(
                    r#"[{{"name": "cfg", "type": {placeholder}}}, {{"name": "amount", "type": "u64"}}]"#
                ),
                "[]",
            );
            let args = decode(&idl, "ix", &with_disc(&9u64.to_le_bytes()));
            assert!(args.get("amount").is_none(), "{placeholder}: {args}");
        }
    }

    #[test]
    fn struct_field_error_does_not_shift_later_fields() {
        let idl = idl_with(
            r#"[{"name": "pair", "type": {"defined": {"name": "Pair"}}}]"#,
            r#"[{"name": "Pair", "type": {"kind": "struct", "fields": [
                {"name": "a", "type": "u64"}, {"name": "b", "type": "u32"}]}}]"#,
        );
        let args = decode(&idl, "ix", &with_disc(&[1, 0, 0, 0]));
        assert!(args.get("pair").is_none(), "{args}");
    }

    #[test]
    fn map_string_keys_are_plain_strings() {
        let idl = idl_with(
            r#"[{"name": "m", "type": {"map": ["string", "u64"]}}]"#,
            "[]",
        );
        let mut payload = 1u32.to_le_bytes().to_vec();
        payload.extend_from_slice(&3u32.to_le_bytes());
        payload.extend_from_slice(b"abc");
        payload.extend_from_slice(&5u64.to_le_bytes());
        let args = decode(&idl, "ix", &with_disc(&payload));
        assert_eq!(args["m"], json!({"abc": 5}));
    }

    #[test]
    fn exponential_struct_tree_hits_work_limit() {
        // T0 = {a: T1, b: T1}, ..., T20 = {}: 2^20 leaves from zero input bytes.
        let mut types: Vec<String> = (0..20)
            .map(|i| {
                format!(
                    r#"{{"name": "T{i}", "type": {{"kind": "struct", "fields": [
                        {{"name": "a", "type": {{"defined": {{"name": "T{n}"}}}}}},
                        {{"name": "b", "type": {{"defined": {{"name": "T{n}"}}}}}}]}}}}"#,
                    n = i + 1
                )
            })
            .collect();
        types.push(r#"{"name": "T20", "type": {"kind": "struct", "fields": []}}"#.into());
        let idl = idl_with(
            r#"[{"name": "x", "type": {"defined": {"name": "T0"}}},
                {"name": "y", "type": "u8"}]"#,
            &format!("[{}]", types.join(",")),
        );
        let args = decode(&idl, "ix", &with_disc(&[7]));
        assert!(args.as_object().unwrap().is_empty());
    }

    #[test]
    fn long_variant_names_hit_work_limit() {
        let name = "v".repeat(100_000);
        let idl = idl_with(
            r#"[{"name": "items", "type": {"vec": {"defined": {"name": "E"}}}}]"#,
            &format!(
                r#"[{{"name": "E", "type": {{"kind": "enum", "variants": [{{"name": "{name}"}}]}}}}]"#
            ),
        );
        let mut payload = 20u32.to_le_bytes().to_vec();
        payload.extend_from_slice(&[0; 20]);
        let args = decode(&idl, "ix", &with_disc(&payload));
        assert!(args.get("items").is_none());
    }
}
