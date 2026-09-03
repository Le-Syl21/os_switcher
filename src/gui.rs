//! The graphical face of os-switcher (eframe/egui).
//!
//! One card per bootable system. Arming a one-shot next boot is a single click,
//! and the restart button then names where the machine will land — the common
//! errand ("boot into the other OS once") is two clicks and no jargon. Making a
//! choice permanent is one click further, deliberately quieter.
//!
//! Writes never happen on the UI thread: they run on a worker, because on Linux
//! they go through `pkexec` and the authorization dialog can take as long as
//! the user does.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use crate::switcher::{is_elevated, reboot, run_self_elevated, shutdown, Entry, OsKind, Switcher};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Margin, RichText, Stroke, Vec2};
use rust_i18n::t;

/// The languages offered in the footer.
const LANGS: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("es", "Español"),
];

/// Entry point of the `os-switcher-gui` binary.
///
/// When it is handed a subcommand — chiefly when a worker re-runs it elevated
/// to apply a change — it acts as the CLI and exits. Otherwise it opens the
/// window, first securing the privileges the UI needs to even read the boot
/// configuration on Windows.
pub fn run() -> std::process::ExitCode {
    use std::process::ExitCode;

    // Window-subsystem process: adopt the launching terminal's console, if any,
    // so a subcommand's output and any error are visible when run from one.
    #[cfg(windows)]
    let _ = crate::console::attach_parent();

    let cli = crate::cli::parse_args();

    // A subcommand means "do this and quit", not "open a window".
    if cli.command.is_some() {
        return crate::cli::execute(&cli);
    }
    let bcd = cli.bcd;

    // On Windows the UI cannot even *list* the entries without privileges. Two
    // ways to get them: the installed service broker (the UI stays unprivileged
    // and talks to it — no prompt at all), or a UAC prompt when the broker is
    // not installed.
    #[cfg(windows)]
    {
        use crate::switcher::winbroker;
        if !winbroker::is_installed() && !is_elevated() {
            let args: Vec<std::ffi::OsString> = match &bcd {
                Some(path) => vec!["--bcd".into(), path.clone().into_os_string()],
                None => Vec::new(),
            };
            return match crate::switcher::relaunch_self_elevated(&args) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    // Double-clicked, there is no console and no window yet:
                    // a message box is the only way to say why nothing opened.
                    crate::console::alert(&format!("{}: {e}", t!("elevation_refused")));
                    ExitCode::FAILURE
                }
            };
        }
    }

    match launch(bcd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            report_launch_error(&e.to_string());
            ExitCode::FAILURE
        }
    }
}

/// Reports a failure to open the window: a message box on Windows (no console),
/// stderr elsewhere.
fn report_launch_error(message: &str) {
    #[cfg(windows)]
    crate::console::alert(&format!("error: {message}"));
    #[cfg(not(windows))]
    eprintln!("error: {message}");
}

/// Opens the window. `bcd` optionally overrides the BCD hive path.
fn launch(bcd: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let locale = detect_locale();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OS Switcher")
            .with_inner_size([480.0, 620.0])
            .with_min_inner_size([400.0, 440.0])
            .with_icon(app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "OS Switcher",
        options,
        Box::new(move |cc| {
            install_style(&cc.egui_ctx);
            let mut app = SwitcherApp::new(bcd, locale);
            app.start(&cc.egui_ctx, Job::Reload);
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}

/// The system language, when it is one we speak.
pub fn detect_locale() -> &'static str {
    let sys = sys_locale::get_locale().unwrap_or_default();
    let code = sys.split(['-', '_']).next().unwrap_or("en");
    LANGS
        .iter()
        .map(|(c, _)| *c)
        .find(|c| *c == code)
        .unwrap_or("en")
}

// ---------------------------------------------------------------- background

/// A unit of work that must not block the UI thread.
enum Job {
    /// Just re-read the boot configuration.
    Reload,
    /// Apply a change, then re-read.
    Apply {
        verb: &'static str,
        selector: Option<String>,
    },
}

/// What a worker hands back: the fresh entry list, or what went wrong.
type JobResult = Result<Vec<Entry>, String>;

impl Job {
    /// The command line that performs this job, for the elevated re-run.
    fn args(&self, bcd: &Option<PathBuf>) -> Option<Vec<String>> {
        let Job::Apply { verb, selector } = self else {
            return None;
        };
        let mut args = Vec::new();
        if let Some(path) = bcd {
            args.push("--bcd".to_string());
            args.push(path.to_string_lossy().into_owned());
        }
        args.push((*verb).to_string());
        args.extend(selector.clone());
        Some(args)
    }
}

/// Runs `job` on a worker thread and wakes the UI when it lands.
fn spawn(ctx: &egui::Context, bcd: Option<PathBuf>, job: Job) -> Receiver<JobResult> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(perform(&bcd, job));
        ctx.request_repaint();
    });
    rx
}

fn perform(bcd: &Option<PathBuf>, job: Job) -> JobResult {
    // With the broker installed, read and write go through the service — no
    // elevation, on the unprivileged UI thread's worker.
    #[cfg(windows)]
    if crate::switcher::winbroker::is_installed() {
        return broker_perform(job);
    }

    if let Job::Apply { verb, selector } = &job {
        if is_elevated() {
            apply(bcd, verb, selector.as_deref()).map_err(|e| e.to_string())?;
        } else {
            // Unprivileged session: hand the whole action to an elevated copy
            // of this binary (a polkit prompt on Linux).
            let args = job.args(bcd).expect("an Apply job always has arguments");
            run_self_elevated(&args).map_err(|e| e.to_string())?;
        }
    }
    read_entries(bcd)
}

/// The broker equivalent of [`perform`]: apply through the service, then re-read
/// from it. Applies the same "hide firmware-only entries" rule as
/// [`read_entries`].
#[cfg(windows)]
fn broker_perform(job: Job) -> JobResult {
    use crate::switcher::{winbroker, Scope};

    if let Job::Apply { verb, selector } = &job {
        let outcome = match (*verb, selector.as_deref()) {
            ("default", Some(s)) => winbroker::set(s, Scope::Default),
            ("next", Some(s)) => winbroker::set(s, Scope::Once),
            _ => winbroker::clear_next(),
        };
        outcome.map_err(|e| e.to_string())?;
    }

    let entries = winbroker::get_entries().map_err(|e| e.to_string())?;
    let to_entry = |e: &winbroker::BrokerEntry| {
        Entry::display_only(
            e.key.clone(),
            e.label.clone(),
            e.kind,
            e.is_default,
            e.is_next,
        )
    };
    let os_only: Vec<Entry> = entries
        .iter()
        .filter(|e| e.kind != OsKind::Other)
        .map(&to_entry)
        .collect();
    let all: Vec<Entry> = entries.iter().map(&to_entry).collect();
    Ok(if os_only.is_empty() { all } else { os_only })
}

/// Performs one action directly, with the privileges this process already has.
fn apply(bcd: &Option<PathBuf>, verb: &str, selector: Option<&str>) -> crate::switcher::Result<()> {
    use crate::switcher::Scope;

    let mut switcher = open(bcd)?;
    match (verb, selector) {
        ("default", Some(s)) => switcher.set(s, Scope::Default).map(|_| ()),
        ("next", Some(s)) => switcher.set(s, Scope::Once).map(|_| ()),
        _ => switcher.clear_next(),
    }
}

fn open(bcd: &Option<PathBuf>) -> crate::switcher::Result<Switcher<crate::switcher::SystemNvram>> {
    match bcd {
        Some(path) => Switcher::detect_with_bcd(path),
        None => Ok(Switcher::detect()),
    }
}

/// The bootable systems, with firmware-only entries (network boot, setup…)
/// hidden — unless hiding them would leave nothing to show.
fn read_entries(bcd: &Option<PathBuf>) -> JobResult {
    let all = open(bcd).map_err(|e| e.to_string())?.entries();
    let os_only: Vec<Entry> = all
        .iter()
        .filter(|e| e.kind != OsKind::Other)
        .cloned()
        .collect();
    Ok(if os_only.is_empty() { all } else { os_only })
}

// ------------------------------------------------------------------ the app

/// A power action awaiting confirmation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Power {
    Reboot,
    Shutdown,
}

struct SwitcherApp {
    bcd: Option<PathBuf>,
    entries: Vec<Entry>,
    locale: &'static str,
    /// Last outcome to show: the text and whether it is a failure.
    status: Option<(String, bool)>,
    /// The worker currently running, if any.
    job: Option<Receiver<JobResult>>,
    confirm: Option<Power>,
    /// Whether the boot configuration has been read at least once.
    loaded: bool,
    /// Whether the desktop application menu has an entry for this app.
    in_menu: bool,
    /// Windows: whether launches skip the approval prompt.
    #[cfg(windows)]
    no_prompt: bool,
}

impl SwitcherApp {
    fn new(bcd: Option<PathBuf>, locale: &'static str) -> Self {
        SwitcherApp {
            bcd,
            entries: Vec::new(),
            locale,
            status: None,
            job: None,
            confirm: None,
            loaded: false,
            in_menu: crate::switcher::shortcut::is_present(),
            #[cfg(windows)]
            no_prompt: crate::switcher::winbroker::is_installed(),
        }
    }

    fn start(&mut self, ctx: &egui::Context, job: Job) {
        if self.job.is_some() {
            return;
        }
        self.status = None;
        self.job = Some(spawn(ctx, self.bcd.clone(), job));
    }

    /// Collects a finished worker, if there is one.
    fn poll(&mut self) {
        let Some(rx) = &self.job else { return };
        match rx.try_recv() {
            Ok(Ok(entries)) => {
                self.entries = entries;
                self.loaded = true;
                self.job = None;
            }
            Ok(Err(message)) => {
                self.status = Some((message, true));
                self.loaded = true;
                self.job = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.job = None,
        }
    }

    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn entry_named(&self, pick: impl Fn(&Entry) -> bool) -> Option<&Entry> {
        self.entries.iter().find(|e| pick(e))
    }
}

impl eframe::App for SwitcherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();

        egui::Frame::central_panel(ui.style())
            .inner_margin(Margin::same(18))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                // The action bar and the footer claim the bottom edge first;
                // the list then stretches into whatever is left.
                egui::Panel::bottom("bottom-bar")
                    .frame(egui::Frame::NONE)
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        ui.add_space(14.0);
                        self.actions(ui);
                        self.footer(ui);
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        self.header(ui);
                        ui.add_space(12.0);
                        self.summary(ui);
                        ui.add_space(14.0);
                        self.entry_list(ui);
                    });
            });

        self.confirmation(ui.ctx());
    }
}

impl SwitcherApp {
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(t!("app_title")).size(22.0).strong());
                ui.label(RichText::new(t!("subtitle")).weak());
            });
            ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                if self.busy() {
                    ui.add(egui::Spinner::new().size(18.0));
                }
                self.menu_entry_toggle(ui);
            });
        });
    }

    /// Registers the app in the desktop's application menu, so it can be found
    /// by name — and, from there, pinned wherever the user keeps things.
    fn menu_entry_toggle(&mut self, ui: &mut egui::Ui) {
        use crate::switcher::shortcut;

        let (label, add_hint) = if cfg!(windows) {
            (t!("menu_add_start"), t!("menu_add_hint_start"))
        } else {
            (t!("menu_add_launcher"), t!("menu_add_hint_launcher"))
        };
        let hint = if self.in_menu {
            t!("menu_remove_hint")
        } else {
            add_hint
        };

        if ui
            .selectable_label(self.in_menu, RichText::new(label).small())
            .on_hover_text(hint)
            .clicked()
        {
            let outcome = if self.in_menu {
                shortcut::remove()
            } else {
                shortcut::add()
            };
            match outcome {
                Ok(()) => {
                    self.in_menu = !self.in_menu;
                    self.status = None;
                }
                Err(e) => self.status = Some((e.to_string(), true)),
            }
        }
    }

    /// The one-line answer to "what happens when I press the power button?".
    fn summary(&mut self, ui: &mut egui::Ui) {
        let default = self.entry_named(|e| e.is_default).map(|e| e.label.clone());
        let next = self.entry_named(|e| e.is_next).map(|e| e.label.clone());

        let (fill, border) = surface(ui);
        egui::Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(14, 11))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                summary_line(
                    ui,
                    &t!("current_next"),
                    next.as_deref(),
                    &t!("next_none"),
                    true,
                );
                ui.add_space(4.0);
                summary_line(ui, &t!("current_default"), default.as_deref(), "—", false);
            });
    }

    fn entry_list(&mut self, ui: &mut egui::Ui) {
        if self.entries.is_empty() {
            if self.loaded {
                empty_state(ui);
            }
            return;
        }

        ui.label(RichText::new(t!("section_choose")).small().weak());
        ui.add_space(4.0);

        let entries = self.entries.clone();
        let busy = self.busy();
        let mut job = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for entry in &entries {
                    if let Some(next) = entry_card(ui, entry, busy) {
                        job = Some(next);
                    }
                    ui.add_space(8.0);
                }
            });

        if let Some(job) = job {
            let ctx = ui.ctx().clone();
            self.start(&ctx, job);
        }
    }

    /// Restart / shut down, plus whatever the last action had to say.
    fn actions(&mut self, ui: &mut egui::Ui) {
        let next = self.entry_named(|e| e.is_next).map(|e| e.label.clone());
        let label = match &next {
            Some(name) => t!("reboot_into", name = name).into_owned(),
            None => t!("reboot").into_owned(),
        };

        ui.horizontal(|ui| {
            // The restart button is the point of the app, so it gets the width
            // and the only saturated colour on the screen.
            let off_width = 110.0;
            let restart_width =
                (ui.available_width() - off_width - ui.spacing().item_spacing.x).max(120.0);
            let restart = egui::Button::new(
                RichText::new(label)
                    .strong()
                    .color(Color32::from_rgb(250, 251, 253)),
            )
            .fill(PRIMARY)
            .corner_radius(9)
            .min_size(Vec2::new(restart_width, 36.0));
            if ui.add_enabled(!self.busy(), restart).clicked() {
                self.confirm = Some(Power::Reboot);
            }

            let off = egui::Button::new(t!("shutdown"))
                .corner_radius(9)
                .min_size(Vec2::new(off_width, 36.0));
            if ui.add_enabled(!self.busy(), off).clicked() {
                self.confirm = Some(Power::Shutdown);
            }
        });

        if let Some((message, is_error)) = &self.status {
            ui.add_space(8.0);
            let colour = if *is_error {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(RichText::new(message).color(colour).small());
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        #[cfg(windows)]
        self.no_prompt_toggle(ui);

        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("language")).small().weak());
            let current = LANGS
                .iter()
                .find(|(c, _)| *c == self.locale)
                .map(|(_, n)| *n)
                .unwrap_or("English");
            egui::ComboBox::from_id_salt("language")
                .selected_text(current)
                .width(120.0)
                .show_ui(ui, |ui| {
                    for (code, name) in LANGS {
                        if ui
                            .selectable_label(self.locale == *code, *name)
                            .on_hover_text(*name)
                            .clicked()
                        {
                            self.locale = code;
                            rust_i18n::set_locale(code);
                        }
                    }
                });
        });
    }

    /// The "stop asking me" switch: installs the service broker (one UAC
    /// prompt), after which the app reads and writes without prompting.
    #[cfg(windows)]
    fn no_prompt_toggle(&mut self, ui: &mut egui::Ui) {
        use crate::switcher::winbroker;

        let mut wanted = self.no_prompt;
        let response = ui
            .checkbox(&mut wanted, RichText::new(t!("no_prompt")).small())
            .on_hover_text(t!("no_prompt_hint"));
        if response.changed() {
            let outcome = if wanted {
                winbroker::install()
            } else {
                winbroker::uninstall(false)
            };
            match outcome {
                Ok(()) => {
                    self.no_prompt = wanted;
                    self.status = None;
                }
                Err(e) => self.status = Some((e.to_string(), true)),
            }
        }
        ui.add_space(4.0);
    }

    /// A restart is not something to trigger by a stray click.
    fn confirmation(&mut self, ctx: &egui::Context) {
        let Some(power) = self.confirm else { return };
        let (title, body) = match power {
            Power::Reboot => (t!("confirm_reboot_title"), t!("confirm_reboot_body")),
            Power::Shutdown => (t!("confirm_shutdown_title"), t!("confirm_shutdown_body")),
        };

        let modal = egui::Modal::new(egui::Id::new("confirm-power")).show(ctx, |ui| {
            ui.set_max_width(300.0);
            ui.label(RichText::new(title).size(16.0).strong());
            ui.add_space(6.0);
            ui.label(body);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button(t!("cancel")).clicked() {
                    self.confirm = None;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button(RichText::new(t!("confirm_yes")).strong())
                        .clicked()
                    {
                        self.confirm = None;
                        let outcome = match power {
                            Power::Reboot => reboot(),
                            Power::Shutdown => shutdown(),
                        };
                        if let Err(e) = outcome {
                            self.status = Some((e.to_string(), true));
                        }
                    }
                });
            });
        });

        if modal.should_close() {
            self.confirm = None;
        }
    }
}

// ------------------------------------------------------------------ widgets

/// `label: value` where the value carries the weight.
fn summary_line(ui: &mut egui::Ui, label: &str, value: Option<&str>, fallback: &str, strong: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label} ")).small().weak());
        let text = value.unwrap_or(fallback);
        let mut rich = RichText::new(text);
        if value.is_some() && strong {
            rich = rich.strong();
        } else if value.is_none() {
            rich = rich.weak();
        }
        ui.label(rich);
    });
}

/// One bootable system: what it is, where it stands, and the two things that
/// can be done to it.
fn entry_card(ui: &mut egui::Ui, entry: &Entry, busy: bool) -> Option<Job> {
    let accent = accent(entry.kind);
    let visuals = ui.visuals().clone();
    let (surface_fill, border) = surface(ui);
    let (fill, stroke) = if entry.is_next {
        (accent.gamma_multiply(0.14), Stroke::new(1.5, accent))
    } else {
        (surface_fill, Stroke::new(1.0, border))
    };

    let mut job = None;
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(10)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                os_icon(ui, entry.kind, 30.0);
                ui.add_space(6.0);

                ui.vertical(|ui| {
                    ui.label(RichText::new(&entry.label).size(15.0).strong());
                    ui.horizontal(|ui| {
                        if entry.is_default {
                            badge(ui, &t!("badge_default"), visuals.weak_text_color());
                        }
                        if entry.is_next {
                            badge(ui, &t!("badge_once"), accent);
                        }
                        if !entry.is_default
                            && ui
                                .add_enabled(
                                    !busy,
                                    egui::Link::new(RichText::new(t!("action_default")).small()),
                                )
                                .clicked()
                        {
                            job = Some(Job::Apply {
                                verb: "default",
                                selector: Some(entry.key.clone()),
                            });
                        }
                    });
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (text, next) = if entry.is_next {
                        (
                            t!("action_cancel_once"),
                            Job::Apply {
                                verb: "clear",
                                selector: None,
                            },
                        )
                    } else {
                        (
                            t!("action_once"),
                            Job::Apply {
                                verb: "next",
                                selector: Some(entry.key.clone()),
                            },
                        )
                    };
                    let button = egui::Button::new(text).corner_radius(8);
                    if ui.add_enabled(!busy, button).clicked() {
                        job = Some(next);
                    }
                });
            });
        });
    job
}

/// A small pill, for a state that is worth naming but not worth a sentence.
fn badge(ui: &mut egui::Ui, text: &str, colour: Color32) {
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.18))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(colour));
        });
}

fn empty_state(ui: &mut egui::Ui) {
    let (fill, border) = surface(ui);
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(10)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(t!("empty_title")).strong());
            ui.add_space(4.0);
            ui.label(RichText::new(t!("empty_body")).small().weak());
        });
}

/// The one saturated colour on the screen, reserved for the restart button.
const PRIMARY: Color32 = Color32::from_rgb(0x2F, 0x6C, 0xE0);

/// Surface and border for the cards, picked per theme: egui's own `faint_bg`
/// sits too close to the panel to read as a separate surface.
fn surface(ui: &egui::Ui) -> (Color32, Color32) {
    if ui.visuals().dark_mode {
        (Color32::from_rgb(33, 37, 44), Color32::from_rgb(58, 64, 74))
    } else {
        (
            Color32::from_rgb(246, 247, 250),
            Color32::from_rgb(221, 225, 233),
        )
    }
}

/// The colour a system is known by.
fn accent(kind: OsKind) -> Color32 {
    match kind {
        OsKind::Windows => Color32::from_rgb(0, 120, 212),
        OsKind::Linux => Color32::from_rgb(233, 110, 40),
        OsKind::MacOs => Color32::from_rgb(140, 140, 150),
        OsKind::Other => Color32::from_rgb(120, 130, 150),
    }
}

/// A drawn mark per operating system — no icon files, no emoji font to miss.
fn os_icon(ui: &mut egui::Ui, kind: OsKind, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let colour = accent(kind);
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same((size * 0.3) as u8),
        colour.gamma_multiply(0.20),
    );

    let centre = rect.center();
    match kind {
        OsKind::Windows => {
            // Four panes, the shape everybody reads as "Windows".
            let pane = size * 0.22;
            let gap = size * 0.05;
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let corner = centre + Vec2::new(dx * (gap + pane) / 2.0, dy * (gap + pane) / 2.0)
                    - Vec2::splat(pane / 2.0);
                painter.rect_filled(
                    egui::Rect::from_min_size(corner, Vec2::splat(pane)),
                    CornerRadius::same(1),
                    colour,
                );
            }
        }
        // No emblem survives legibly at this size, and a muddy one looks worse
        // than none: the colour and the label carry the identification.
        OsKind::Linux | OsKind::MacOs => {
            painter.circle_filled(centre, size * 0.24, colour);
        }
        OsKind::Other => {
            painter.rect_filled(
                egui::Rect::from_center_size(centre, Vec2::splat(size * 0.34)),
                CornerRadius::same(2),
                colour,
            );
        }
    }
}

// ------------------------------------------------------------------- chrome

/// Spacing, corner radius and type scale — the difference between "an egui app"
/// and an app.
fn install_style(ctx: &egui::Context) {
    use egui::{FontFamily::Proportional, FontId, TextStyle};

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(21.0, Proportional)),
            (TextStyle::Body, FontId::new(14.0, Proportional)),
            (TextStyle::Button, FontId::new(13.5, Proportional)),
            (TextStyle::Small, FontId::new(11.5, Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(12.5, egui::FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        style.spacing.interact_size.y = 26.0;
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
            &mut style.visuals.widgets.noninteractive,
        ] {
            widget.corner_radius = CornerRadius::same(8);
        }
    });
}

/// The window and taskbar icon — the same mark the executable itself carries.
fn app_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    egui::IconData {
        rgba: crate::icon::rgba(SIZE),
        width: SIZE,
        height: SIZE,
    }
}
