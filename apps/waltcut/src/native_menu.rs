//! Native macOS menu bar (muda), generated from the command registry — like Premiere, menus live in
//! the system menu bar. Items send command ids to the app's command inbox.

use std::sync::mpsc::{Receiver, channel};

use muda::accelerator::Accelerator;
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use waltcut_ui_egui::FilmcraftApp;
use waltcut_ui_egui::menus::{MENUS, MenuItem as Item, NATIVE_APP_MENU, menu_items};

fn accel(s: &str) -> Option<Accelerator> {
    // Only shortcuts with a modifier become native key equivalents (single keys stay in-app so
    // typing in fields keeps working).
    if !(s.contains("Cmd+") || s.contains("Ctrl+") || s.contains("Alt+")) {
        return None;
    }
    let mapped =
        s.replace("Cmd+", "CmdOrCtrl+").replace(";", "Semicolon").replace("'", "Quote").replace("/", "Slash").replace("=", "Equal").replace("\\", "Backslash");
    mapped.parse().ok()
}

/// Updates native item labels, key equivalents and enablement when the menu tree changes.
pub type MenuUpdater = Box<dyn FnMut(&[Item])>;

pub fn install(app: &FilmcraftApp, ctx: egui::Context) -> (Receiver<String>, MenuUpdater) {
    let items = menu_items(app);
    // each native item with the menu entry it currently shows, so updates touch only what changed
    let mut native: Vec<(Item, MenuItem)> = Vec::new();
    let mut titles: Vec<(String, Submenu)> = Vec::new();
    let bar = Menu::new();
    let app_menu = Submenu::new("FilmCraft", true);
    let [settings_label, hide_label, hide_others_label, quit_label] = NATIVE_APP_MENU;
    // FilmCraft ▸ Settings ▸ <category> (Premiere's app-menu layout; General is Cmd+,)
    let settings = Submenu::new(waltcut_ui_egui::i18n::t(settings_label), true);
    titles.push((settings_label.into(), settings.clone()));
    for it in items.iter().filter(|i| i.id.starts_with("app.settings.")) {
        let label = it.label.trim_end_matches('…').to_string();
        let mi = MenuItem::with_id(it.id.clone(), label, it.enabled, it.shortcut.as_deref().and_then(accel));
        native.push((it.clone(), mi.clone()));
        let _ = settings.append(&mi);
    }
    // About repeats a Help menu command: same label, updated with it
    let label = |id: &str, english: &str| items.iter().find(|i| i.id == id).map_or_else(|| english.to_string(), |i| i.label.clone());
    let about = MenuItem::with_id("app.about", label("app.about", "About FilmCraft"), true, None);
    if let Some(it) = items.iter().find(|i| i.id == "app.about") {
        // recorded as built (enabled, no shortcut), so the first update applies the real state
        native.push((Item { enabled: true, shortcut: None, ..it.clone() }, about.clone()));
    }
    // the predefined items keep their native actions; our text lets them follow the language
    let predefined = [
        (hide_label, PredefinedMenuItem::hide(Some(waltcut_ui_egui::i18n::t(hide_label)))),
        (hide_others_label, PredefinedMenuItem::hide_others(Some(waltcut_ui_egui::i18n::t(hide_others_label)))),
        (quit_label, PredefinedMenuItem::quit(Some(waltcut_ui_egui::i18n::t(quit_label)))),
    ];
    let [(_, hide), (_, hide_others), (_, quit)] = &predefined;
    let _ = app_menu.append_items(&[
        &about,
        &PredefinedMenuItem::separator(),
        &settings,
        &PredefinedMenuItem::separator(),
        hide,
        hide_others,
        &PredefinedMenuItem::separator(),
        quit,
    ]);
    let _ = bar.append(&app_menu);
    for top in MENUS {
        let sub = Submenu::new(waltcut_ui_egui::i18n::t(top), true);
        titles.push((top.into(), sub.clone()));
        let mine: Vec<&Item> = items.iter().filter(|i| i.path.first().map(String::as_str) == Some(top) && !i.id.starts_with("app.settings.")).collect();
        // submenus by path prefix (e.g. Clip ▸ Video Options ▸ Time Interpolation), created where
        // their first item appears
        let mut subs: Vec<(Vec<String>, Submenu)> = Vec::new();
        for it in mine {
            let mi = MenuItem::with_id(it.id.clone(), &it.label, it.enabled, it.shortcut.as_deref().and_then(accel));
            native.push(((*it).clone(), mi.clone()));
            let mut parent = sub.clone();
            for depth in 1..it.path.len() {
                let key = it.path[..=depth].to_vec();
                parent = match subs.iter().find(|(k, _)| *k == key) {
                    Some((_, s)) => s.clone(),
                    None => {
                        let s = Submenu::new(waltcut_ui_egui::i18n::t(&it.path[depth]), true);
                        titles.push((it.path[depth].clone(), s.clone()));
                        let _ = parent.append(&s);
                        subs.push((key, s.clone()));
                        s
                    }
                };
            }
            let _ = parent.append(&mi);
        }
        let _ = bar.append(&sub);
    }
    bar.init_for_nsapp();
    if let Some(help) = bar.items().last().and_then(|i| i.as_submenu().cloned()) {
        help.set_as_help_menu_for_nsapp();
    }
    Box::leak(Box::new(bar));
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        while let Ok(ev) = MenuEvent::receiver().recv() {
            if tx.send(ev.id().0.clone()).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    let update: MenuUpdater = Box::new(move |items: &[Item]| {
        for (shown, mi) in &mut native {
            let Some(it) = items.iter().find(|i| i.id == shown.id) else { continue };
            if it.label != shown.label {
                // Settings entries drop the trailing ellipsis, as when the menu was built
                mi.set_text(if it.id.starts_with("app.settings.") { it.label.trim_end_matches('…') } else { &it.label });
            }
            if it.shortcut != shown.shortcut {
                let _ = mi.set_accelerator(it.shortcut.as_deref().and_then(accel));
            }
            if it.enabled != shown.enabled {
                mi.set_enabled(it.enabled);
            }
            *shown = it.clone();
        }
    });
    (rx, update)
}
