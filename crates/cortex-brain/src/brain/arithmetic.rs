//! Bounded exact arithmetic. Explanations are emitted by the operations executed.

use std::fmt;

const LIMIT: i128 = 1_000_000_000_000_000_000_000_000;
const MAX_TOKENS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Number {
    pub numerator: i128,
    pub denominator: i128,
}

impl Number {
    pub fn new(numerator: i128, denominator: i128) -> Result<Self, String> {
        if denominator == 0 {
            return Err("Division by zero is undefined.".into());
        }
        if numerator.unsigned_abs() > LIMIT as u128 || denominator.unsigned_abs() > LIMIT as u128 {
            return Err("This calculation exceeds the exact arithmetic budget.".into());
        }
        let mut a = numerator.abs();
        let mut b = denominator.abs();
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let divisor = a.max(1) * denominator.signum();
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    pub fn integer(value: i128) -> Self {
        Self {
            numerator: value,
            denominator: 1,
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let (text, sign) = text.strip_prefix('-').map_or((text, 1), |text| (text, -1));
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if fraction.len() > 12 || whole.len() > 15 {
            return Err("Use at most 15 integer digits and 12 decimal places.".into());
        }
        let scale = 10_i128.pow(fraction.len() as u32);
        let whole: i128 = whole
            .parse()
            .map_err(|_| "Expected a number.".to_string())?;
        let fraction: i128 = if fraction.is_empty() {
            0
        } else {
            fraction
                .parse()
                .map_err(|_| "Invalid decimal.".to_string())?
        };
        Self::new(sign * (whole * scale + fraction), scale)
    }

    pub fn apply(self, operator: char, rhs: Self) -> Result<Self, String> {
        let checked = |value: Option<i128>| value.ok_or_else(|| "Arithmetic overflow.".to_string());
        match operator {
            '+' | '-' => {
                let a = checked(self.numerator.checked_mul(rhs.denominator))?;
                let b = checked(rhs.numerator.checked_mul(self.denominator))?;
                let numerator = checked(if operator == '+' {
                    a.checked_add(b)
                } else {
                    a.checked_sub(b)
                })?;
                Self::new(
                    numerator,
                    checked(self.denominator.checked_mul(rhs.denominator))?,
                )
            }
            '*' => Self::new(
                checked(self.numerator.checked_mul(rhs.numerator))?,
                checked(self.denominator.checked_mul(rhs.denominator))?,
            ),
            '/' => Self::new(
                checked(self.numerator.checked_mul(rhs.denominator))?,
                checked(self.denominator.checked_mul(rhs.numerator))?,
            ),
            '^' => {
                if rhs.denominator != 1 || rhs.numerator.abs() > 12 {
                    return Err("Exponents must be whole numbers between -12 and 12.".into());
                }
                let exponent = rhs.numerator.unsigned_abs() as u32;
                if self.numerator == 0 && rhs.numerator <= 0 {
                    return Err("Zero to a nonpositive power is undefined here.".into());
                }
                let n = checked(self.numerator.checked_pow(exponent))?;
                let d = checked(self.denominator.checked_pow(exponent))?;
                if rhs.numerator < 0 {
                    Self::new(d, n)
                } else {
                    Self::new(n, d)
                }
            }
            _ => Err("Unsupported operation.".into()),
        }
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denominator == 1 {
            return write!(f, "{}", self.numerator);
        }
        let mut rest = self.denominator;
        for factor in [2, 5] {
            while rest % factor == 0 {
                rest /= factor;
            }
        }
        if rest != 1 {
            return write!(f, "{}/{}", self.numerator, self.denominator);
        }
        let n = self.numerator.abs();
        let mut decimal = format!(
            "{}{}",
            if self.numerator < 0 { "-" } else { "" },
            n / self.denominator
        );
        decimal.push('.');
        let mut remainder = n % self.denominator;
        for _ in 0..24 {
            remainder *= 10;
            decimal.push(char::from(b'0' + (remainder / self.denominator) as u8));
            remainder %= self.denominator;
            if remainder == 0 {
                break;
            }
        }
        if remainder != 0 {
            write!(f, "{}/{}", self.numerator, self.denominator)
        } else {
            f.write_str(&decimal)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Calculation {
    pub expression: String,
    pub value: Number,
    pub steps: Vec<String>,
    pub equality: Option<(Number, bool)>,
}

impl Calculation {
    pub fn answer(&self) -> String {
        match self.equality {
            Some((claimed, true)) => format!("Yes. {} = {claimed}.", self.expression),
            Some((claimed, false)) => {
                format!("No. {} = {}, not {claimed}.", self.expression, self.value)
            }
            None => format!("{} = {}.", self.expression, self.value),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(Number),
    Operator(char),
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    steps: Vec<String>,
    depth: usize,
}

impl Parser {
    fn take(&mut self, operator: char) -> bool {
        if self.tokens.get(self.at) == Some(&Token::Operator(operator)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn operation(&mut self, a: Number, operator: char, b: Number) -> Result<Number, String> {
        let value = a.apply(operator, b)?;
        let description = match operator {
            '+' if a.denominator == 1
                && b.denominator == 1
                && a.numerator >= 0
                && b.numerator >= 0
                && b.numerator <= 20 =>
            {
                let counts: Vec<String> = (0..=b.numerator)
                    .map(|step| (a.numerator + step).to_string())
                    .collect();
                format!(
                    "Start with {a} {} and add {b} more. Count on: {}. The combined collection has {value} {}.",
                    if a.numerator == 1 {
                        "object"
                    } else {
                        "objects"
                    },
                    counts.join(" → "),
                    if value == Number::integer(1) {
                        "object"
                    } else {
                        "objects"
                    }
                )
            }
            '+' => format!("Combine the quantities {a} and {b}: {value}."),
            '-' => format!("Subtract {b} from {a}: {value}. Check: {value} + {b} = {a}."),
            '*' => format!(
                "Multiply {a} by {b}: {value}. Multiplication scales one quantity by the other."
            ),
            '/' => format!("Divide {a} by {b}: {value}. Check: {value} × {b} = {a}."),
            '^' => format!("Raise {a} to the power {b}: {value}."),
            _ => unreachable!(),
        };
        self.steps.push(description);
        Ok(value)
    }

    fn sum(&mut self) -> Result<Number, String> {
        self.depth += 1;
        if self.depth > 32 {
            return Err("The expression is nested too deeply.".into());
        }
        let result = self.sum_inner();
        self.depth -= 1;
        result
    }

    fn sum_inner(&mut self) -> Result<Number, String> {
        let mut value = self.product()?;
        loop {
            let operator = if self.take('+') {
                '+'
            } else if self.take('-') {
                '-'
            } else {
                break;
            };
            let rhs = self.product()?;
            value = self.operation(value, operator, rhs)?;
        }
        Ok(value)
    }

    fn product(&mut self) -> Result<Number, String> {
        let mut value = self.unary()?;
        loop {
            let operator = if self.take('*') {
                '*'
            } else if self.take('/') {
                '/'
            } else {
                break;
            };
            let rhs = self.unary()?;
            value = self.operation(value, operator, rhs)?;
        }
        Ok(value)
    }

    fn unary(&mut self) -> Result<Number, String> {
        // Token count bounds unary and exponent recursion as well as work.
        if self.take('-') {
            let value = self.unary()?;
            return Number::new(-value.numerator, value.denominator);
        }
        if self.take('+') {
            return self.unary();
        }
        let value = if self.take('(') {
            let value = self.sum()?;
            if !self.take(')') {
                return Err("A closing parenthesis is missing.".into());
            }
            value
        } else if let Some(Token::Number(value)) = self.tokens.get(self.at).cloned() {
            self.at += 1;
            value
        } else {
            return Err("Expected a number or parenthesized expression.".into());
        };
        if self.take('^') {
            let exponent = self.unary()?;
            self.operation(value, '^', exponent)
        } else {
            Ok(value)
        }
    }
}

pub fn evaluate(expression: &str) -> Result<Calculation, String> {
    if expression.len() > 2048 {
        return Err("The expression is too long.".into());
    }
    let mut tokens = Vec::new();
    let mut chars = expression.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let mut number = c.to_string();
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                number.push(chars.next().unwrap());
            }
            if number.starts_with('.') {
                number.insert(0, '0');
            }
            tokens.push(Token::Number(Number::parse(&number)?));
        } else {
            let c = match c {
                '×' | '·' => '*',
                '÷' => '/',
                '−' => '-',
                c => c,
            };
            if !matches!(c, '+' | '-' | '*' | '/' | '^' | '(' | ')') {
                return Err(format!("Unsupported arithmetic symbol: {c}."));
            }
            tokens.push(Token::Operator(c));
        }
        if tokens.len() > MAX_TOKENS {
            return Err("The expression has too many operations.".into());
        }
    }
    let mut parser = Parser {
        tokens,
        at: 0,
        steps: Vec::new(),
        depth: 0,
    };
    let value = parser.sum()?;
    if parser.at != parser.tokens.len() {
        return Err("There are unused terms in the expression.".into());
    }
    if parser.steps.is_empty() {
        parser
            .steps
            .push(format!("This numeral denotes the quantity {value}."));
    }
    Ok(Calculation {
        expression: expression.trim().into(),
        value,
        steps: parser.steps,
        equality: None,
    })
}

pub fn question(text: &str) -> Option<Result<Calculation, String>> {
    let mut text = text.trim().to_lowercase();
    for suffix in [" and why", " explain why", " please"] {
        if let Some(before) = text.strip_suffix(suffix) {
            text = before.trim().into();
        }
    }
    let text = text.trim_end_matches(['?', '.', '!']).trim();
    let text = [
        "why does ",
        "why is ",
        "what is ",
        "what's ",
        "how much is ",
        "calculate ",
        "compute ",
        "explain ",
        "why ",
        "is ",
    ]
    .iter()
    .find_map(|prefix| text.strip_prefix(prefix))
    .unwrap_or(text);
    let mut expression = text.to_string();
    for (word, symbol) in [
        ("multiplied by", "*"),
        ("divided by", "/"),
        ("to the power of", "^"),
        ("is equal to", "="),
        ("equal to", "="),
        ("equals", "="),
        ("plus", "+"),
        ("minus", "-"),
        ("times", "*"),
        ("over", "/"),
    ] {
        expression = expression.split_whitespace().collect::<Vec<_>>().join(" ");
        // Space padding prevents replacements inside words.
        expression = format!(" {expression} ")
            .replace(&format!(" {word} "), &format!(" {symbol} "))
            .trim()
            .into();
    }
    let numbers = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    expression = expression
        .split_whitespace()
        .map(|word| {
            numbers
                .iter()
                .position(|n| *n == word)
                .map_or_else(|| word.to_string(), |n| n.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ");
    if expression.is_empty() || expression.chars().any(|c| c.is_alphabetic()) {
        return None;
    }
    if !expression.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Some((lhs, rhs)) = expression.split_once('=') {
        Some(evaluate(lhs).and_then(|mut calculation| {
            let expected = evaluate(rhs)?;
            calculation.steps.extend(expected.steps);
            calculation.equality = Some((expected.value, calculation.value == expected.value));
            calculation.steps.push(format!(
                "Compare the exact quantities: {} {} {}.",
                calculation.value,
                if calculation.value == expected.value {
                    "="
                } else {
                    "≠"
                },
                expected.value
            ));
            Ok(calculation)
        }))
    } else {
        Some(evaluate(&expression))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_generalizes_and_explains_the_executed_operation() {
        for a in 0..20 {
            for b in 0..20 {
                let answer = evaluate(&format!("{a}+{b}")).unwrap();
                assert_eq!(answer.value, Number::integer(a + b));
                assert!(answer.steps[0].contains(&format!(
                    "{} {}",
                    a + b,
                    if a + b == 1 { "object" } else { "objects" }
                )));
            }
        }
        assert!(
            question("why is one plus three equal to four")
                .unwrap()
                .unwrap()
                .equality
                .unwrap()
                .1
        );
        let answer = question("why is 1 + 3 = 4?").unwrap().unwrap();
        assert_eq!(answer.equality, Some((Number::integer(4), true)));
        assert!(answer.steps[0].contains("1 → 2 → 3 → 4"));
        assert!(!question("1 + 3 = 5").unwrap().unwrap().equality.unwrap().1);
    }

    #[test]
    fn exact_decimals_fractions_precedence_and_invalid_inputs() {
        for (input, expected) in [
            ("0.1+0.2", "0.3"),
            ("1/3 + 1/6", "0.5"),
            ("-2^2", "-4"),
            ("(-2)^2", "4"),
            ("2^3^2", "512"),
            ("(2+3)*4", "20"),
            ("1/3", "1/3"),
            ("2^-3", "0.125"),
        ] {
            assert_eq!(
                evaluate(input).unwrap().value.to_string(),
                expected,
                "{input}"
            );
        }
        for input in [
            "1/0",
            "0^0",
            "2^99",
            "2 3",
            "1..2",
            "(2+3",
            "2+",
            "NaN",
            "1e999",
            "999999999999999*999999999999999",
        ] {
            assert!(evaluate(input).is_err(), "{input}");
        }
        assert!(evaluate(&format!("{}1{}", "(".repeat(40), ")".repeat(40))).is_err());
    }
}
