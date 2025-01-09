use std::ops::{Add, Div, Mul, Sub};

#[derive(Debug, Clone, PartialEq)]
pub enum RuspyType {
    Int(i32),
    Int32(i32),
    Int64(i64),
    Float(f64),
    Float32(f32),
    Float64(f64),
    Char(char),
    Str(String),
    Str8(String),
    Str32(String),
    Str64(String),
}

impl RuspyType {
    pub fn is_compatible_with(&self, other: &RuspyType) -> bool {
        match (self, other) {
            // Allow Int64 to be compatible with Int
            (RuspyType::Int(_), RuspyType::Int64(_)) => true,
            (RuspyType::Int(_), RuspyType::Int(_)) => true,

            // Allow Int32 to be assigned to Int64 or Int
            (RuspyType::Int64(_), RuspyType::Int32(_)) => true,
            (RuspyType::Int(_), RuspyType::Int32(_)) => true,
            (RuspyType::Int32(_), RuspyType::Int32(_)) => true,

            // Keep existing Int64 compatibility
            (RuspyType::Int64(_), RuspyType::Int64(_)) => true,

            // Other types remain strictly matched
            (RuspyType::Float(_), RuspyType::Float(_)) => true,
            (RuspyType::Float32(_), RuspyType::Float32(_)) => true,
            (RuspyType::Float64(_), RuspyType::Float64(_)) => true,
            (RuspyType::Char(_), RuspyType::Char(_)) => true,
            (RuspyType::Str(_), RuspyType::Str(_)) => true,
            (RuspyType::Str8(_), RuspyType::Str8(_)) => true,
            (RuspyType::Str32(_), RuspyType::Str32(_)) => true,
            (RuspyType::Str64(_), RuspyType::Str64(_)) => true,
            _ => false,
        }
    }
}

impl Add for RuspyType {
    type Output = RuspyType;

    fn add(self, other: RuspyType) -> RuspyType {
        match (self, other) {
            (RuspyType::Int(a), RuspyType::Int(b)) => RuspyType::Int(a + b),
            (RuspyType::Int32(a), RuspyType::Int32(b)) => RuspyType::Int32(a + b),
            (RuspyType::Int64(a), RuspyType::Int64(b)) => RuspyType::Int64(a + b),
            (RuspyType::Float(a), RuspyType::Float(b)) => RuspyType::Float(a + b),
            (RuspyType::Float32(a), RuspyType::Float32(b)) => RuspyType::Float32(a + b),
            (RuspyType::Float64(a), RuspyType::Float64(b)) => RuspyType::Float64(a + b),
            (RuspyType::Str(a), RuspyType::Str(b)) => RuspyType::Str(a + &b),
            _ => panic!("Unsupported operand types for addition"),
        }
    }
}

impl Sub for RuspyType {
    type Output = RuspyType;

    fn sub(self, other: RuspyType) -> RuspyType {
        match (self, other) {
            (RuspyType::Int(a), RuspyType::Int(b)) => RuspyType::Int(a - b),
            (RuspyType::Int32(a), RuspyType::Int32(b)) => RuspyType::Int32(a - b),
            (RuspyType::Int64(a), RuspyType::Int64(b)) => RuspyType::Int64(a - b),
            (RuspyType::Float(a), RuspyType::Float(b)) => RuspyType::Float(a - b),
            (RuspyType::Float32(a), RuspyType::Float32(b)) => RuspyType::Float32(a - b),
            (RuspyType::Float64(a), RuspyType::Float64(b)) => RuspyType::Float64(a - b),
            _ => panic!("Unsupported operand types for subtraction"),
        }
    }
}

impl Mul for RuspyType {
    type Output = RuspyType;

    fn mul(self, other: RuspyType) -> RuspyType {
        match (self, other) {
            (RuspyType::Int(a), RuspyType::Int(b)) => RuspyType::Int(a * b),
            (RuspyType::Int32(a), RuspyType::Int32(b)) => RuspyType::Int32(a * b),
            (RuspyType::Int64(a), RuspyType::Int64(b)) => RuspyType::Int64(a * b),
            (RuspyType::Float(a), RuspyType::Float(b)) => RuspyType::Float(a * b),
            (RuspyType::Float32(a), RuspyType::Float32(b)) => RuspyType::Float32(a * b),
            (RuspyType::Float64(a), RuspyType::Float64(b)) => RuspyType::Float64(a * b),
            _ => panic!("Unsupported operand types for multiplication"),
        }
    }
}

impl Div for RuspyType {
    type Output = RuspyType;

    fn div(self, other: RuspyType) -> RuspyType {
        match (self, other) {
            (RuspyType::Int(a), RuspyType::Int(b)) => {
                if b == 0 {
                    panic!("Division by zero");
                }
                RuspyType::Int(a / b)
            }
            (RuspyType::Int32(a), RuspyType::Int32(b)) => {
                if b == 0 {
                    panic!("Division by zero");
                }
                RuspyType::Int32(a / b)
            }
            (RuspyType::Int64(a), RuspyType::Int64(b)) => {
                if b == 0 {
                    panic!("Division by zero");
                }
                RuspyType::Int64(a / b)
            }
            (RuspyType::Float(a), RuspyType::Float(b)) => {
                if b == 0.0 {
                    panic!("Division by zero");
                }
                RuspyType::Float(a / b)
            }
            (RuspyType::Float32(a), RuspyType::Float32(b)) => {
                if b == 0.0 {
                    panic!("Division by zero");
                }
                RuspyType::Float32(a / b)
            }
            (RuspyType::Float64(a), RuspyType::Float64(b)) => {
                if b == 0.0 {
                    panic!("Division by zero");
                }
                RuspyType::Float64(a / b)
            }
            _ => panic!("Unsupported operand types for division"),
        }
    }
}

// Implementation for type inference
#[allow(dead_code)]
pub fn infer_type(value: &str) -> RuspyType {
    if let Ok(i) = value.parse::<i32>() {
        RuspyType::Int(i)
    } else if let Ok(f) = value.parse::<f64>() {
        RuspyType::Float(f)
    } else if value.len() == 1 {
        RuspyType::Char(value.chars().next().unwrap())
    } else {
        RuspyType::Str(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_inference() {
        assert_eq!(infer_type("42"), RuspyType::Int(42));
        assert_eq!(infer_type("3.14"), RuspyType::Float(3.14));
        assert_eq!(infer_type("a"), RuspyType::Char('a'));
        assert_eq!(infer_type("hello"), RuspyType::Str("hello".to_string()));
    }

    #[test]
    fn test_type_compatibility() {
        let int_type = RuspyType::Int(0);
        let int64_type = RuspyType::Int64(0);
        let float_type = RuspyType::Float(0.0);

        assert!(int_type.is_compatible_with(&int64_type));
        assert!(!int_type.is_compatible_with(&float_type));
    }

    #[test]
    fn test_arithmetic_operations() {
        let a = RuspyType::Int(5);
        let b = RuspyType::Int(3);

        assert_eq!(a.clone() + b.clone(), RuspyType::Int(8));
        assert_eq!(a.clone() - b.clone(), RuspyType::Int(2));
        assert_eq!(a.clone() * b.clone(), RuspyType::Int(15));
        assert_eq!(a.clone() / b.clone(), RuspyType::Int(1));
    }

    #[test]
    #[should_panic(expected = "Division by zero")]
    fn test_division_by_zero() {
        let a = RuspyType::Int(5);
        let b = RuspyType::Int(0);
        let _ = a / b;
    }

    #[test]
    fn test_string_concatenation() {
        let a = RuspyType::Str("Hello, ".to_string());
        let b = RuspyType::Str("World!".to_string());
        assert_eq!(a + b, RuspyType::Str("Hello, World!".to_string()));
    }
}
