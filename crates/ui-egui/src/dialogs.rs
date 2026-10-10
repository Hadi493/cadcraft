//! Dialogs: Drafting Settings, About, command reference, blocks; dispatches the Layer Properties
//! Manager ([`crate::layers`]), Quick Select ([`crate::quick`]) and Parameters Manager
//! ([`crate::parametric`]).

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    mtext_editor(app, ctx);
    crate::quick::quick_properties(app, ctx);
    let Some(d) = app.ui.dialog.clone() else { return };
    let mut open = true;
    match d.as_str() {
        "layers" => crate::layers::dialog(app, ctx, &mut open),
        "qselect" => crate::quick::qselect_dialog(app, ctx, &mut open),
        "parameters" => crate::parametric::parameters_dialog(app, ctx, &mut open),
        "dsettings" => dsettings(app, ctx, &mut open),
        "preview" => print_preview(app, ctx, &mut open),
        "recovery" => recovery_dialog(app, ctx, &mut open),
        "unsaved" => unsaved_dialog(app, ctx, &mut open),
        "about" => about(ctx, &mut open),
        "commands" => commands(app, ctx, &mut open),
        "blocks" => blocks(app, ctx, &mut open),
        _ => open = false,
    }
    if !open {
        app.ui.dialog = None;
    }
}

fn dsettings(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("Drafting Settings").open(open).resizable(false).show(ctx, |ui| {
        let s = &mut app.session.settings;
        ui.heading("Snap and Grid");
        ui.checkbox(&mut s.snapmode, "Snap On (F9)");
        ui.horizontal(|ui| {
            ui.label("Snap X spacing");
            ui.add(egui::DragValue::new(&mut s.snapunit.x).speed(0.05).range(0.0001..=1e6));
            ui.label("Y");
            ui.add(egui::DragValue::new(&mut s.snapunit.y).speed(0.05).range(0.0001..=1e6));
        });
        ui.checkbox(&mut s.gridmode, "Grid On (F7)");
        ui.horizontal(|ui| {
            ui.label("Grid spacing");
            ui.add(egui::DragValue::new(&mut s.gridunit.x).speed(0.05).range(0.0001..=1e6));
            ui.label("Major line every");
            ui.add(egui::DragValue::new(&mut s.gridmajor).range(1..=100));
        });
        ui.separator();
        ui.heading("Polar Tracking");
        ui.checkbox(&mut s.polarmode, "Polar Tracking On (F10)");
        let mut deg = s.polarang.to_degrees();
        ui.horizontal(|ui| {
            ui.label("Increment angle");
            egui::ComboBox::from_id_salt("polarang").selected_text(format!("{deg}")).show_ui(ui, |ui| {
                for a in [90.0, 45.0, 30.0, 22.5, 18.0, 15.0, 10.0, 5.0] {
                    ui.selectable_value(&mut deg, a, format!("{a}"));
                }
            });
        });
        s.polarang = deg.to_radians();
        ui.separator();
        ui.heading("Object Snap");
        let mut on = s.osmode & cadcraft_engine::snap::mode::OFF == 0;
        if ui.checkbox(&mut on, "Object Snap On (F3)").changed() {
            if on {
                s.osmode &= !cadcraft_engine::snap::mode::OFF;
            } else {
                s.osmode |= cadcraft_engine::snap::mode::OFF;
            }
        }
        egui::Grid::new("osnap_grid").num_columns(2).show(ui, |ui| {
            for (i, (bit, name)) in cadcraft_engine::snap::mode::ALL.iter().enumerate() {
                let mut v = s.osmode & bit != 0;
                if ui.checkbox(&mut v, *name).changed() {
                    if v {
                        s.osmode |= bit;
                    } else {
                        s.osmode &= !bit;
                    }
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.separator();
        ui.checkbox(&mut s.dynmode, "Enable Dynamic Input (F12)");
        ui.checkbox(&mut s.orthomode, "Ortho (F8)");
    });
}

fn about(ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let tab_id = egui::Id::new("about_tab");
    egui::Window::new("About CADCraft").open(open).default_size(vec2(640.0, 420.0)).collapsible(false).show(ctx, |ui| {
        let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
        ui.horizontal(|ui| {
            for (i, l) in ["About", "Contributors", "Models"].iter().enumerate() {
                if ui.selectable_label(tab == i as u8, *l).clicked() {
                    tab = i as u8;
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(tab_id, tab));
        ui.separator();
        match tab {
            1 => crate::credits::contributors_ui(ui),
            2 => crate::credits::models_ui(ui),
            _ => {
                ui.heading("CADCraft");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("Computer-aided design and drafting: an open-source, clean-room CAD application written in pure Rust.");
                ui.add_space(6.0);
                ui.label(RichText::new("One of the Crafting Apps by the ArtCraft team and community.").color(t.text_dim));
                ui.hyperlink_to("getartcraft.com/apps/cadcraft", "https://getartcraft.com/apps/cadcraft");
                ui.hyperlink_to("Join us on Discord", "https://discord.gg/artcraft");
                ui.add_space(6.0);
                ui.label(RichText::new("MIT OR Apache-2.0. Not affiliated with Autodesk, Inc.").small().color(t.text_faint));
            }
        }
    });
}

fn commands(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut start = None;
    egui::Window::new("Command Reference").open(open).default_size(vec2(640.0, 480.0)).show(ctx, |ui| {
        let id = ui.id().with("cmdfilter");
        let mut filter = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.text_edit_singleline(&mut filter);
        });
        ui.data_mut(|d| d.insert_temp(id, filter.clone()));
        let f = filter.to_ascii_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("cmdref").striped(true).num_columns(3).show(ui, |ui| {
                for c in cadcraft_engine::command_specs() {
                    if !f.is_empty() && !c.id.contains(&f) && !c.label.to_ascii_lowercase().contains(&f) {
                        continue;
                    }
                    if ui.link(c.id.to_ascii_uppercase()).clicked() {
                        start = Some(c.id);
                    }
                    ui.label(c.label);
                    ui.label(RichText::new(if c.aliases.is_empty() { String::new() } else { c.aliases.join(", ").to_ascii_uppercase() }).small());
                    ui.end_row();
                }
            });
        });
    });
    if let Some(c) = start {
        app.ui.dialog = None;
        app.start(c);
    }
}

/// What an unsaved-changes prompt applies to.
#[derive(Clone, Debug)]
pub enum UnsavedMode {
    /// Close one drawing (file-tab index).
    Close(usize),
    /// Close every drawing.
    CloseAll,
    /// Quit the application.
    Quit,
}

/// Arm the unsaved-changes dialog for `mode` (callers check dirtiness first).
pub fn ask_unsaved(app: &mut CadApp, mode: UnsavedMode) {
    app.unsaved_ask = Some(mode);
    app.ui.dialog = Some("unsaved".into());
}

/// Save every dirty drawing, asking for a file name where needed (file
/// picker). Returns false when the user cancels, before anything is closed.
/// The active drawing is restored afterwards.
pub(crate) fn save_all_dirty(app: &mut CadApp) -> Result<bool, String> {
    if app.session.running.is_some() {
        return Err("finish the running command before saving".into());
    }
    let back = app.session.active;
    let restore = |app: &mut CadApp| {
        if back < app.session.docs.len() {
            app.session.active = back;
        }
    };
    let targets: Vec<(usize, String, Option<String>)> =
        app.session.docs.iter().enumerate().filter(|(_, d)| d.is_dirty()).map(|(i, d)| (i, d.title.clone(), d.path.clone())).collect();
    for (i, title, path) in targets {
        app.session.active = i;
        match path {
            Some(_) => {
                if let Err(e) = app.session.execute("qsave", &Value::Null) {
                    let msg = e.to_string();
                    restore(app);
                    return Err(msg);
                }
            }
            None => match app.services.pick_save.as_ref().and_then(|f| f(&format!("{title}.dxf"))) {
                Some(p) => {
                    if let Err(e) = app.session.execute("saveas", &json!({ "path": p })) {
                        let msg = e.to_string();
                        restore(app);
                        return Err(msg);
                    }
                }
                None => {
                    restore(app);
                    return Ok(false);
                }
            },
        }
    }
    restore(app);
    Ok(true)
}

/// Unsaved-changes prompt: Save / Don't Save / Cancel for a close or quit.
fn unsaved_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let dirty: Vec<String> = app.session.docs.iter().filter(|d| d.is_dirty()).map(|d| d.title.clone()).collect();
    if dirty.is_empty() {
        // Nothing left to ask about (saved elsewhere meanwhile).
        app.unsaved_ask = None;
        *open = false;
        return;
    }
    let mut done = false;
    let center = ctx.viewport_rect().center();
    egui::Window::new("Unsaved Changes").open(open).default_pos(center).pivot(egui::Align2::CENTER_CENTER).default_size(vec2(460.0, 200.0)).show(
        ctx,
        |ui| {
            ui.label("Save changes before closing?");
            ui.label(dirty.join(", "));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    match save_all_dirty(app) {
                        Ok(true) => finish_unsaved(app),
                        Ok(false) => {}
                        Err(e) => app.session.echo(e),
                    }
                    done = true;
                }
                if ui.button("Don't Save").clicked() {
                    match app.unsaved_ask.clone() {
                        Some(UnsavedMode::Close(i)) => {
                            if let Err(e) = app.session.execute("close", &json!({ "index": i, "force": true })) {
                                app.session.echo(e.to_string());
                            }
                        }
                        Some(UnsavedMode::CloseAll) => {
                            if let Err(e) = app.session.execute("closeall", &json!({ "force": true })) {
                                app.session.echo(e.to_string());
                            }
                        }
                        Some(UnsavedMode::Quit) | None => app.quit_requested = true,
                    }
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
        },
    );
    if done || !*open {
        app.unsaved_ask = None;
        app.ui.dialog = None;
    }

    /// Run the pending close/quit now that saving is done.
    fn finish_unsaved(app: &mut CadApp) {
        match app.unsaved_ask.clone() {
            Some(UnsavedMode::Close(i)) => {
                if let Err(e) = app.session.execute("close", &json!({ "index": i, "force": true })) {
                    app.session.echo(e.to_string());
                }
            }
            Some(UnsavedMode::CloseAll) => {
                if let Err(e) = app.session.execute("closeall", &json!({ "force": true })) {
                    app.session.echo(e.to_string());
                }
            }
            Some(UnsavedMode::Quit) | None => app.quit_requested = true,
        }
    }
}

/// After launch, offer any recovery files left behind by a crash (desktop only).
/// Called once at startup; a no-op when there is nothing to recover.
pub fn startup_recovery(app: &mut CadApp) {
    if query_recovery(app).is_empty() {
        return;
    }
    app.session.echo("Found recoverable file(s) from a previous session (Drawing Recovery dialog).".to_string());
    app.ui.dialog = Some("recovery".into());
}

fn query_recovery(app: &mut CadApp) -> Vec<(String, String, u64)> {
    match app.session.execute("recovery.list", &Value::Null) {
        Ok(v) => v
            .get("files")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|f| Some((f.get("name")?.as_str()?.to_string(), f.get("path")?.as_str()?.to_string(), f.get("bytes")?.as_u64()?)))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Drawing recovery: open or discard automatic-save files from crashed sessions.
/// The list loads on first show and after every action.
fn recovery_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let list_id = egui::Id::new("recovery_files");
    let loaded_id = egui::Id::new("recovery_loaded");
    let files: Vec<(String, String, u64)> = ctx.data_mut(|d| {
        if d.get_temp::<bool>(loaded_id).unwrap_or(false) {
            d.get_temp(list_id).unwrap_or_default()
        } else {
            d.insert_temp(loaded_id, true);
            let fresh = query_recovery(app);
            d.insert_temp(list_id, fresh.clone());
            fresh
        }
    });
    let mut invalidate = false;
    let mut close = false;
    let mut action: Option<(bool, String)> = None; // (open?, path)
    let center = ctx.viewport_rect().center();
    egui::Window::new("Drawing Recovery").open(open).default_pos(center).pivot(egui::Align2::CENTER_CENTER).default_size(vec2(520.0, 300.0)).show(
        ctx,
        |ui| {
            ui.label("These automatic-save files were left by a previous session:");
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if files.is_empty() {
                    ui.label("Nothing to recover.");
                }
                for (name, path, bytes) in &files {
                    ui.horizontal(|ui| {
                        ui.label(format!("{name} ({bytes} bytes)")).on_hover_text(path);
                        if ui.button("Open").clicked() {
                            action = Some((true, path.clone()));
                        }
                        if ui.button("Delete").clicked() {
                            action = Some((false, path.clone()));
                        }
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() {
                    invalidate = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        },
    );
    if let Some((open_it, path)) = action {
        if open_it {
            match app.session.execute("recover", &json!({ "path": path })) {
                Ok(v) => {
                    let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
                    app.session.echo(format!("Recovered {path} ({msg})"));
                    app.canvas.zoom_pending = true;
                }
                Err(e) => app.session.echo(format!("Recovery of {path} failed: {e}")),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !open_it {
            let _ = std::fs::remove_file(&path);
        }
        invalidate = true;
    }
    if invalidate {
        ctx.data_mut(|d| d.remove::<bool>(loaded_id));
    }
    if close {
        *open = false;
    }
}

fn blocks(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("Blocks").open(open).default_size(vec2(320.0, 360.0)).show(ctx, |ui| {
        let Ok(d) = app.session.doc() else { return };
        let names: Vec<&String> = d.blocks.keys().filter(|k| !k.starts_with('*')).collect();
        if names.is_empty() {
            ui.label("No blocks defined in this drawing.");
        }
        for n in names {
            ui.label(n);
        }
    });
}

/// Print preview: renders the current plot display list to a PNG through the
/// engine `preview` command and shows it. Re-renders when the layout changes,
/// the drawing is edited, or Refresh is pressed.
fn print_preview(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("preview_layout");
    let mut names = vec!["Model".to_string()];
    if let Ok(d) = app.session.doc() {
        names.extend(d.layouts.iter().map(|l| l.name.clone()));
    }
    let current = app
        .session
        .state()
        .map(|s| match &s.space {
            cadcraft_doc::Space::Paper(n) => n.clone(),
            _ => "Model".into(),
        })
        .unwrap_or_else(|_| "Model".into());
    let mut layout = ctx.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.clone());
    if !names.contains(&layout) {
        layout = current;
    }
    let rev = app.session.state().map(|s| s.revision).unwrap_or(0);
    let key = format!("{layout}@{rev}");
    let mut refresh = app.preview_key != key || app.preview_tex.is_none();
    // Open in the centre of the screen (still draggable afterwards).
    let center = ctx.viewport_rect().center();
    egui::Window::new("Print Preview").open(open).default_pos(center).pivot(egui::Align2::CENTER_CENTER).default_size(vec2(720.0, 600.0)).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.label("Layout");
                egui::ComboBox::from_id_salt("preview_layout_combo").selected_text(&layout).show_ui(ui, |ui| {
                    for n in &names {
                        ui.selectable_value(&mut layout, n.clone(), n);
                    }
                });
                if ui.button("Refresh").clicked() {
                    refresh = true;
                }
            });
            ui.separator();
            if refresh {
                app.preview_tex = None;
                match app.session.execute("preview", &json!({"layout": layout, "width": 1100})) {
                    Ok(v) => {
                        let bytes = v.get("data").and_then(Value::as_str).map(cadcraft_engine::cmd::file::base64_decode).unwrap_or_default();
                        match bytes.and_then(|b| image::load_from_memory(&b).ok()) {
                            Some(img) => {
                                let rgba = img.to_rgba8();
                                let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                                let px = rgba.into_raw();
                                let tex =
                                    ctx.load_texture("print_preview", egui::ColorImage::from_rgba_unmultiplied([w, h], &px), Default::default());
                                app.preview_tex = Some(tex);
                                app.preview_key = key.clone();
                            }
                            None => app.session.echo("Print Preview: could not render that layout.".to_string()),
                        }
                    }
                    Err(e) => app.session.echo(format!("Print Preview: {e}")),
                }
            }
            if let Some(tex) = &app.preview_tex {
                let size = tex.size_vec2();
                let scale = (ui.available_width() / size.x).min(1.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.image((tex.id(), size * scale));
                });
            } else {
                ui.label("No preview yet.");
            }
        },
    );
    ctx.data_mut(|d| d.insert_temp(id, layout));
}

/// The multiline text editor shown while MTEXT asks for its contents.
fn mtext_editor(app: &mut CadApp, ctx: &egui::Context) {
    let active = app.session.running.as_ref().is_some_and(|r| r.id == "mtext")
        && app.session.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point);
    let id = egui::Id::new("mtext_editor_buffer");
    if !active {
        ctx.data_mut(|d| d.remove::<String>(id));
        return;
    }
    let mut buf = ctx.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
    let mut submit = None;
    let mut cancel = false;
    egui::Window::new("Text Editor").collapsible(false).resizable(true).default_size(vec2(460.0, 220.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Style: Standard");
            ui.separator();
            let h = app.session.doc().map(|d| d.header.f64("TEXTSIZE", 0.2)).unwrap_or(0.2);
            ui.label(format!("Height: {h:.4}"));
            ui.separator();
            if ui.button("B").on_hover_text("Bold").clicked() {
                buf.push_str("{\\fArial|b1;}");
            }
            if ui.button("⅟").on_hover_text("Stack (type 1/2 then select)").clicked() {
                buf.push_str("\\S1/2;");
            }
            if ui.button("°").on_hover_text("Degree").clicked() {
                buf.push_str("%%d");
            }
            if ui.button("±").on_hover_text("Plus/minus").clicked() {
                buf.push_str("%%p");
            }
            if ui.button("⌀").on_hover_text("Diameter").clicked() {
                buf.push_str("%%c");
            }
        });
        let r = ui.add(
            egui::TextEdit::multiline(&mut buf).desired_rows(6).desired_width(f32::INFINITY).hint_text("Type text; Enter starts a new paragraph"),
        );
        if !r.has_focus() && buf.is_empty() {
            r.request_focus();
        }
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() || (r.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))) {
                submit = Some(buf.replace('\n', "\\P"));
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            ui.label(RichText::new("⌘↩ to finish").small());
        });
    });
    ctx.data_mut(|d| d.insert_temp(id, buf));
    if let Some(t) = submit {
        ctx.data_mut(|d| d.remove::<String>(id));
        let _ = app.session.input(cadcraft_engine::Input::Text(t));
    } else if cancel {
        ctx.data_mut(|d| d.remove::<String>(id));
        app.session.cancel();
    }
}
