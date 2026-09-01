//! Native GUI (eframe/egui): two columns of buttons — set the default OS on the
//! left, arm a one-shot next boot on the right — plus reboot / shutdown and a
//! language switch. Writes go through the privileged helper (a polkit prompt).

use std::path::PathBuf;

use eframe::egui;
use os_switcher_core::{reboot, run_helper_elevated, shutdown, Entry, OsKind, Switcher};
use rust_i18n::t;

const LANGS: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("es", "Español"),
];

/// Launches the GUI. `bcd` optionally overrides the BCD hive path.
pub fn run(bcd: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let locale = detect_locale();
    rust_i18n::set_locale(locale);

    let mut app = SwitcherApp {
        bcd,
        entries: Vec::new(),
        message: String::new(),
        locale,
    };
    app.reload();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([440.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native("OS Switcher", options, Box::new(|_cc| Ok(Box::new(app))))?;
    Ok(())
}

fn detect_locale() -> &'static str {
    let sys = sys_locale::get_locale().unwrap_or_default();
    let code = sys.split(['-', '_']).next().unwrap_or("en");
    LANGS
        .iter()
        .map(|(c, _)| *c)
        .find(|c| *c == code)
        .unwrap_or("en")
}

struct SwitcherApp {
    bcd: Option<PathBuf>,
    entries: Vec<Entry>,
    message: String,
    locale: &'static str,
}

impl SwitcherApp {
    /// Reloads the entries (OS entries only; network/firmware ones are hidden).
    fn reload(&mut self) {
        let entries = match &self.bcd {
            Some(p) => Switcher::detect_with_bcd(p)
                .map(|s| s.entries())
                .unwrap_or_default(),
            None => Switcher::detect().entries(),
        };
        self.entries = entries
            .into_iter()
            .filter(|e| e.kind != OsKind::Other)
            .collect();
    }

    /// Runs a write action through the privileged helper, then reloads.
    fn act(&mut self, verb: &str, key: Option<&str>) {
        let bcd = self.bcd.as_ref().map(|p| p.to_string_lossy().into_owned());
        let mut args: Vec<&str> = Vec::new();
        if let Some(p) = &bcd {
            args.push("--bcd");
            args.push(p);
        }
        args.push(verb);
        if let Some(k) = key {
            args.push(k);
        }
        self.message = match run_helper_elevated(&args) {
            Ok(()) => String::new(),
            Err(e) => e.to_string(),
        };
        self.reload();
    }
}

impl eframe::App for SwitcherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading(t!("app_title"));

        let default = self
            .entries
            .iter()
            .find(|e| e.is_default)
            .map(|e| e.label.clone());
        let next = self
            .entries
            .iter()
            .find(|e| e.is_next)
            .map(|e| e.label.clone());
        ui.label(format!(
            "{}: {}",
            t!("current_default"),
            default.unwrap_or_else(|| "?".into())
        ));
        ui.label(format!(
            "{}: {}",
            t!("current_next"),
            next.unwrap_or_else(|| t!("next_none").into_owned())
        ));
        ui.separator();

        // Two columns: default (left) and one-shot next (right).
        let entries = self.entries.clone();
        let mut action: Option<(&'static str, Option<String>)> = None;
        ui.columns(2, |cols| {
            cols[0].strong(t!("col_default"));
            for e in &entries {
                if cols[0]
                    .selectable_label(e.is_default, os_prefix(e.kind, &e.label))
                    .clicked()
                {
                    action = Some(("default", Some(e.key.clone())));
                }
            }
            cols[1].strong(t!("col_next"));
            for e in &entries {
                if cols[1]
                    .selectable_label(e.is_next, os_prefix(e.kind, &e.label))
                    .clicked()
                {
                    action = Some(if e.is_next {
                        ("clear", None)
                    } else {
                        ("next", Some(e.key.clone()))
                    });
                }
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(t!("reboot")).clicked() {
                let _ = reboot();
            }
            if ui.button(t!("shutdown")).clicked() {
                let _ = shutdown();
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(t!("language"));
            for (code, name) in LANGS {
                if ui.selectable_label(self.locale == *code, *name).clicked() {
                    self.locale = code;
                    rust_i18n::set_locale(code);
                }
            }
        });

        ui.add_space(4.0);
        ui.small(t!("hint_privilege"));
        if !self.message.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(200, 80, 80), &self.message);
        }

        if let Some((verb, key)) = action {
            self.act(verb, key.as_deref());
        }
    }
}

/// Prefixes a label with a small OS glyph.
fn os_prefix(kind: OsKind, label: &str) -> String {
    let glyph = match kind {
        OsKind::Windows => "🪟",
        OsKind::Linux => "🐧",
        OsKind::MacOs => "",
        OsKind::Other => "•",
    };
    if glyph.is_empty() {
        label.to_string()
    } else {
        format!("{glyph} {label}")
    }
}
