use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde_json::{Value, json};

pub struct Calculate;

#[async_trait]
impl Tool for Calculate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "calculate".into(),
            description: "Evaluate numeric arithmetic locally instead of guessing. Supports finite decimal/scientific numbers, parentheses, +, -, *, /, %, and right-associative ^ powers. Uses IEEE-754 double precision; unsuitable for exact decimal accounting. No code execution or network access.".into(),
            input_schema: json!({"type":"object","properties":{"expression":{"type":"string","maxLength":512}},"required":["expression"],"additionalProperties":false}),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, _context: &ToolContext) -> Result<String, ToolError> {
        let expression = input["expression"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("expression must be a string".into()))?;
        evaluate(expression)
            .map(|result| format!("{expression} = {result}"))
            .map_err(ToolError::InvalidInput)
    }
}

fn evaluate(expression: &str) -> Result<f64, String> {
    if expression.is_empty() || expression.len() > 512 || !expression.is_ascii() {
        return Err("Use 1 to 512 ASCII characters of numeric arithmetic.".into());
    }
    let mut parser = Parser {
        input: expression.as_bytes(),
        at: 0,
        depth: 0,
    };
    let value = parser.sum()?;
    parser.space();
    if parser.at != parser.input.len() {
        return Err(format!(
            "Unexpected character at position {}",
            parser.at + 1
        ));
    }
    finite(value)
}

fn finite(value: f64) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err("Calculation produced a non-finite result or an invalid power.".into())
    }
}

struct Parser<'a> {
    input: &'a [u8],
    at: usize,
    depth: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.input.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }
    fn take(&mut self, byte: u8) -> bool {
        self.space();
        if self.input.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn sum(&mut self) -> Result<f64, String> {
        let mut value = self.product()?;
        loop {
            if self.take(b'+') {
                value = finite(value + self.product()?)?;
            } else if self.take(b'-') {
                value = finite(value - self.product()?)?;
            } else {
                return Ok(value);
            }
        }
    }
    fn product(&mut self) -> Result<f64, String> {
        let mut value = self.unary()?;
        loop {
            if self.take(b'*') {
                value = finite(value * self.unary()?)?;
            } else if self.take(b'/') {
                let divisor = self.unary()?;
                if divisor == 0.0 {
                    return Err("Division by zero.".into());
                }
                value = finite(value / divisor)?;
            } else if self.take(b'%') {
                let divisor = self.unary()?;
                if divisor == 0.0 {
                    return Err("Remainder by zero.".into());
                }
                value = finite(value % divisor)?;
            } else {
                return Ok(value);
            }
        }
    }
    fn unary(&mut self) -> Result<f64, String> {
        self.depth += 1;
        if self.depth > 32 {
            return Err("Expression nesting exceeds 32 levels.".into());
        }
        let result = if self.take(b'+') {
            self.unary()
        } else if self.take(b'-') {
            self.unary().map(|value| -value)
        } else {
            let value = self.atom()?;
            if self.take(b'^') {
                finite(value.powf(self.unary()?))
            } else {
                Ok(value)
            }
        };
        self.depth -= 1;
        result
    }
    fn atom(&mut self) -> Result<f64, String> {
        if self.take(b'(') {
            let value = self.sum()?;
            if !self.take(b')') {
                return Err("Missing closing parenthesis.".into());
            }
            return Ok(value);
        }
        self.space();
        let start = self.at;
        while self
            .input
            .get(self.at)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
        {
            self.at += 1;
        }
        if matches!(self.input.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.input.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            while self.input.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
        }
        let text = std::str::from_utf8(&self.input[start..self.at]).unwrap_or_default();
        let value = text
            .parse::<f64>()
            .map_err(|_| format!("Expected a number at position {}", start + 1))?;
        finite(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_unary_powers_and_scientific_numbers() {
        for (input, expected) in [
            ("2+3*4", 14.0),
            ("(2+3)*4", 20.0),
            ("-2^2", -4.0),
            ("2^3^2", 512.0),
            ("2^-2", 0.25),
            ("1.25e3 / 5", 250.0),
            ("10%3", 1.0),
        ] {
            assert_eq!(evaluate(input).unwrap(), expected, "{input}");
        }
    }
    #[test]
    fn rejects_invalid_unbounded_and_executable_input() {
        for input in [
            "",
            "1/0",
            "2%0",
            "(-1)^0.5",
            "1e999",
            "1e",
            "1..2",
            "(2",
            "2)",
            "1 + import os",
            "NaN",
            "1;system('ls')",
        ] {
            assert!(evaluate(input).is_err(), "{input}");
        }
        assert!(evaluate(&"(".repeat(100)).is_err());
        assert!(evaluate(&"-".repeat(100)).is_err());
    }
}
