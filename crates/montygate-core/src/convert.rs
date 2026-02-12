use monty::MontyObject;
use serde_json::Value;

/// Convert a `MontyObject` to a `serde_json::Value`.
///
/// Mapping:
/// - None -> Null
/// - Bool -> Bool
/// - Int -> Number (i64)
/// - Float -> Number (f64)
/// - String -> String
/// - List/Tuple -> Array (recursive)
/// - Dict -> Object (string keys, recursive values)
/// - BigInt -> Number (i64) or String
/// - Everything else -> String via py_repr()
pub fn monty_to_json(obj: &MontyObject) -> Value {
    match obj {
        MontyObject::None => Value::Null,
        MontyObject::Bool(b) => Value::Bool(*b),
        MontyObject::Int(n) => Value::Number((*n).into()),
        MontyObject::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        MontyObject::String(s) => Value::String(s.clone()),
        MontyObject::List(items) | MontyObject::Tuple(items) => {
            Value::Array(items.iter().map(monty_to_json).collect())
        }
        MontyObject::Dict(pairs) => {
            let mut map = serde_json::Map::new();
            for (k, v) in pairs {
                let key = match k {
                    MontyObject::String(s) => s.clone(),
                    other => other.py_repr(),
                };
                map.insert(key, monty_to_json(v));
            }
            Value::Object(map)
        }
        MontyObject::BigInt(n) => {
            if let Ok(n) = i64::try_from(n) {
                Value::Number(n.into())
            } else {
                Value::String(n.to_string())
            }
        }
        other => Value::String(other.py_repr()),
    }
}

/// Convert a `serde_json::Value` to a `MontyObject`.
///
/// Mapping:
/// - Null -> None
/// - Bool -> Bool
/// - Number(i64) -> Int, Number(f64) -> Float
/// - String -> String
/// - Array -> List (recursive)
/// - Object -> Dict (recursive)
pub fn json_to_monty(value: &Value) -> MontyObject {
    match value {
        Value::Null => MontyObject::None,
        Value::Bool(b) => MontyObject::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                MontyObject::Int(i)
            } else if let Some(f) = n.as_f64() {
                MontyObject::Float(f)
            } else {
                MontyObject::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => MontyObject::String(s.clone()),
        Value::Array(arr) => MontyObject::List(arr.iter().map(json_to_monty).collect()),
        Value::Object(map) => {
            let pairs = map
                .iter()
                .map(|(k, v)| (MontyObject::String(k.clone()), json_to_monty(v)))
                .collect();
            MontyObject::Dict(pairs)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_null() {
        let v = json!(null);
        let obj = json_to_monty(&v);
        assert!(matches!(obj, MontyObject::None));
        assert_eq!(monty_to_json(&obj), v);
    }

    #[test]
    fn roundtrip_bool() {
        let v = json!(true);
        let obj = json_to_monty(&v);
        assert!(matches!(obj, MontyObject::Bool(true)));
        assert_eq!(monty_to_json(&obj), v);
    }

    #[test]
    fn roundtrip_int() {
        let v = json!(42);
        let obj = json_to_monty(&v);
        assert!(matches!(obj, MontyObject::Int(42)));
        assert_eq!(monty_to_json(&obj), v);
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn roundtrip_float() {
        let v = json!(3.14);
        let obj = json_to_monty(&v);
        assert!(matches!(obj, MontyObject::Float(_)));
        assert_eq!(monty_to_json(&obj), v);
    }

    #[test]
    fn roundtrip_string() {
        let v = json!("hello");
        let obj = json_to_monty(&v);
        assert!(matches!(obj, MontyObject::String(_)));
        assert_eq!(monty_to_json(&obj), v);
    }

    #[test]
    fn roundtrip_array() {
        let v = json!([1, "two", null]);
        let obj = json_to_monty(&v);
        assert!(matches!(obj, MontyObject::List(_)));
        assert_eq!(monty_to_json(&obj), v);
    }

    #[test]
    fn roundtrip_object() {
        let v = json!({"name": "Alice", "age": 30});
        let obj = json_to_monty(&v);
        let result = monty_to_json(&obj);
        assert_eq!(result["name"], "Alice");
        assert_eq!(result["age"], 30);
    }

    #[test]
    fn nested_structure() {
        let v = json!({
            "rows": [
                {"id": 1, "name": "a"},
                {"id": 2, "name": "b"}
            ],
            "count": 2
        });
        let obj = json_to_monty(&v);
        let result = monty_to_json(&obj);
        assert_eq!(result["count"], 2);
        assert_eq!(result["rows"][0]["name"], "a");
    }
}
