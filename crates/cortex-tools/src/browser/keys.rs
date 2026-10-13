//! Turning a key's name, such as `Enter` or `Control+a`, into the events
//! the browser expects for it.

use cortex_core::ToolError;
use serde_json::{Value, json};

const ALT: u8 = 1;
const CONTROL: u8 = 2;
const META: u8 = 4;
const SHIFT: u8 = 8;

/// Name, the code of the physical key, the number older pages read, and
/// the text it types.
const NAMED: [(&str, &str, u32, &str); 18] = [
    ("Enter", "Enter", 13, "\r"),
    ("Tab", "Tab", 9, ""),
    ("Escape", "Escape", 27, ""),
    ("Backspace", "Backspace", 8, ""),
    ("Delete", "Delete", 46, ""),
    ("ArrowUp", "ArrowUp", 38, ""),
    ("ArrowDown", "ArrowDown", 40, ""),
    ("ArrowLeft", "ArrowLeft", 37, ""),
    ("ArrowRight", "ArrowRight", 39, ""),
    ("Home", "Home", 36, ""),
    ("End", "End", 35, ""),
    ("PageUp", "PageUp", 33, ""),
    ("PageDown", "PageDown", 34, ""),
    ("Insert", "Insert", 45, ""),
    (" ", "Space", 32, " "),
    ("Shift", "ShiftLeft", 16, ""),
    ("Control", "ControlLeft", 17, ""),
    ("Alt", "AltLeft", 18, ""),
];

/// One key press, with the keys held down during it.
#[derive(Debug, PartialEq, Eq)]
pub struct Press {
    modifiers: u8,
    key: String,
    code: String,
    key_code: u32,
    text: String,
}

impl Press {
    /// Reads `Enter`, `a`, `Shift+Tab`, `Control+Shift+K` and the like.
    pub fn parse(spelled: &str) -> Result<Self, ToolError> {
        let unknown = || {
            ToolError::InvalidInput(format!(
                "`{spelled}` is not a key. Use a single character or a name such as Enter, \
                 Tab, Escape, Backspace, Delete, ArrowDown, PageDown, Home, End, Space or F5, \
                 with Control+, Shift+, Alt+ or Meta+ in front to hold those down"
            ))
        };

        // `+` separates the parts, and is also a key: `Control++`.
        let (held, name) = match spelled.strip_suffix("++") {
            Some(held) => (held, "+"),
            None => match spelled.rsplit_once('+') {
                Some((held, name)) if !name.is_empty() => (held, name),
                _ => ("", spelled),
            },
        };

        let mut modifiers = 0;

        for part in held.split('+').filter(|part| !part.is_empty()) {
            modifiers |= match part.to_ascii_lowercase().as_str() {
                "alt" | "option" => ALT,
                "control" | "ctrl" => CONTROL,
                "meta" | "cmd" | "command" => META,
                "shift" => SHIFT,
                _ => return Err(unknown()),
            };
        }

        let alias = match name.to_ascii_lowercase().as_str() {
            "esc" => "Escape",
            "return" => "Enter",
            "space" | "spacebar" => " ",
            "del" => "Delete",
            "up" => "ArrowUp",
            "down" => "ArrowDown",
            "left" => "ArrowLeft",
            "right" => "ArrowRight",
            _ => name,
        };

        // With a command key held, a key does its command, not its text.
        let commanding = modifiers & (CONTROL | META | ALT) != 0;

        if let Some((key, code, key_code, text)) = NAMED
            .iter()
            .find(|(key, ..)| key.eq_ignore_ascii_case(alias))
        {
            return Ok(Self {
                modifiers,
                key: key.to_string(),
                code: code.to_string(),
                key_code: *key_code,
                text: if commanding { "" } else { text }.to_string(),
            });
        }

        if let Some(number) = alias
            .strip_prefix(['F', 'f'])
            .and_then(|number| number.parse::<u32>().ok())
            .filter(|number| (1..=12).contains(number))
        {
            return Ok(Self {
                modifiers,
                key: format!("F{number}"),
                code: format!("F{number}"),
                key_code: 111 + number,
                text: String::new(),
            });
        }

        let mut characters = alias.chars();

        let (Some(character), None) = (characters.next(), characters.next()) else {
            return Err(unknown());
        };

        let character = if modifiers & SHIFT != 0 {
            character.to_ascii_uppercase()
        } else {
            character
        };

        let (code, key_code) = match character {
            'a'..='z' | 'A'..='Z' => (
                format!("Key{}", character.to_ascii_uppercase()),
                character.to_ascii_uppercase() as u32,
            ),
            '0'..='9' => (format!("Digit{character}"), character as u32),
            _ => (String::new(), 0),
        };

        Ok(Self {
            modifiers,
            key: character.to_string(),
            code,
            key_code,
            text: if commanding {
                String::new()
            } else {
                character.to_string()
            },
        })
    }

    /// The key going down and coming up, as two `Input.dispatchKeyEvent`
    /// commands.
    pub fn events(&self) -> [Value; 2] {
        let event = |kind: &str| {
            let mut event = json!({
                "type": kind,
                "modifiers": self.modifiers,
                "key": self.key,
                "code": self.code,
                "windowsVirtualKeyCode": self.key_code,
                "nativeVirtualKeyCode": self.key_code,
            });

            if kind == "keyDown" {
                event["text"] = json!(self.text);
            }

            event
        };

        // A key that types nothing goes down "raw", or the page would be
        // sent an empty character.
        let down = if self.text.is_empty() {
            "rawKeyDown"
        } else {
            "keyDown"
        };

        [event(down), event("keyUp")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_key_carries_its_code_and_its_text() {
        let enter = Press::parse("enter").unwrap();

        assert_eq!(enter.key, "Enter");
        assert_eq!(enter.key_code, 13);

        let [down, up] = enter.events();

        assert_eq!(down["type"], "keyDown");
        assert_eq!(down["text"], "\r");
        assert_eq!(up["type"], "keyUp");

        assert_eq!(
            Press::parse("Esc").unwrap().events()[0]["type"],
            "rawKeyDown"
        );
        assert_eq!(Press::parse("F5").unwrap().key_code, 116);
        assert_eq!(Press::parse("Space").unwrap().text, " ");
    }

    #[test]
    fn held_keys_are_added_up_and_stop_a_character_from_typing() {
        let shortcut = Press::parse("Control+Shift+k").unwrap();

        assert_eq!(shortcut.modifiers, CONTROL | SHIFT);
        assert_eq!(shortcut.key, "K");
        assert_eq!(shortcut.code, "KeyK");
        assert_eq!(shortcut.text, "");

        let back = Press::parse("Shift+Tab").unwrap();

        assert_eq!((back.modifiers, back.key.as_str()), (SHIFT, "Tab"));

        assert_eq!(Press::parse("Cmd+a").unwrap().modifiers, META);
    }

    #[test]
    fn a_plain_character_types_itself_and_plus_is_a_key_too() {
        assert_eq!(Press::parse("a").unwrap().text, "a");
        assert_eq!(Press::parse("+").unwrap().text, "+");
        assert_eq!(Press::parse("Control++").unwrap().key, "+");
        assert_eq!(Press::parse("/").unwrap().key_code, 0);
    }

    #[test]
    fn anything_else_is_refused_with_the_names_that_work() {
        assert!(Press::parse("Hyper+a").is_err());
        assert!(Press::parse("Entr").is_err());
        assert!(Press::parse("").is_err());
    }
}
