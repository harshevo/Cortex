//! The macOS menu bar. Its Edit menu holds Cut, Copy, Paste and Select
//! All, so that their shortcuts reach the app as commands. The window
//! library would otherwise read Cmd+V as a paste of text only, and drop it
//! when the clipboard holds a picture or files.

use eframe::egui;

/// A command chosen from the menu, or by its shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The menu bar, kept alive while the app runs. Elsewhere than macOS it
/// is empty and gives no commands: the window library handles the keys.
#[derive(Default)]
pub struct MenuBar {
    #[cfg(target_os = "macos")]
    inner: Option<macos::Inner>,
}

impl MenuBar {
    /// Puts the menu bar up. Called once the window is open, so that it
    /// replaces the window library's default menu.
    pub fn install(context: &egui::Context) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                inner: macos::Inner::install(context),
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = context;
            Self::default()
        }
    }

    /// The commands chosen since the last call.
    pub fn take(&self) -> Vec<Command> {
        #[cfg(target_os = "macos")]
        if let Some(inner) = &self.inner {
            return inner.take();
        }

        Vec::new()
    }
}

/// The events that stand for `command` in a text field.
pub fn events(command: Command) -> Vec<egui::Event> {
    let key = |pressed| egui::Event::Key {
        key: egui::Key::A,
        physical_key: Some(egui::Key::A),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };

    match command {
        Command::Cut => vec![egui::Event::Cut],
        Command::Copy => vec![egui::Event::Copy],
        Command::SelectAll => vec![key(true), key(false)],

        // Read from the clipboard by whoever takes it.
        Command::Paste => Vec::new(),
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Command;
    use eframe::egui;
    use muda::accelerator::{Accelerator, Code, Modifiers};
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use std::sync::{Arc, Mutex};

    const COMMANDS: [(&str, &str, Code, Command); 4] = [
        ("cortex.cut", "Cut", Code::KeyX, Command::Cut),
        ("cortex.copy", "Copy", Code::KeyC, Command::Copy),
        ("cortex.paste", "Paste", Code::KeyV, Command::Paste),
        (
            "cortex.select_all",
            "Select All",
            Code::KeyA,
            Command::SelectAll,
        ),
    ];

    pub struct Inner {
        _menu: Menu,
        chosen: Arc<Mutex<Vec<Command>>>,
    }

    impl Inner {
        pub fn install(context: &egui::Context) -> Option<Self> {
            let items: Vec<MenuItem> = COMMANDS
                .iter()
                .map(|(id, label, code, _)| {
                    MenuItem::with_id(
                        *id,
                        label,
                        true,
                        Some(Accelerator::new(Modifiers::META, *code)),
                    )
                })
                .collect();

            let separator = PredefinedMenuItem::separator();

            // The same application menu the window library puts up.
            let application = Submenu::with_items(
                "Cortex",
                true,
                &[
                    &PredefinedMenuItem::about(None, None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::services(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::quit(None),
                ],
            )
            .ok()?;

            let edit = Submenu::with_items(
                "Edit",
                true,
                &[&items[0], &items[1], &items[2], &separator, &items[3]],
            )
            .ok()?;

            let menu = Menu::with_items(&[&application, &edit]).ok()?;
            menu.init_for_nsapp();

            let chosen = Arc::new(Mutex::new(Vec::new()));
            let sink = chosen.clone();
            let context = context.clone();

            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let command = COMMANDS
                    .iter()
                    .find(|(id, ..)| event.id() == id)
                    .map(|(.., command)| *command);

                if let Some(command) = command {
                    if let Ok(mut chosen) = sink.lock() {
                        chosen.push(command);
                    }
                    context.request_repaint();
                }
            }));

            Some(Self {
                _menu: menu,
                chosen,
            })
        }

        pub fn take(&self) -> Vec<Command> {
            self.chosen
                .lock()
                .map(|mut chosen| std::mem::take(&mut *chosen))
                .unwrap_or_default()
        }
    }
}
