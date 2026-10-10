//! The CADCraft egui front end.
//!
//! A thin shell over [`cadcraft_engine::Session`]: panels read engine state and act through
//! commands (`app.run(id, params)`) or the command line (`app.cmdline(text)`). Nothing here owns
//! drawing data. The layout follows the familiar CAD desktop: title and tool bar, file tabs,
//! Tool Sets on the left, the drawing area with its command line, Layers and Properties on the
//! right, layout tabs and drafting toggles in the status bar.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod canvas;
pub mod chrome;
pub mod cmdline;
pub mod control;
pub mod credits;
pub mod dialogs;
pub mod gpu;
pub mod icons;
pub mod layers;
pub mod menus;
pub mod palettes;
pub mod parametric;
pub mod quick;
pub mod theme;

use std::sync::mpsc::Receiver;

use cadcraft_engine::Session;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use control::{ControlRequest, ControlResponse};

/// Persisted UI state (serde, so automation can read and set it).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub show_toolsets: bool,
    pub show_palettes: bool,
    pub show_toolbar: bool,
    pub show_file_tabs: bool,
    pub show_status_bar: bool,
    pub show_command_line: bool,
    pub show_viewcube: bool,
    pub show_ucs_icon: bool,
    pub show_layer_list: bool,
    /// "Drafting" or "Modeling".
    pub toolset_tab: String,
    pub collapsed_groups: Vec<String>,
    pub properties_all: bool,
    /// Show an in-window menu bar (platforms without a native menu bar, or on request).
    pub in_window_menu: bool,
    pub start_tab: bool,
    pub dialog: Option<String>,
    pub history_lines: usize,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            show_toolsets: true,
            show_palettes: true,
            show_toolbar: true,
            show_file_tabs: true,
            show_status_bar: true,
            show_command_line: true,
            show_viewcube: true,
            show_ucs_icon: true,
            show_layer_list: false,
            toolset_tab: "Drafting".into(),
            collapsed_groups: Vec::new(),
            properties_all: true,
            in_window_menu: !cfg!(target_os = "macos"),
            start_tab: false,
            dialog: None,
            history_lines: 3,
        }
    }
}

/// Host services (file pickers, clipboard) injected by the app so this crate stays portable.
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn() -> Option<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str) -> Option<String>>>,
}

pub struct CadApp {
    pub session: Session,
    pub ui: UiState,
    pub services: Services,
    pub canvas: canvas::CanvasState,
    pub cmd: cmdline::CmdLine,
    pub status: Option<(String, f64)>,
    pub integrated_titlebar: bool,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_shots: Vec<(u64, Option<String>, std::sync::mpsc::Sender<Value>, f64)>,
    queued_shots: Vec<(u64, f64, u32)>,
    shot_token: u64,
    pub synthetic: Vec<egui::Event>,
    styled: bool,
    pub frame_ms: f64,
    pub quit_requested: bool,
    /// Seconds since the last autosave attempt (desktop SAVETIME timer).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    autosave_accum_s: f64,
    /// Print-preview image and the `layout@revision` key it was rendered for.
    pub preview_tex: Option<egui::TextureHandle>,
    pub preview_key: String,
    /// Pending unsaved-changes prompt (close/close-all/quit target).
    pub unsaved_ask: Option<crate::dialogs::UnsavedMode>,
}

impl CadApp {
    pub fn new(session: Session, services: Services) -> Self {
        CadApp {
            session,
            ui: UiState::default(),
            services,
            canvas: canvas::CanvasState::default(),
            cmd: cmdline::CmdLine::default(),
            status: None,
            integrated_titlebar: false,
            control_rx: None,
            pending_shots: Vec::new(),
            queued_shots: Vec::new(),
            shot_token: 0,
            synthetic: Vec::new(),
            styled: false,
            frame_ms: 0.0,
            quit_requested: false,
            autosave_accum_s: 0.0,
            preview_tex: None,
            preview_key: String::new(),
            unsaved_ask: None,
        }
    }

    /// Draw the canvas on the GPU with the app's wgpu render state (eframe's
    /// `CreationContext::wgpu_render_state`). Without it the canvas draws on the CPU.
    pub fn set_wgpu(&mut self, rs: &egui_wgpu::RenderState) {
        self.canvas.gpu = Some(gpu::install(rs));
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Run a command programmatically (or a UI command).
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        let r = self.session.execute(id, &params).map_err(|e| e.to_string());
        if let Ok(v) = &r
            && let Some(m) = v.get("message").and_then(Value::as_str)
        {
            for l in m.lines() {
                self.session.echo(l.to_string());
            }
        }
        if let Err(e) = &r {
            self.session.echo(e.clone());
        }
        r
    }

    /// Start a command as if typed (interactive when it has prompts).
    pub fn start(&mut self, name: &str) {
        if menus::run_ui_command(self, name, &Value::Null).is_some() {
            return;
        }
        if let Err(e) = self.session.start(name) {
            self.session.echo(e.to_string());
        }
    }

    /// Submit a line of command-line text.
    pub fn cmdline(&mut self, text: &str) {
        if self.session.running.is_none() {
            let first = text.split_whitespace().next().unwrap_or("");
            if !first.is_empty() && menus::run_ui_command(self, &first.to_ascii_lowercase(), &Value::Null).is_some() {
                return;
            }
        }
        if let Err(e) = self.session.cmdline(text) {
            self.session.echo(e.to_string());
        }
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), now_ms()));
    }

    pub fn open_path(&mut self, path: &str) {
        if let Err(e) = self.run("open", json!({ "path": path })) {
            self.set_status(e);
        } else {
            self.ui.start_tab = false;
            self.canvas.zoom_pending = true;
        }
    }

    /// Automatic save on the SAVETIME interval (desktop only: the web build has
    /// no files to write to). The host owns the wall clock and reports elapsed
    /// minutes; the engine decides whether saving is needed. A completed save
    /// is echoed to the command line, anything else stays silent.
    /// `request_repaint_after` wakes the event loop for the next due save.
    #[cfg(not(target_arch = "wasm32"))]
    fn autosave_tick_frame(&mut self, ctx: &egui::Context) {
        let savetime = self.session.settings.savetime;
        if savetime > 0.0 && !self.session.docs.is_empty() {
            let dt = ctx.input(|i| f64::from(i.unstable_dt).max(0.0));
            let (accum, due) = autosave_tick(self.autosave_accum_s, dt, savetime);
            self.autosave_accum_s = accum;
            if let Some(minutes) = due
                && let Ok(v) = self.session.execute("autosave", &json!({ "minutes": minutes, "all": true }))
                && v.get("autosaved").and_then(Value::as_bool).unwrap_or(false)
                && let Some(m) = v.get("message").and_then(Value::as_str)
            {
                self.session.echo(m);
            }
            let remaining = (savetime * 60.0 - self.autosave_accum_s).max(0.0);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(remaining.min(3600.0)));
        } else {
            self.autosave_accum_s = 0.0;
        }
    }

    /// The OS window [X] with unsaved work: cancel the close and ask instead.
    /// (Desktop only; nothing to save to on the web.)
    #[cfg(not(target_arch = "wasm32"))]
    fn intercept_window_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if !self.session.docs.iter().any(|d| d.is_dirty()) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.ui.dialog.as_deref() != Some("unsaved") {
            crate::dialogs::ask_unsaved(self, crate::dialogs::UnsavedMode::Quit);
        }
    }

    /// Per-frame logic before layout.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if !self.styled {
            theme::install_fonts(ctx);
            theme::apply(ctx);
            self.styled = true;
        }
        self.drain_control(ctx);
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
        self.collect_screenshots(ctx);
        self.issue_screenshots(ctx);
        menus::shortcuts(self, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        self.autosave_tick_frame(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        self.intercept_window_close(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            let p = f.path().to_string_lossy().to_string();
            if !p.is_empty() {
                self.open_path(&p);
            }
        }
    }

    /// Inject synthetic events (one pointer event per frame).
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        if self.synthetic.is_empty() {
            return;
        }
        let n = match self.synthetic.first() {
            Some(egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. }) => 1,
            _ => self.synthetic.iter().position(|e| matches!(e, egui::Event::Key { pressed: false, .. })).map_or(self.synthetic.len(), |i| i + 1),
        };
        if let Some(egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. }) = self.synthetic.first() {
            raw.events.push(egui::Event::PointerMoved(*p));
        }
        let n = n.min(self.synthetic.len());
        raw.events.extend(self.synthetic.drain(..n));
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let t0 = now_ms();
        let t = theme::Tokens::get();
        chrome::title_and_toolbar(self, ui);
        if self.ui.in_window_menu {
            menus::menu_bar(self, ui);
        }
        if self.ui.show_status_bar {
            chrome::status_bar(self, ui);
        }
        if self.ui.show_file_tabs {
            chrome::file_tabs(self, ui);
        }
        let has_doc = !self.session.docs.is_empty() && !self.ui.start_tab;
        if has_doc && self.ui.show_toolsets {
            palettes::toolsets(self, ui);
        }
        if has_doc && self.ui.show_palettes {
            palettes::right_palettes(self, ui);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
            if has_doc {
                canvas::show(self, ui);
            } else {
                chrome::start_page(self, ui);
            }
        });
        dialogs::show(self, ui.ctx());
        self.frame_ms = now_ms() - t0;
    }

    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Screenshot { path } => {
                    self.shot_token += 1;
                    let token = self.shot_token;
                    self.queued_shots.push((token, now_ms() + 120.0, 0));
                    self.pending_shots.push((token, path, reply, now_ms() + 8000.0));
                }
            }
            ctx.request_repaint();
        }
        self.control_rx = Some(rx);
    }

    fn issue_screenshots(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        self.queued_shots.retain_mut(|(token, at, frames)| {
            *frames += 1;
            if now >= *at && *frames >= 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(*token)));
                false
            } else {
                true
            }
        });
        if !self.queued_shots.is_empty() || !self.pending_shots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_shots.is_empty() {
            return;
        }
        let events: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let token = user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).copied()?;
                        Some((token, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (token, image) in events {
            if let Some(i) = self.pending_shots.iter().position(|(t, ..)| *t == token) {
                let (_, path, reply, _) = self.pending_shots.remove(i);
                let _ = reply.send(control::save_screenshot(&image, path.as_deref()));
            }
        }
        let now = now_ms();
        self.pending_shots.retain(|(_, _, reply, deadline)| {
            if now < *deadline {
                return true;
            }
            let _ = reply.send(json!({"ok": false, "error": "no frame was presented (screen locked or window hidden); use ui.render"}));
            false
        });
    }
}

pub fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0.0
    }
}

/// Autosave timer step: accumulate frame time, and once SAVETIME minutes elapse
/// report the elapsed minutes so the host can run `autosave`. Pure for tests;
/// NaN counts as off.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn autosave_tick(accum_s: f64, dt_s: f64, savetime_min: f64) -> (f64, Option<f64>) {
    if !savetime_min.is_finite() || savetime_min <= 0.0 {
        return (0.0, None);
    }
    let accum = (accum_s + dt_s.max(0.0)).min(1e9);
    if accum >= savetime_min * 60.0 { (0.0, Some(accum / 60.0)) } else { (accum, None) }
}

#[cfg(test)]
mod tests {
    use super::autosave_tick;
    use super::{CadApp, Services, Session};
    use serde_json::json;

    #[test]
    fn savetime_timer() {
        assert_eq!(autosave_tick(0.0, 1.0, 0.0), (0.0, None), "SAVETIME 0 is off");
        assert_eq!(autosave_tick(999.0, 1.0, f64::NAN), (0.0, None), "NaN is off");
        assert_eq!(autosave_tick(0.0, 60.0, 10.0), (60.0, None));
        assert_eq!(autosave_tick(599.0, 2.0, 10.0), (0.0, Some(601.0 / 60.0)), "fires and resets");
        assert_eq!(autosave_tick(0.0, -5.0, 10.0), (0.0, None), "negative frames clamp");
    }

    fn fake_io() {
        cadcraft_engine::cmd::file::set_io(cadcraft_engine::cmd::file::IoHooks {
            read: |_, _| Err("no reader in tests".into()),
            write: |_, name| Ok(format!("fake {name}").into_bytes()),
            plot: None,
        });
    }

    fn writer_ok(s: &mut Session) -> bool {
        s.execute("document.bytes", &json!({})).is_ok()
    }

    fn dirty_app(pick: Option<Box<dyn Fn(&str) -> Option<String>>>) -> CadApp {
        let mut app = CadApp::new(Session::new(), Services { pick_open: None, pick_save: pick });
        app.session.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
        assert!(app.session.state().unwrap().is_dirty());
        app
    }

    #[test]
    fn save_all_cancel_keeps_drawings() {
        fake_io();
        let mut app = dirty_app(None);
        if !writer_ok(&mut app.session) {
            return; // another test installed hooks without a writer first
        }
        // No picker: cancelling saves nothing and closes nothing.
        assert_eq!(crate::dialogs::save_all_dirty(&mut app), Ok(false));
        assert!(app.session.state().unwrap().is_dirty());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn save_all_picks_names_for_untitled() {
        fake_io();
        let dir = std::env::temp_dir().join(format!("cadcraft-saveall-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let picked = dir.join("picked.dxf").to_string_lossy().to_string();
        let mut app = dirty_app(Some(Box::new(move |_| Some(picked.clone()))));
        if !writer_ok(&mut app.session) {
            return;
        }
        assert_eq!(crate::dialogs::save_all_dirty(&mut app), Ok(true));
        assert!(!app.session.state().unwrap().is_dirty());
        assert!(dir.join("picked.dxf").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_all_refuses_while_command_runs() {
        fake_io();
        let mut app = dirty_app(None);
        if !writer_ok(&mut app.session) {
            return;
        }
        app.session.start("line").unwrap();
        assert!(app.session.running.is_some());
        assert!(crate::dialogs::save_all_dirty(&mut app).is_err());
        app.session.cancel();
    }

    /// Drive one egui frame headlessly (real input semantics, no display).
    fn dialog_frame(ctx: &egui::Context, app: &mut CadApp, events: Vec<egui::Event>, t: &mut f64) {
        *t += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            time: Some(*t),
            events,
            ..Default::default()
        };
        ctx.begin_pass(raw);
        crate::dialogs::show(app, ctx);
        ctx.end_pass().textures_delta.clear();
    }

    fn click_at(ctx: &egui::Context, app: &mut CadApp, x: f32, y: f32, t: &mut f64) {
        use egui::Event;
        let pos = egui::pos2(x, y);
        dialog_frame(ctx, app, vec![Event::PointerMoved(pos)], t);
        dialog_frame(
            ctx,
            app,
            vec![Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::default() }],
            t,
        );
        dialog_frame(
            ctx,
            app,
            vec![Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::default() }],
            t,
        );
    }

    fn ask_close(app: &mut CadApp) {
        app.unsaved_ask = Some(crate::dialogs::UnsavedMode::Close(app.session.active));
        app.ui.dialog = Some("unsaved".into());
    }

    /// Click every point of a grid over the dialog area until `done` holds.
    /// Re-arms the dialog when a click dismisses it without finishing the job
    /// (e.g. hitting Save with no picker, or the window X).
    fn grid_click_until(
        ctx: &egui::Context,
        app: &mut CadApp,
        t: &mut f64,
        x_range: (f32, f32),
        y_range: (f32, f32),
        done: impl Fn(&CadApp) -> bool,
    ) -> Option<(f32, f32)> {
        let mut y = y_range.0;
        while y <= y_range.1 {
            let mut x = x_range.0;
            while x <= x_range.1 {
                click_at(ctx, app, x, y, t);
                if done(app) {
                    return Some((x, y));
                }
                if app.ui.dialog.is_none() {
                    ask_close(app);
                }
                x += 15.0;
            }
            y += 12.0;
        }
        done(app).then_some((-1.0, -1.0))
    }

    #[test]
    fn unsaved_cancel_dismisses_without_closing() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let mut app = dirty_app(None);
        ask_close(&mut app);
        dialog_frame(&ctx, &mut app, vec![], &mut t);
        assert_eq!(app.ui.dialog.as_deref(), Some("unsaved"));
        // Right-hand side only: Cancel lives there, Save (left) is excluded so
        // only Cancel itself can satisfy this. Fine steps: the headless
        // layout compresses widgets, so the button is smaller than on screen.
        let mut hit = None;
        let mut y = 480.0;
        while hit.is_none() && y <= 620.0 {
            let mut x = 848.0;
            while hit.is_none() && x <= 952.0 {
                click_at(&ctx, &mut app, x, y, &mut t);
                if app.ui.dialog.is_none() {
                    hit = Some((x, y));
                }
                x += 4.0;
            }
            y += 4.0;
        }
        assert!(hit.is_some(), "no click dismissed the dialog");
        assert_eq!(app.session.docs.len(), 1, "nothing may close on the way out");
        assert!(app.session.state().unwrap().is_dirty(), "drawing stays dirty");
    }

    #[test]
    fn unsaved_dont_save_closes() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let mut app = dirty_app(None);
        ask_close(&mut app);
        let hit = grid_click_until(&ctx, &mut app, &mut t, (560.0, 1050.0), (480.0, 620.0), |a| a.session.docs.is_empty());
        assert!(hit.is_some(), "no click closed the drawing");
        assert!(app.ui.dialog.is_none());
    }
}
