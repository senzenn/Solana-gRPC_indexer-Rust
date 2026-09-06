use anyhow::{Context, Result};
use idl_drift::model::{ArrayLen, EnumFields, Idl, Type, TypeDefTy};
use serde_json::{json, Value};

pub fn decode_args(idl: &Idl, instruction_name: &str, data: &[u8]) -> Result<Value> {
    let Some(ix) = idl
        .instructions
        .iter()
        .find(|i| i.name == instruction_name)
    else {
        return Ok(json!({}));
    };

    if data.len() < 8 {
        return Ok(json!({}));
    }
    let mut cursor = 8usize;
    let mut out = serde_json::Map::new();

    for arg in &ix.args {
        match decode_type(&idl.types, &arg.ty, data, &mut cursor) {
            Ok(v) => {
                out.insert(arg.name.clone(), v);
            }
            Err(_) => break,
        }
    }

    Ok(Value::Object(out))
}

fn decode_type(types: &[idl_drift::model::TypeDef], ty: &Type, data: &[u8], cursor: &mut usize) -> Result<Value> {
    match ty {
        Type::Bool => Ok(json!(read_bool(data, cursor)?)),
        Type::U8 => Ok(json!(read_u8(data, cursor)?)),
        Type::U16 => Ok(json!(read_u16(data, cursor)?)),
        Type::U32 => Ok(json!(read_u32(data, cursor)?)),
        Type::U64 => Ok(json!(read_u64(data, cursor)?)),
        Type::U128 => Ok(json!(read_u128(data, cursor)?.to_string())),
        Type::I8 => Ok(json!(read_i8(data, cursor)?)),
        Type::I16 => Ok(json!(read_i16(data, cursor)?)),
        Type::I32 => Ok(json!(read_i32(data, cursor)?)),
        Type::I64 => Ok(json!(read_i64(data, cursor)?)),
        Type::I128 => Ok(json!(read_i128(data, cursor)?.to_string())),
        Type::Pubkey => {
            let bytes = read_bytes(data, cursor, 32)?;
            Ok(json!(bs58::encode(bytes).into_string()))
        }
        Type::String => {
            let len = read_u32(data, cursor)? as usize;
            let bytes = read_bytes(data, cursor, len)?;
            Ok(json!(String::from_utf8_lossy(bytes)))
        }
        Type::Bytes => {
            let len = read_u32(data, cursor)? as usize;
            let bytes = read_bytes(data, cursor, len)?;
            Ok(json!(bs58::encode(bytes).into_string()))
        }
        Type::Option(inner) => {
            let tag = read_u8(data, cursor)?;
            if tag == 0 {
                Ok(Value::Null)
            } else {
                decode_type(types, inner, data, cursor)
            }
        }
        Type::Vec(inner) => {
            let len = read_u32(data, cursor)? as usize;
            let mut items = Vec::new();
            for _ in 0..len {
                items.push(decode_type(types, inner, data, cursor)?);
            }
            Ok(Value::Array(items))
        }
        Type::Array(inner, len) => {
            let n = match len {
                ArrayLen::Value(v) => *v,
                ArrayLen::Generic(_) => return Ok(Value::Null),
            };
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(decode_type(types, inner, data, cursor)?);
            }
            Ok(Value::Array(items))
        }
        Type::Tuple(items) => {
            let mut out = Vec::new();
            for item in items {
                out.push(decode_type(types, item, data, cursor)?);
            }
            Ok(Value::Array(out))
        }
        Type::Defined { name, .. } => decode_defined(types, name, data, cursor),
        Type::Map(key, value) => {
            let len = read_u32(data, cursor)? as usize;
            let mut map = serde_json::Map::new();
            for _ in 0..len {
                let k = decode_type(types, key, data, cursor)?;
                let v = decode_type(types, value, data, cursor)?;
                map.insert(k.to_string(), v);
            }
            Ok(Value::Object(map))
        }
        Type::Set(inner) => {
            let len = read_u32(data, cursor)? as usize;
            let mut items = Vec::new();
            for _ in 0..len {
                items.push(decode_type(types, inner, data, cursor)?);
            }
            Ok(Value::Array(items))
        }
        Type::Generic(_) => Ok(Value::Null),
    }
}

fn decode_defined(types: &[idl_drift::model::TypeDef], name: &str, data: &[u8], cursor: &mut usize) -> Result<Value> {
    let Some(def) = types.iter().find(|t| t.name == name) else {
        return Ok(json!({}));
    };
    match &def.ty {
        TypeDefTy::Struct { fields } => {
            let mut map = serde_json::Map::new();
            for field in fields {
                if let Ok(v) = decode_type(types, &field.ty, data, cursor) {
                    map.insert(field.name.clone(), v);
                }
            }
            Ok(Value::Object(map))
        }
        TypeDefTy::Enum { variants } => {
            let disc = read_u8(data, cursor)?;
            if let Some(variant) = variants.get(disc as usize) {
                let mut map = serde_json::Map::new();
                map.insert("variant".into(), json!(variant.name));
                match &variant.fields {
                    EnumFields::Named(fields) => {
                        for field in fields {
                            if let Ok(v) = decode_type(types, &field.ty, data, cursor) {
                                map.insert(field.name.clone(), v);
                            }
                        }
                    }
                    EnumFields::Tuple(types_) => {
                        let mut arr = Vec::new();
                        for t in types_ {
                            arr.push(decode_type(types, t, data, cursor)?);
                        }
                        map.insert("fields".into(), Value::Array(arr));
                    }
                }
                Ok(Value::Object(map))
            } else {
                Ok(json!({ "variant_index": disc }))
            }
        }
        TypeDefTy::Type { alias } => decode_type(types, alias, data, cursor),
    }
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
    Ok(u128::from_le_bytes(read_bytes(data, cursor, 16)?.try_into()?))
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
    Ok(i128::from_le_bytes(read_bytes(data, cursor, 16)?.try_into()?))
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
        let args = decode_args(&idl, "swap", &data).unwrap();
        assert_eq!(args["amount"], json!(42));
    }

    #[test]
    fn returns_empty_object_for_unknown_instruction() {
        let idl = Idl::from_json(SWAP_OLD).unwrap();
        let args = decode_args(&idl, "missing", &[0; 8]).unwrap();
        assert!(args.as_object().unwrap().is_empty());
    }

    #[test]
    fn returns_partial_on_truncated_payload() {
        let idl = Idl::from_json(SWAP_OLD).unwrap();
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 1, 2];
        let args = decode_args(&idl, "swap", &data).unwrap();
        assert!(args.as_object().unwrap().is_empty());
    }
}
