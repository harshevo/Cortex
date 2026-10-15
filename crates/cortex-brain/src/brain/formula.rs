//! Formulae are learned explicitly; execution checks quantities and SI dimensions.

use super::arithmetic::{Number, evaluate};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Formula {
    pub target: String,
    pub expression: String,
    pub variables: Vec<String>,
    pub memory: u64,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantity {
    pub value: Number,
    pub dimension: [i8; 3], // mass, length, time
}

impl Quantity {
    fn apply(self, operator: char, rhs: Self) -> Result<Self, String> {
        let mut dimension = self.dimension;
        match operator {
            '+' | '-' if self.dimension != rhs.dimension => {
                return Err("Cannot add or subtract incompatible physical dimensions.".into());
            }
            '+' | '-' => {}
            '*' | '/' => {
                for (at, d) in dimension.iter_mut().enumerate() {
                    *d = if operator == '*' {
                        d.checked_add(rhs.dimension[at])
                    } else {
                        d.checked_sub(rhs.dimension[at])
                    }
                    .ok_or("Dimension exponent overflow.")?;
                }
            }
            '^' => {
                if rhs.dimension != [0; 3]
                    || rhs.value.denominator != 1
                    || rhs.value.numerator.abs() > 12
                {
                    return Err("A power requires a bounded dimensionless integer exponent.".into());
                }
                for d in &mut dimension {
                    *d = d
                        .checked_mul(rhs.value.numerator as i8)
                        .ok_or("Dimension exponent overflow.")?;
                }
            }
            _ => return Err("Unsupported quantity operation.".into()),
        }
        Ok(Self {
            value: self.value.apply(operator, rhs.value)?,
            dimension,
        })
    }

    pub fn describe(self) -> String {
        let unit = match self.dimension {
            [0, 0, 0] => String::new(),
            [1, 0, 0] => "kg".into(),
            [0, 1, 0] => "m".into(),
            [0, 0, 1] => "s".into(),
            [0, 1, -1] => "m/s".into(),
            [0, 1, -2] => "m/s^2".into(),
            [1, 1, -2] => "N".into(),
            [1, 2, -2] => "J".into(),
            [1, 2, -3] => "W".into(),
            [1, 1, -1] => "kg*m/s".into(),
            dimensions => dimensions
                .iter()
                .zip(["kg", "m", "s"])
                .filter(|(power, _)| **power != 0)
                .map(|(power, name)| format!("{name}^{power}"))
                .collect::<Vec<_>>()
                .join("*"),
        };
        format!(
            "{}{}{}",
            self.value,
            if unit.is_empty() { "" } else { " " },
            unit
        )
    }
}

fn known_dimension(name: &str) -> Option<[i8; 3]> {
    Some(match name {
        "mass" => [1, 0, 0],
        "distance" | "length" | "height" | "displacement" => [0, 1, 0],
        "time" | "duration" => [0, 0, 1],
        "speed" | "velocity" => [0, 1, -1],
        "acceleration" => [0, 1, -2],
        "force" | "weight" => [1, 1, -2],
        "energy" | "work" | "kinetic_energy" | "potential_energy" => [1, 2, -2],
        "power" => [1, 2, -3],
        "momentum" => [1, 1, -1],
        _ => return None,
    })
}

pub(crate) fn unit(text: &str) -> Option<(Number, [i8; 3])> {
    Some(match text.to_lowercase().replace('²', "^2").as_str() {
        "kg" | "kilogram" | "kilograms" => (Number::integer(1), [1, 0, 0]),
        "g" | "gram" | "grams" => (Number::new(1, 1000).ok()?, [1, 0, 0]),
        "m" | "meter" | "meters" | "metre" | "metres" => (Number::integer(1), [0, 1, 0]),
        "cm" => (Number::new(1, 100).ok()?, [0, 1, 0]),
        "km" => (Number::integer(1000), [0, 1, 0]),
        "s" | "second" | "seconds" => (Number::integer(1), [0, 0, 1]),
        "ms" => (Number::new(1, 1000).ok()?, [0, 0, 1]),
        "min" | "minutes" => (Number::integer(60), [0, 0, 1]),
        "m/s" => (Number::integer(1), [0, 1, -1]),
        "km/h" => (Number::new(5, 18).ok()?, [0, 1, -1]),
        "m/s^2" => (Number::integer(1), [0, 1, -2]),
        "n" | "newton" | "newtons" => (Number::integer(1), [1, 1, -2]),
        "j" | "joule" | "joules" => (Number::integer(1), [1, 2, -2]),
        "w" | "watt" | "watts" => (Number::integer(1), [1, 2, -3]),
        _ => return None,
    })
}

#[derive(Clone)]
enum Expr {
    Constant(Number),
    Variable(String),
    Binary(char, Box<Expr>, Box<Expr>),
    Negate(Box<Expr>),
}

impl Expr {
    fn contains(&self, name: &str) -> bool {
        match self {
            Self::Variable(v) => v == name,
            Self::Binary(_, a, b) => a.contains(name) || b.contains(name),
            Self::Negate(a) => a.contains(name),
            _ => false,
        }
    }

    fn variables(&self, names: &mut BTreeSet<String>) {
        match self {
            Self::Variable(v) => {
                names.insert(v.clone());
            }
            Self::Binary(_, a, b) => {
                a.variables(names);
                b.variables(names);
            }
            Self::Negate(a) => a.variables(names),
            _ => {}
        }
    }

    fn run(
        &self,
        values: &BTreeMap<String, Quantity>,
        steps: &mut Vec<String>,
    ) -> Result<Quantity, String> {
        match self {
            Self::Constant(value) => Ok(Quantity {
                value: *value,
                dimension: [0; 3],
            }),
            Self::Variable(name) => values
                .get(name)
                .copied()
                .ok_or_else(|| format!("I need a value for {}.", name.replace('_', " "))),
            Self::Negate(expr) => {
                let mut value = expr.run(values, steps)?;
                value.value.numerator = -value.value.numerator;
                Ok(value)
            }
            Self::Binary(op, a, b) => {
                let a = a.run(values, steps)?;
                let b = b.run(values, steps)?;
                let value = a.apply(*op, b)?;
                steps.push(format!(
                    "{} {op} {} = {}.",
                    a.describe(),
                    b.describe(),
                    value.describe()
                ));
                Ok(value)
            }
        }
    }

    fn solve(
        &self,
        name: &str,
        result: Quantity,
        values: &BTreeMap<String, Quantity>,
        steps: &mut Vec<String>,
    ) -> Result<Quantity, String> {
        match self {
            Self::Variable(variable) if variable == name => Ok(result),
            Self::Negate(expr) => {
                let mut negative = result;
                negative.value.numerator = -negative.value.numerator;
                expr.solve(name, negative, values, steps)
            }
            Self::Binary(op, a, b) if a.contains(name) != b.contains(name) => {
                let left = a.contains(name);
                let known = if left { b } else { a }.run(values, steps)?;
                let next = match (*op, left) {
                    ('+', _) => result.apply('-', known)?,
                    ('-', true) => result.apply('+', known)?,
                    ('-', false) => known.apply('-', result)?,
                    ('*', _) => result.apply('/', known)?,
                    ('/', true) => result.apply('*', known)?,
                    ('/', false) => known.apply('/', result)?,
                    _ => return Err("I cannot invert that formula with this solver.".into()),
                };
                steps.push(format!(
                    "Undo {op} to isolate {}: {}.",
                    name.replace('_', " "),
                    next.describe()
                ));
                if *op == '/' && !left && next.value.numerator == 0 {
                    return Err("The rearranged equation would divide by zero.".into());
                }
                if left { a } else { b }.solve(name, next, values, steps)
            }
            _ => Err("The unknown must occur once in an invertible formula.".into()),
        }
    }
}

struct Expression {
    tokens: Vec<String>,
    at: usize,
}
impl Expression {
    fn parse(text: &str) -> Option<Expr> {
        if text.len() > 512 {
            return None;
        }
        let mut tokens = Vec::new();
        let mut current = String::new();
        for c in text.chars() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                current.push(c);
            } else {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                if !c.is_whitespace() {
                    tokens.push(c.to_string());
                }
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        if tokens.len() > 64 {
            return None;
        }
        let mut parser = Self { tokens, at: 0 };
        let expr = parser.expression(0)?;
        (parser.at == parser.tokens.len()).then_some(expr)
    }

    fn expression(&mut self, min: u8) -> Option<Expr> {
        let token = self.tokens.get(self.at)?.clone();
        self.at += 1;
        let mut left = match token.as_str() {
            "-" => Expr::Negate(Box::new(self.expression(5)?)),
            "(" => {
                let expr = self.expression(0)?;
                if self.tokens.get(self.at)?.as_str() != ")" {
                    return None;
                }
                self.at += 1;
                expr
            }
            _ if token.chars().next()?.is_ascii_digit() => {
                Expr::Constant(Number::parse(&token).ok()?)
            }
            _ if token.chars().next()?.is_ascii_alphabetic()
                && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                Expr::Variable(token)
            }
            _ => return None,
        };
        while let Some(token) = self.tokens.get(self.at) {
            let (l, r, op) = match token.as_str() {
                "+" => (1, 2, '+'),
                "-" => (1, 2, '-'),
                "*" => (3, 4, '*'),
                "/" => (3, 4, '/'),
                "^" => (6, 5, '^'),
                _ => break,
            };
            if l < min {
                break;
            }
            self.at += 1;
            left = Expr::Binary(op, Box::new(left), Box::new(self.expression(r)?));
        }
        Some(left)
    }
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .replace("kinetic energy", "kinetic_energy")
        .replace("potential energy", "potential_energy")
        .replace('²', "^2")
        .replace('×', "*")
        .replace('÷', "/")
        .replace('−', "-")
}

impl Formula {
    pub fn learn(text: &str, source: &str, memory: u64) -> Option<Self> {
        let mut text = normalize(text);
        for (phrase, operator) in [
            (" multiplied by ", " * "),
            (" divided by ", " / "),
            (" times ", " * "),
            (" plus ", " + "),
            (" minus ", " - "),
            (" equals ", " = "),
            (" is ", " = "),
        ] {
            text = text.replace(phrase, operator);
        }
        let (target, expression) = text.split_once('=')?;
        let target = target.trim();
        let expression = expression.trim();
        if target.is_empty()
            || !target.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
            || !expression.contains(['+', '-', '*', '/', '^'])
        {
            return None;
        }
        let expr = Expression::parse(expression)?;
        let mut variables = BTreeSet::new();
        expr.variables(&mut variables);
        if variables.is_empty() || variables.len() > 12 || variables.contains(target) {
            return None;
        }
        Some(Self {
            target: target.into(),
            expression: expression.into(),
            variables: variables.into_iter().collect(),
            memory,
            source: source.into(),
        })
    }

    pub fn answer(&self, text: &str) -> Option<Result<FormulaAnswer, String>> {
        let text = normalize(text);
        let goal_text = text.trim().trim_start_matches("please ");
        let goal_text = ["what is ", "calculate ", "find ", "compute ", "why is "]
            .iter()
            .find_map(|p| goal_text.strip_prefix(p))?;
        let names: Vec<&str> = std::iter::once(self.target.as_str())
            .chain(self.variables.iter().map(String::as_str))
            .collect();
        let goal = names
            .iter()
            .find(|name| {
                goal_text.strip_prefix(**name).is_some_and(|tail| {
                    tail.is_empty() || tail.starts_with(|c: char| !c.is_alphanumeric() && c != '_')
                })
            })?
            .to_string();
        let expression = Expression::parse(&self.expression)?;
        let mut values = BTreeMap::new();
        let mut steps = vec![format!(
            "Learned rule from memory {} ({}): {} = {}.",
            self.memory, self.source, self.target, self.expression
        )];
        for name in names {
            let pattern = format!(
                r"\b{}\s*(?:is\s*|=\s*|of\s*)?(-?\d+(?:\.\d+)?)(?:\s*([a-zA-Z][a-zA-Z0-9/*^²]*))?",
                regex::escape(name)
            );
            let regex = Regex::new(&pattern).ok()?;
            let mut supplied = regex.captures_iter(&text);
            let Some(captures) = supplied.next() else {
                continue;
            };
            if supplied.next().is_some() {
                return Some(Err(format!(
                    "More than one value was supplied for {name}. Please give one unambiguous value."
                )));
            }
            let value = match evaluate(&captures[1]) {
                Ok(c) => c.value,
                Err(e) => return Some(Err(e)),
            };
            let unit_text = captures
                .get(2)
                .map(|m| m.as_str())
                .filter(|s| !matches!(*s, "and" | "what" | "if" | "then"));
            let (scale, dimension) = match unit_text {
                Some(unit_text) => match unit(unit_text) {
                    Some(unit) => unit,
                    None => return Some(Err(format!("I do not support the unit {unit_text}."))),
                },
                None => {
                    steps.push(format!(
                        "No unit supplied for {name}; assuming its SI unit."
                    ));
                    (Number::integer(1), known_dimension(name).unwrap_or([0; 3]))
                }
            };
            if known_dimension(name).is_some_and(|expected| expected != dimension) {
                return Some(Err(format!(
                    "The unit supplied for {name} has incompatible dimensions."
                )));
            }
            let value = match value.apply('*', scale) {
                Ok(value) => value,
                Err(e) => return Some(Err(e)),
            };
            values.insert(name.to_string(), Quantity { value, dimension });
        }
        // A bare conceptual question should use facts, not demand numbers.
        if values.is_empty() {
            return None;
        }
        let result = if goal == self.target {
            expression.run(&values, &mut steps)
        } else {
            values
                .get(&self.target)
                .copied()
                .ok_or_else(|| format!("I need a value for {}.", self.target))
                .and_then(|result| expression.solve(&goal, result, &values, &mut steps))
        };
        Some(result.and_then(|result| {
            if known_dimension(&goal).is_some_and(|expected| expected != result.dimension) {
                return Err("The learned formula produces incompatible dimensions for the requested quantity.".into());
            }
            // Substitute a solved unknown back into the original equation.
            if goal != self.target {
                values.insert(goal.clone(),result);
                let checked = expression.run(&values,&mut Vec::new())?;
                if Some(&checked) != values.get(&self.target) { return Err("The solution failed substitution into the learned formula.".into()); }
                steps.push("Substitution into the original equation checks out.".into());
            }
            Ok(FormulaAnswer { text: format!("{} = {}.",goal.replace('_', " "),result.describe()), steps, memory:self.memory })
        }))
    }
}

pub struct FormulaAnswer {
    pub text: String,
    pub steps: Vec<String>,
    pub memory: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn learned_rules_transfer_to_new_values_and_solve_backwards() {
        let rule = Formula::learn("force is mass times acceleration", "test", 1).unwrap();
        for mass in 1..10 {
            let answer = rule
                .answer(&format!(
                    "what is force if mass is {mass} kg and acceleration is 3 m/s^2?"
                ))
                .unwrap()
                .unwrap();
            assert_eq!(answer.text, format!("force = {} N.", mass * 3));
        }
        assert_eq!(
            rule.answer("what is mass if force is 6 N and acceleration is 3 m/s^2?")
                .unwrap()
                .unwrap()
                .text,
            "mass = 2 kg."
        );
        assert_eq!(
            rule.answer("what is force if mass is 500 g and acceleration is 4 m/s^2?")
                .unwrap()
                .unwrap()
                .text,
            "force = 2 N."
        );
        assert!(
            rule.answer("what is force if mass is 2 s and acceleration is 3 m/s^2")
                .unwrap()
                .is_err()
        );
        assert!(
            rule.answer("what is force if mass is 2 kg")
                .unwrap()
                .is_err()
        );
        assert!(
            rule.answer("what is mass if force is 6 N and acceleration is 0 m/s^2")
                .unwrap()
                .is_err()
        );
        assert!(
            rule.answer("what is force if mass is 2 frogs and acceleration is 3 m/s^2")
                .unwrap()
                .is_err()
        );
    }
}
