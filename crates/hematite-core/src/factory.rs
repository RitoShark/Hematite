//! ValueFactory — JSON → PropertyValue conversion.
//!
//! Centralizes value creation that was previously scattered across `applier.rs`.
//! The fix config specifies values as JSON; this module converts them to
//! [`PropertyValue`] instances based on the declared [`BinDataType`].
//!
//! Also handles type conversions (e.g. vec3 → vec4) for the `ChangeFieldType`
//! transform action.

use crate::strings::fnv1a_hash;
use anyhow::{anyhow, Result};
use hematite_types::bin::PropertyValue;

/// Convert a JSON value to a PropertyValue based on the declared data type.
///
/// # Arguments
/// * `value` - The JSON value to convert
/// * `data_type` - The target BIN data type (matches BinDataType enum names)
///
/// # Returns
/// The converted PropertyValue, or an error if conversion fails
pub fn json_to_value(value: &serde_json::Value, data_type: &str) -> Result<PropertyValue> {
    let type_lower = data_type.to_lowercase();

    match type_lower.as_str() {
        "bool" => {
            let v = value.as_bool().ok_or_else(|| anyhow!("Expected bool"))?;
            Ok(PropertyValue::Bool(v))
        }
        "u8" => {
            let v = value.as_u64().ok_or_else(|| anyhow!("Expected u8"))? as u8;
            Ok(PropertyValue::U8(v))
        }
        "i8" => {
            let v = value.as_i64().ok_or_else(|| anyhow!("Expected i8"))? as i8;
            Ok(PropertyValue::I8(v))
        }
        "u16" => {
            let v = value.as_u64().ok_or_else(|| anyhow!("Expected u16"))? as u16;
            Ok(PropertyValue::U16(v))
        }
        "i16" => {
            let v = value.as_i64().ok_or_else(|| anyhow!("Expected i16"))? as i16;
            Ok(PropertyValue::I16(v))
        }
        "u32" => {
            let v = value.as_u64().ok_or_else(|| anyhow!("Expected u32"))? as u32;
            Ok(PropertyValue::U32(v))
        }
        "i32" => {
            let v = value.as_i64().ok_or_else(|| anyhow!("Expected i32"))? as i32;
            Ok(PropertyValue::I32(v))
        }
        "u64" => {
            let v = value.as_u64().ok_or_else(|| anyhow!("Expected u64"))?;
            Ok(PropertyValue::U64(v))
        }
        "i64" => {
            let v = value.as_i64().ok_or_else(|| anyhow!("Expected i64"))?;
            Ok(PropertyValue::I64(v))
        }
        "f32" => {
            let v = value.as_f64().ok_or_else(|| anyhow!("Expected f32"))? as f32;
            Ok(PropertyValue::F32(v))
        }
        "vector2" | "vec2" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("Expected array for vec2"))?;
            if arr.len() != 2 {
                return Err(anyhow!("Vec2 requires exactly 2 elements"));
            }
            let x = arr[0]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec2[0]"))? as f32;
            let y = arr[1]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec2[1]"))? as f32;
            Ok(PropertyValue::Vector2([x, y]))
        }
        "vector3" | "vec3" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("Expected array for vec3"))?;
            if arr.len() != 3 {
                return Err(anyhow!("Vec3 requires exactly 3 elements"));
            }
            let x = arr[0]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec3[0]"))? as f32;
            let y = arr[1]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec3[1]"))? as f32;
            let z = arr[2]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec3[2]"))? as f32;
            Ok(PropertyValue::Vector3([x, y, z]))
        }
        "vector4" | "vec4" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("Expected array for vec4"))?;
            if arr.len() != 4 {
                return Err(anyhow!("Vec4 requires exactly 4 elements"));
            }
            let x = arr[0]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec4[0]"))? as f32;
            let y = arr[1]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec4[1]"))? as f32;
            let z = arr[2]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec4[2]"))? as f32;
            let w = arr[3]
                .as_f64()
                .ok_or_else(|| anyhow!("Expected f32 for vec4[3]"))? as f32;
            Ok(PropertyValue::Vector4([x, y, z, w]))
        }
        "string" => {
            let v = value.as_str().ok_or_else(|| anyhow!("Expected string"))?;
            Ok(PropertyValue::String(v.to_string()))
        }
        "hash" => {
            let v = value
                .as_u64()
                .ok_or_else(|| anyhow!("Expected u32 for hash"))? as u32;
            Ok(PropertyValue::Hash(v))
        }
        "link" => {
            let v = value
                .as_u64()
                .ok_or_else(|| anyhow!("Expected u32 for link"))? as u32;
            Ok(PropertyValue::Link(v))
        }
        "color" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("Expected array for color"))?;
            if arr.len() != 4 {
                return Err(anyhow!("Color requires exactly 4 elements (RGBA)"));
            }
            let r = arr[0]
                .as_u64()
                .ok_or_else(|| anyhow!("Expected u8 for color[0]"))? as u8;
            let g = arr[1]
                .as_u64()
                .ok_or_else(|| anyhow!("Expected u8 for color[1]"))? as u8;
            let b = arr[2]
                .as_u64()
                .ok_or_else(|| anyhow!("Expected u8 for color[2]"))? as u8;
            let a = arr[3]
                .as_u64()
                .ok_or_else(|| anyhow!("Expected u8 for color[3]"))? as u8;
            Ok(PropertyValue::Color([r, g, b, a]))
        }
        _ => Err(anyhow!("Unknown data type: {}", data_type)),
    }
}

/// Convert a PropertyValue from one type to another.
///
/// # Known conversions
/// - vec2 → vec3 (append z from config, default 0.0)
/// - vec3 → vec4 (append w from config, default 1.0 for alpha)
/// - vec2 → vec4 (append z,w from config)
/// - link/hash → string (convert to hex representation)
/// - string → link/hash (parse hex or compute FNV-1a)
/// - u8 → u32 (upcast)
/// - u32 → u8 (downcast with clamping)
/// - f32 → string (to_string)
///
/// # Arguments
/// * `value` - The value to convert
/// * `from` - Source type name (case-insensitive)
/// * `to` - Target type name (case-insensitive)
/// * `append_values` - Values to append during conversion (e.g. alpha for vec3→vec4)
///
/// # Returns
/// Some(converted_value) if conversion is supported, None if not applicable
pub fn convert_type(
    value: &PropertyValue,
    from: &str,
    to: &str,
    append_values: &[serde_json::Value],
) -> Result<Option<PropertyValue>> {
    let from_lower = from.to_lowercase();
    let to_lower = to.to_lowercase();

    match (from_lower.as_str(), to_lower.as_str()) {
        ("vec2" | "vector2", "vec3" | "vector3") => {
            if let PropertyValue::Vector2([x, y]) = value {
                let z = append_values
                    .first()
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(0.0);
                Ok(Some(PropertyValue::Vector3([*x, *y, z])))
            } else {
                Ok(None)
            }
        }
        ("vec3" | "vector3", "vec4" | "vector4") => {
            if let PropertyValue::Vector3([x, y, z]) = value {
                let w = append_values
                    .first()
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(1.0);
                Ok(Some(PropertyValue::Vector4([*x, *y, *z, w])))
            } else {
                Ok(None)
            }
        }
        ("vec2" | "vector2", "vec4" | "vector4") => {
            if let PropertyValue::Vector2([x, y]) = value {
                let z = append_values
                    .first()
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(0.0);
                let w = append_values
                    .get(1)
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(1.0);
                Ok(Some(PropertyValue::Vector4([*x, *y, z, w])))
            } else {
                Ok(None)
            }
        }

        ("link" | "hash", "string") => match value {
            PropertyValue::Link(h) | PropertyValue::Hash(h) => {
                Ok(Some(PropertyValue::String(format!("{:08x}", h))))
            }
            _ => Ok(None),
        },

        ("string", "link" | "hash") => {
            if let PropertyValue::String(s) = value {
                let hash = u32::from_str_radix(s.trim_start_matches("0x"), 16)
                    .unwrap_or_else(|_| fnv1a_hash(s));

                if to_lower == "link" {
                    Ok(Some(PropertyValue::Link(hash)))
                } else {
                    Ok(Some(PropertyValue::Hash(hash)))
                }
            } else {
                Ok(None)
            }
        }
        ("u8", "u32") => {
            if let PropertyValue::U8(v) = value {
                Ok(Some(PropertyValue::U32(*v as u32)))
            } else {
                Ok(None)
            }
        }
        ("u32", "u8") => {
            if let PropertyValue::U32(v) = value {
                let clamped = (*v).min(255) as u8;
                Ok(Some(PropertyValue::U8(clamped)))
            } else {
                Ok(None)
            }
        }
        ("u16", "u32") => {
            if let PropertyValue::U16(v) = value {
                Ok(Some(PropertyValue::U32(*v as u32)))
            } else {
                Ok(None)
            }
        }
        ("i8", "i32") => {
            if let PropertyValue::I8(v) = value {
                Ok(Some(PropertyValue::I32(*v as i32)))
            } else {
                Ok(None)
            }
        }
        ("i16", "i32") => {
            if let PropertyValue::I16(v) = value {
                Ok(Some(PropertyValue::I32(*v as i32)))
            } else {
                Ok(None)
            }
        }

        ("f32", "string") => {
            if let PropertyValue::F32(v) = value {
                Ok(Some(PropertyValue::String(v.to_string())))
            } else {
                Ok(None)
            }
        }

        ("bool", "u8") => {
            if let PropertyValue::Bool(v) = value {
                Ok(Some(PropertyValue::U8(if *v { 1 } else { 0 })))
            } else {
                Ok(None)
            }
        }

        _ => Err(anyhow!("Unsupported type conversion: {} → {}", from, to)),
    }
}

/// Check if a PropertyValue matches a JSON expected value.
///
/// Used by detection rules to compare current field values against expected.
///
/// # Arguments
/// * `value` - The PropertyValue to check
/// * `expected` - The expected JSON value
///
/// # Returns
/// true if the value matches the expected value
pub fn matches_json(value: &PropertyValue, expected: &serde_json::Value) -> bool {
    match (value, expected) {
        (PropertyValue::Bool(v), serde_json::Value::Bool(e)) => v == e,
        (PropertyValue::U8(v), serde_json::Value::Number(n)) => {
            n.as_u64().map(|e| *v as u64 == e).unwrap_or(false)
        }
        (PropertyValue::I8(v), serde_json::Value::Number(n)) => {
            n.as_i64().map(|e| *v as i64 == e).unwrap_or(false)
        }
        (PropertyValue::U16(v), serde_json::Value::Number(n)) => {
            n.as_u64().map(|e| *v as u64 == e).unwrap_or(false)
        }
        (PropertyValue::I16(v), serde_json::Value::Number(n)) => {
            n.as_i64().map(|e| *v as i64 == e).unwrap_or(false)
        }
        (PropertyValue::U32(v), serde_json::Value::Number(n)) => {
            n.as_u64().map(|e| *v as u64 == e).unwrap_or(false)
        }
        (PropertyValue::I32(v), serde_json::Value::Number(n)) => {
            n.as_i64().map(|e| *v as i64 == e).unwrap_or(false)
        }
        (PropertyValue::U64(v), serde_json::Value::Number(n)) => {
            n.as_u64().map(|e| *v == e).unwrap_or(false)
        }
        (PropertyValue::I64(v), serde_json::Value::Number(n)) => {
            n.as_i64().map(|e| *v == e).unwrap_or(false)
        }

        (PropertyValue::F32(v), serde_json::Value::Number(n)) => n
            .as_f64()
            .map(|e| (*v - e as f32).abs() < 0.0001)
            .unwrap_or(false),

        (PropertyValue::String(v), serde_json::Value::String(e)) => v == e,
        (PropertyValue::Hash(v), serde_json::Value::Number(n)) => {
            n.as_u64().map(|e| *v as u64 == e).unwrap_or(false)
        }
        (PropertyValue::Link(v), serde_json::Value::Number(n)) => {
            n.as_u64().map(|e| *v as u64 == e).unwrap_or(false)
        }
        (PropertyValue::WadHash(v), serde_json::Value::Number(n)) => {
            n.as_u64().map(|e| *v == e).unwrap_or(false)
        }

        (PropertyValue::Vector2([x, y]), serde_json::Value::Array(arr)) => {
            arr.len() == 2
                && arr[0]
                    .as_f64()
                    .map(|e| (*x - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
                && arr[1]
                    .as_f64()
                    .map(|e| (*y - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
        }
        (PropertyValue::Vector3([x, y, z]), serde_json::Value::Array(arr)) => {
            arr.len() == 3
                && arr[0]
                    .as_f64()
                    .map(|e| (*x - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
                && arr[1]
                    .as_f64()
                    .map(|e| (*y - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
                && arr[2]
                    .as_f64()
                    .map(|e| (*z - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
        }
        (PropertyValue::Vector4([x, y, z, w]), serde_json::Value::Array(arr)) => {
            arr.len() == 4
                && arr[0]
                    .as_f64()
                    .map(|e| (*x - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
                && arr[1]
                    .as_f64()
                    .map(|e| (*y - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
                && arr[2]
                    .as_f64()
                    .map(|e| (*z - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
                && arr[3]
                    .as_f64()
                    .map(|e| (*w - e as f32).abs() < 0.0001)
                    .unwrap_or(false)
        }

        (PropertyValue::Color([r, g, b, a]), serde_json::Value::Array(arr)) => {
            arr.len() == 4
                && arr[0].as_u64().map(|e| *r as u64 == e).unwrap_or(false)
                && arr[1].as_u64().map(|e| *g as u64 == e).unwrap_or(false)
                && arr[2].as_u64().map(|e| *b as u64 == e).unwrap_or(false)
                && arr[3].as_u64().map(|e| *a as u64 == e).unwrap_or(false)
        }

        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_to_value_primitives() {
        assert!(matches!(
            json_to_value(&serde_json::json!(true), "bool").unwrap(),
            PropertyValue::Bool(true)
        ));

        assert!(matches!(
            json_to_value(&serde_json::json!(42), "u8").unwrap(),
            PropertyValue::U8(42)
        ));

        assert!(matches!(
            json_to_value(&serde_json::json!(-5), "i8").unwrap(),
            PropertyValue::I8(-5)
        ));

        assert!(matches!(
            json_to_value(&serde_json::json!(2.5), "f32").unwrap(),
            PropertyValue::F32(_)
        ));
    }

    #[test]
    fn test_json_to_value_string() {
        let result = json_to_value(&serde_json::json!("test.tex"), "string").unwrap();
        match result {
            PropertyValue::String(s) => assert_eq!(s, "test.tex"),
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_json_to_value_vectors() {
        let vec2 = json_to_value(&serde_json::json!([1.0, 2.0]), "vec2").unwrap();
        assert!(matches!(vec2, PropertyValue::Vector2([1.0, 2.0])));

        let vec3 = json_to_value(&serde_json::json!([1.0, 2.0, 3.0]), "vec3").unwrap();
        assert!(matches!(vec3, PropertyValue::Vector3([1.0, 2.0, 3.0])));

        let vec4 = json_to_value(&serde_json::json!([1.0, 2.0, 3.0, 4.0]), "vec4").unwrap();
        assert!(matches!(vec4, PropertyValue::Vector4([1.0, 2.0, 3.0, 4.0])));
    }

    #[test]
    fn test_json_to_value_color() {
        let color = json_to_value(&serde_json::json!([255, 128, 64, 32]), "color").unwrap();
        assert!(matches!(color, PropertyValue::Color([255, 128, 64, 32])));
    }

    #[test]
    fn test_convert_vec3_to_vec4() {
        let vec3 = PropertyValue::Vector3([1.0, 2.0, 3.0]);
        let append = vec![serde_json::json!(1.0)];

        let result = convert_type(&vec3, "vec3", "vec4", &append).unwrap();

        match result {
            Some(PropertyValue::Vector4([x, y, z, w])) => {
                assert_eq!(x, 1.0);
                assert_eq!(y, 2.0);
                assert_eq!(z, 3.0);
                assert_eq!(w, 1.0);
            }
            _ => panic!("Expected vec4"),
        }
    }

    #[test]
    fn test_convert_vec3_to_vec4_default_alpha() {
        let vec3 = PropertyValue::Vector3([1.0, 2.0, 3.0]);

        let result = convert_type(&vec3, "vec3", "vec4", &[]).unwrap();

        match result {
            Some(PropertyValue::Vector4([_, _, _, w])) => {
                assert_eq!(w, 1.0); // Default alpha
            }
            _ => panic!("Expected vec4"),
        }
    }

    #[test]
    fn test_convert_hash_to_string() {
        let hash = PropertyValue::Hash(0xDEADBEEF);

        let result = convert_type(&hash, "hash", "string", &[]).unwrap();

        match result {
            Some(PropertyValue::String(s)) => assert_eq!(s, "deadbeef"),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn test_convert_string_to_hash() {
        let string = PropertyValue::String("test".to_string());

        let result = convert_type(&string, "string", "hash", &[]).unwrap();

        match result {
            Some(PropertyValue::Hash(h)) => assert!(h != 0), // FNV-1a hash should be non-zero
            _ => panic!("Expected hash"),
        }
    }

    #[test]
    fn test_convert_u8_to_u32() {
        let u8_val = PropertyValue::U8(42);

        let result = convert_type(&u8_val, "u8", "u32", &[]).unwrap();

        assert!(matches!(result, Some(PropertyValue::U32(42))));
    }

    #[test]
    fn test_convert_u32_to_u8_clamped() {
        let u32_val = PropertyValue::U32(300);

        let result = convert_type(&u32_val, "u32", "u8", &[]).unwrap();

        assert!(matches!(result, Some(PropertyValue::U8(255)))); // Clamped to max u8
    }

    #[test]
    fn test_matches_json_bool() {
        assert!(matches_json(
            &PropertyValue::Bool(true),
            &serde_json::json!(true)
        ));
        assert!(!matches_json(
            &PropertyValue::Bool(true),
            &serde_json::json!(false)
        ));
    }

    #[test]
    fn test_matches_json_integer() {
        assert!(matches_json(
            &PropertyValue::U32(42),
            &serde_json::json!(42)
        ));
        assert!(!matches_json(
            &PropertyValue::U32(42),
            &serde_json::json!(43)
        ));
    }

    #[test]
    fn test_matches_json_string() {
        assert!(matches_json(
            &PropertyValue::String("test".to_string()),
            &serde_json::json!("test")
        ));
        assert!(!matches_json(
            &PropertyValue::String("test".to_string()),
            &serde_json::json!("other")
        ));
    }

    #[test]
    fn test_matches_json_vec3() {
        assert!(matches_json(
            &PropertyValue::Vector3([1.0, 2.0, 3.0]),
            &serde_json::json!([1.0, 2.0, 3.0])
        ));
        assert!(!matches_json(
            &PropertyValue::Vector3([1.0, 2.0, 3.0]),
            &serde_json::json!([1.0, 2.0, 4.0])
        ));
    }

    #[test]
    fn test_matches_json_type_mismatch() {
        assert!(!matches_json(
            &PropertyValue::U32(42),
            &serde_json::json!("42")
        ));
        assert!(!matches_json(
            &PropertyValue::String("42".to_string()),
            &serde_json::json!(42)
        ));
    }
}
