//! The Plugins menu, drawn headlessly over a real plugin command registry.
//!
//! `egui::Context::run_ui` lays a frame out on the CPU with no window and no
//! GPU, so the menu can be painted on a CI runner. What matters is that the
//! host really does turn what a plugin registered into menu entries and
//! keyboard bindings, and that painting the menu chooses nothing on its own.

use eframe::egui::{self, Key, Modifiers};
use sub_plugin::menu::{CommandDesc, PluginCommandRegistry};
use sub_ui::plugins::{EMPTY_LABEL, MENU_TITLE, PluginMenu, plugins_menu_ui};
use sub_ui::shortcuts::ShortcutMap;

/// A description as a `commands`-world plugin declares it.
fn desc(id: &str, title: &str, shortcut: Option<&str>) -> CommandDesc {
    CommandDesc {
        id: id.to_owned(),
        title: title.to_owned(),
        shortcut: shortcut.map(str::to_owned),
    }
}

/// Two plugins, three commands, one of them asking for a chord the editor
/// already owns.
fn registry() -> PluginCommandRegistry {
    let mut registry = PluginCommandRegistry::new();
    registry
        .register(
            "com.example.silence-cutter",
            &[
                desc("cut-silence", "Cut silence", Some("Ctrl+Shift+K")),
                desc("report", "Silence report", None),
            ],
        )
        .expect("valid descriptions");
    registry
        .register(
            "com.example.montage",
            &[desc("build", "Build montage", Some("Ctrl+Z"))],
        )
        .expect("valid descriptions");
    registry
}

/// Runs one headless frame drawing `body`.
fn frame<R>(ctx: &egui::Context, mut body: impl FnMut(&mut egui::Ui) -> R) -> R {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        ..Default::default()
    };
    let mut result = None;
    let mut output = ctx.run_ui(input, |ui| {
        result = Some(body(ui));
    });
    let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    output.textures_delta.clear();
    result.expect("the frame body ran")
}

#[test]
fn every_registered_command_reaches_the_menu_and_the_free_chords_reach_the_keyboard() {
    let menu = PluginMenu::register(&registry(), &ShortcutMap::default_map());

    let titles: Vec<&str> = menu
        .entries()
        .iter()
        .map(|entry| entry.title.as_str())
        .collect();
    assert_eq!(titles, ["Cut silence", "Silence report", "Build montage"]);

    // The free chord is bound; the one that collides with undo is not, and the
    // command keeps its menu entry.
    assert_eq!(
        menu.command_for(Key::K, Modifiers::COMMAND | Modifiers::SHIFT),
        Some("com.example.silence-cutter/cut-silence")
    );
    assert_eq!(menu.chord_for("com.example.montage/build"), None);
    assert_eq!(menu.problems().len(), 1);
    assert_eq!(
        menu.problems()[0].code.as_str(),
        "ui.plugin_shortcut_conflict"
    );

    // Ctrl+Z is still the editor's undo: the plugin did not take it.
    let map = ShortcutMap::default_map();
    assert!(map.action_for(Key::Z, Modifiers::COMMAND).is_some());
    assert_eq!(menu.command_for(Key::Z, Modifiers::COMMAND), None);
}

#[test]
fn the_menu_paints_and_invents_nothing_on_its_own() {
    let menu = PluginMenu::register(&registry(), &ShortcutMap::default_map());
    let ctx = egui::Context::default();

    // Two frames: egui needs a second pass before its widgets have their real
    // sizes, and neither pass may run a command nobody clicked.
    for _ in 0..2 {
        let chosen = frame(&ctx, |ui| plugins_menu_ui(ui, &menu));
        assert_eq!(chosen, None);
    }
}

#[test]
fn an_editor_with_no_plugins_still_has_a_menu() {
    let menu = PluginMenu::empty();
    let ctx = egui::Context::default();
    assert!(menu.is_empty());
    for _ in 0..2 {
        let chosen = frame(&ctx, |ui| plugins_menu_ui(ui, &menu));
        assert_eq!(chosen, None);
    }
    assert_eq!(MENU_TITLE, "Plugins");
    assert_eq!(EMPTY_LABEL, "No plugin commands");
}

#[test]
fn a_removed_plugin_takes_its_menu_entries_and_its_chord_with_it() {
    let mut registry = registry();
    assert_eq!(registry.remove_plugin("com.example.silence-cutter"), 2);
    let menu = PluginMenu::register(&registry, &ShortcutMap::default_map());

    assert_eq!(menu.entries().len(), 1);
    assert_eq!(
        menu.command_for(Key::K, Modifiers::COMMAND | Modifiers::SHIFT),
        None,
        "the uninstalled plugin's chord is free again"
    );
}
