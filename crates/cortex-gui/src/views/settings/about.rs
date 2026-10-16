//! This copy of the app, and the keys it answers to.

use eframe::egui::{RichText, Ui};

use super::Action;
use super::rows::{self, Filter};
use crate::theme::{self, palette};

const SHORTCUTS: [(&str, &str); 9] = [
    ("Return", "Send the message"),
    ("Shift+Return", "Start a new line"),
    ("Up, in an empty box", "Bring back your last message"),
    ("Esc", "Stop the run in progress"),
    ("⌘N", "New chat"),
    ("⌘B", "Show or hide the sidebar"),
    ("⌘E", "Show or hide the files"),
    ("⌘[  ⌘]", "Step back and forward through the chats"),
    ("Drop a picture", "Attach it to the next message"),
];

pub fn show(ui: &mut Ui, filter: &Filter) -> Option<Action> {
    rows::row(
        ui,
        filter,
        "Cortex",
        "A coding harness with a native desktop interface.",
        |ui| {
            ui.label(
                RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION")))
                    .font(theme::small())
                    .color(palette::text_secondary()),
            );
        },
    );

    rows::row(
        ui,
        filter,
        "Where settings are kept",
        "Everything on this page except API keys, which are in the system keychain.",
        |ui| {
            ui.label(
                RichText::new("~/.cortex/app.json")
                    .font(theme::mono())
                    .color(palette::text_secondary()),
            );
        },
    );

    for (keys, what) in SHORTCUTS {
        rows::row(ui, filter, what, "", |ui| {
            ui.label(
                RichText::new(keys)
                    .font(theme::mono())
                    .color(palette::text_secondary()),
            );
        });
    }

    None
}
