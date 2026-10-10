//! File menu: new, close, switching drawings. Open/save of DWG/DXF go through the io layer,
//! registered by the host (desktop/CLI) via [`crate::cmd::file::set_io`].

use std::sync::OnceLock;

use cadcraft_doc::{Drawing, Handle};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Plot a space to PDF bytes with JSON options (`{paper?, landscape?, fit?, lineweights?, …}`).
pub type PlotHook = fn(&Drawing, &cadcraft_doc::Space, &Value) -> std::result::Result<Vec<u8>, String>;

/// File format hooks (installed by the app so the engine stays I/O-agnostic and wasm-safe).
pub struct IoHooks {
    pub read: fn(&[u8], &str) -> std::result::Result<Drawing, String>,
    pub write: fn(&Drawing, &str) -> std::result::Result<Vec<u8>, String>,
    /// PLOT / EXPORTPDF (optional: builds without a plotter report "not available").
    pub plot: Option<PlotHook>,
}

static IO: OnceLock<IoHooks> = OnceLock::new();

pub fn set_io(h: IoHooks) {
    let _ = IO.set(h);
}
pub fn io() -> Option<&'static IoHooks> {
    IO.get()
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("new", "New Drawing...", run_new)
            .menu(&["File", "New Drawing..."])
            .key("Cmd+N")
            .alias(&["qnew"])
            .params("{metric?: bool}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("open", "Open...", run_open)
            .menu(&["File", "Open..."])
            .key("Cmd+O")
            .params("{path} | {data: base64, name}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("close", "Close", run_close)
            .menu(&["File", "Close"])
            .key("Cmd+W")
            .params("{index?, force?: discard unsaved changes}")
            .noundo(),
        CommandSpec::new("closeall", "Close All", run_closeall)
            .menu(&["File", "Close All"])
            .params("{force?: discard unsaved changes}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("qsave", "Save", run_save).menu(&["File", "Save"]).key("Cmd+S").alias(&["save"]).params("{path?}").noundo(),
        CommandSpec::new("saveas", "Save As...", run_saveas)
            .menu(&["File", "Save As..."])
            .key("Cmd+Shift+S")
            .params("{path, format?: dxf|dwg}")
            .noundo(),
        CommandSpec::new("document.switch", "Switch Drawing", run_switch).params("{index}").enabled(always).noundo(),
        CommandSpec::new("document.bytes", "Drawing as bytes", run_bytes).params("{format?: dxf} → {data: base64}").noundo(),
        CommandSpec::new("audit", "Audit Drawing", run_audit)
            .menu(&["File", "Drawing Utilities", "Audit"])
            .params("{fix?: bool (default true)} → {errors, fixed, details}"),
        CommandSpec::new("recover", "Recover Drawing...", run_recover)
            .menu(&["File", "Drawing Utilities", "Recover..."])
            .params("{path} | {data: base64, name?}, {fix?: bool} → {index, errors, fixed, details}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("autosave", "Automatic Save", run_autosave)
            .params("{force?: bool, minutes?: elapsed minutes, all?: save every open drawing, path?} → {needed, autosaved, path?|files?}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("recovery.list", "List Recovery Files", run_recovery_list)
            .params("{dir?: default temp dir} → {dir, files: [{name, path, bytes, modified}]}")
            .enabled(always)
            .noundo(),
    ]
}

fn run_new(s: &mut Session, p: &Value) -> Result<Value> {
    let i = s.new_drawing(bool_or(p, "metric", false));
    Ok(json!({ "index": i, "title": s.docs.get(i).map(|d| d.title.clone()) }))
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

pub(crate) fn open_bytes(s: &mut Session, bytes: &[u8], name: &str, path: Option<String>) -> Result<usize> {
    let hooks = io().ok_or_else(|| bad("open", "file formats are not available in this build"))?;
    let d = (hooks.read)(bytes, name).map_err(|e| bad("open", e))?;
    Ok(s.open_drawing(d, &file_name(name), path))
}

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(_path) = str_param(p, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        let path = _path;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let bytes = std::fs::read(path).map_err(|e| bad("open", format!("{path}: {e}")))?;
            let i = open_bytes(s, &bytes, path, Some(path.to_string()))?;
            return Ok(json!({ "index": i, "entities": s.docs.get(i).map(|d| d.doc.entity_count()) }));
        }
        #[cfg(target_arch = "wasm32")]
        return Err(bad("open", "paths are not available on the web; pass `data`"));
    }
    let data = str_param(p, "data").ok_or_else(|| bad("open", "`path` or `data` (base64) is required"))?;
    let bytes = base64_decode(data).ok_or_else(|| bad("open", "invalid base64"))?;
    let name = str_param(p, "name").unwrap_or("Drawing.dxf");
    let i = open_bytes(s, &bytes, name, None)?;
    Ok(json!({ "index": i }))
}

fn run_close(s: &mut Session, p: &Value) -> Result<Value> {
    let i = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(s.active);
    if i >= s.docs.len() {
        return Err(bad("close", "no such drawing"));
    }
    // Programmatic calls never open dialogs: refuse instead of discarding work.
    if !bool_or(p, "force", false) && s.docs.get(i).is_some_and(|d| d.is_dirty()) {
        let title = s.docs.get(i).map(|d| d.title.clone()).unwrap_or_default();
        return Err(bad("close", format!("`{title}` has unsaved changes; save it, or close with force:true")));
    }
    s.cancel();
    s.docs.remove(i);
    if s.active >= s.docs.len() {
        s.active = s.docs.len().saturating_sub(1);
    }
    ok()
}

fn run_closeall(s: &mut Session, p: &Value) -> Result<Value> {
    if !bool_or(p, "force", false) {
        let dirty: Vec<String> = s.docs.iter().filter(|d| d.is_dirty()).map(|d| d.title.clone()).collect();
        if !dirty.is_empty() {
            return Err(bad("closeall", format!("unsaved changes in {}; save them, or close with force:true", dirty.join(", "))));
        }
    }
    s.cancel();
    s.docs.clear();
    s.active = 0;
    ok()
}

pub(crate) fn save_to(s: &mut Session, path: &str) -> Result<(usize, bool)> {
    let hooks = io().ok_or_else(|| bad("save", "file formats are not available in this build"))?;
    let bytes = (hooks.write)(s.doc()?, path).map_err(|e| bad("save", e))?;
    let isavebak = s.settings.isavebak;
    #[cfg(not(target_arch = "wasm32"))]
    let mut backup = false;
    #[cfg(not(target_arch = "wasm32"))]
    {
        if isavebak && std::path::Path::new(path).exists() {
            let bak = sibling_with_ext(path, "bak");
            // A failed backup must never block the save itself; it is reported instead.
            if bak != path && std::fs::copy(path, &bak).is_ok() {
                backup = true;
            }
        }
        let tmp = format!("{path}.cadcraft-tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| bad("save", format!("{path}: {e}")))?;
        std::fs::rename(&tmp, path).map_err(|e| bad("save", format!("{path}: {e}")))?;
    }
    #[cfg(target_arch = "wasm32")]
    let backup = false;
    #[cfg(target_arch = "wasm32")]
    let _ = isavebak;
    let st = s.state_mut()?;
    st.saved = st.doc.clone();
    st.path = Some(path.to_string());
    st.title = file_name(path);
    // A real save makes recovery data redundant.
    st.autosave_rev = st.revision;
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Best-effort cleanup of this drawing's own recovery file (ours end in .sv$).
        if let Some(old) = st.autosave_path.take()
            && old != path
            && old.to_ascii_lowercase().ends_with(".sv$")
        {
            let _ = std::fs::remove_file(old);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        st.autosave_path = None;
    }
    Ok((bytes.len(), backup))
}

fn run_save(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match str_param(p, "path") {
        Some(p) => p.to_string(),
        None => s.state()?.path.clone().ok_or_else(|| bad("qsave", "drawing has no file name yet; use saveas {path}"))?,
    };
    let (n, backup) = save_to(s, &path)?;
    Ok(json!({ "path": path, "bytes": n, "backup": backup }))
}

fn run_saveas(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("saveas", "`path` is required"))?.to_string();
    let (n, backup) = save_to(s, &path)?;
    Ok(json!({ "path": path, "bytes": n, "backup": backup }))
}

fn run_switch(s: &mut Session, p: &Value) -> Result<Value> {
    let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("document.switch", "`index` is required"))? as usize;
    if i >= s.docs.len() {
        return Err(bad("document.switch", "no such drawing"));
    }
    s.cancel();
    s.active = i;
    ok()
}

fn run_bytes(s: &mut Session, p: &Value) -> Result<Value> {
    let hooks = io().ok_or_else(|| bad("document.bytes", "file formats are not available in this build"))?;
    let fmt = str_param(p, "format").unwrap_or("dxf");
    let bytes = (hooks.write)(s.doc()?, &format!("drawing.{fmt}")).map_err(|e| bad("document.bytes", e))?;
    Ok(json!({ "data": base64_encode(&bytes), "bytes": bytes.len() }))
}

// ---------------- file safety: backup, autosave, audit, recover ----------------

/// Replace (or add) the extension of a path or file name: `plan.dxf` + `sv$` → `plan.sv$`.
fn sibling_with_ext(base: &str, ext: &str) -> String {
    let t = base.trim();
    if t.is_empty() {
        return format!("Drawing.{ext}");
    }
    let sep = t.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let (dir, file) = t.split_at(sep);
    let stem = match file.rfind('.') {
        Some(i) if i > 0 => &file[..i],
        _ => file,
    };
    let stem = if stem.is_empty() { "Drawing" } else { stem };
    // Truncate hostile lengths without splitting a UTF-8 character.
    let stem: String = stem.chars().take(200).collect();
    format!("{dir}{stem}.{ext}")
}

/// Suggested automatic-save file for a drawing path or title (AutoCAD's `.sv$` convention).
pub fn recovery_name(base: &str) -> String {
    sibling_with_ext(base, "sv$")
}

/// True when the host should run an automatic save now (SAVETIME in minutes, 0 = off).
/// The host owns the wall clock and passes elapsed minutes; `force` always saves.
pub(crate) fn autosave_due(savetime: f64, minutes: f64, force: bool) -> bool {
    force || (savetime > 0.0 && minutes >= savetime)
}

fn recovery_path_for(st: &crate::DocState) -> String {
    if let Some(p) = &st.path {
        return recovery_name(p);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let name = recovery_name(&st.title);
        std::env::temp_dir().join(name).to_string_lossy().to_string()
    }
    #[cfg(target_arch = "wasm32")]
    return recovery_name(&st.title);
}

fn run_autosave(s: &mut Session, p: &Value) -> Result<Value> {
    let savetime = s.settings.savetime;
    let force = bool_or(p, "force", false);
    let minutes = p.get("minutes").and_then(Value::as_f64).filter(|f| f.is_finite()).map(|f| f.max(0.0)).unwrap_or(0.0);
    let all = bool_or(p, "all", false);
    // Recovery tracks the revision, not the file: a QSAVE also marks recovery current.
    // `all` walks every open drawing without disturbing the active one or the selection.
    let targets: Vec<usize> = if all {
        s.docs.iter().enumerate().filter(|(_, st)| st.revision != st.autosave_rev).map(|(i, _)| i).collect()
    } else {
        s.state().map(|st| if st.revision != st.autosave_rev { vec![s.active] } else { Vec::new() }).unwrap_or_default()
    };
    if targets.is_empty() {
        return Ok(json!({ "needed": false, "autosaved": false, "message": "No changes since the last save." }));
    }
    if !autosave_due(savetime, minutes, force) {
        let msg = if savetime <= 0.0 {
            "SAVETIME is 0 (automatic save is off).".to_string()
        } else {
            format!("Next automatic save in {:.1} minute(s) (SAVETIME {savetime}).", (savetime - minutes).max(0.0))
        };
        return Ok(json!({ "needed": false, "autosaved": false, "savetime": savetime, "message": msg }));
    }
    if all && str_param(p, "path").is_some() {
        return Err(bad("autosave", "`path` needs a single drawing; omit it with `all`"));
    }
    let hooks = io().ok_or_else(|| bad("autosave", "file formats are not available in this build"))?;
    let explicit = str_param(p, "path").map(str::to_string);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut files = Vec::new();
        for i in targets {
            let dest = match &explicit {
                Some(x) => x.clone(),
                None => {
                    let st = s.docs.get(i).ok_or_else(|| bad("autosave", "drawing closed"))?;
                    recovery_path_for(st)
                }
            };
            let bytes = {
                let st = s.docs.get(i).ok_or_else(|| bad("autosave", "drawing closed"))?;
                (hooks.write)(&st.doc, "drawing.dxf").map_err(|e| bad("autosave", e))?
            };
            let tmp = format!("{dest}.cadcraft-tmp");
            std::fs::write(&tmp, &bytes).map_err(|e| bad("autosave", format!("{dest}: {e}")))?;
            std::fs::rename(&tmp, &dest).map_err(|e| bad("autosave", format!("{dest}: {e}")))?;
            let st = s.docs.get_mut(i).ok_or_else(|| bad("autosave", "drawing closed"))?;
            st.autosave_rev = st.revision;
            st.autosave_path = Some(dest.clone());
            files.push(json!({ "index": i, "title": st.title, "path": dest, "bytes": bytes.len() }));
        }
        if !all {
            let f = files.into_iter().next().ok_or_else(|| bad("autosave", "nothing saved"))?;
            return Ok(json!({ "needed": true, "autosaved": true, "path": f["path"], "bytes": f["bytes"],
                "message": format!("Autosaved {} byte(s) to {}.", f["bytes"], f["path"].as_str().unwrap_or("")) }));
        }
        let n = files.len();
        Ok(json!({ "needed": true, "autosaved": true, "files": files, "message": format!("Autosaved {n} drawing(s).") }))
    }
    #[cfg(target_arch = "wasm32")]
    {
        if explicit.is_some() {
            return Err(bad("autosave", "paths are not available on the web; omit `path` to receive `data`"));
        }
        let mut files = Vec::new();
        for i in targets {
            let st = s.docs.get(i).ok_or_else(|| bad("autosave", "drawing closed"))?;
            let bytes = (hooks.write)(&st.doc, "drawing.dxf").map_err(|e| bad("autosave", e))?;
            files.push(
                json!({ "index": i, "title": st.title, "suggestion": recovery_path_for(st), "data": base64_encode(&bytes), "bytes": bytes.len() }),
            );
            let st = s.docs.get_mut(i).ok_or_else(|| bad("autosave", "drawing closed"))?;
            st.autosave_rev = st.revision;
        }
        if !all {
            let f = files.into_iter().next().ok_or_else(|| bad("autosave", "nothing saved"))?;
            return Ok(json!({ "needed": true, "autosaved": true, "suggestion": f["suggestion"], "data": f["data"], "bytes": f["bytes"] }));
        }
        let n = files.len();
        Ok(json!({ "needed": true, "autosaved": true, "files": files, "message": format!("Autosaved {n} drawing(s).") }))
    }
}

fn run_recovery_list(s: &mut Session, p: &Value) -> Result<Value> {
    let _ = s;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = p;
        return Ok(json!({ "dir": "", "files": [], "message": "Recovery files are not available on the web." }));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if p.get("dir").is_some_and(|v| !v.is_string()) {
            return Err(bad("recovery.list", "`dir` must be a path string"));
        }
        let dir = str_param(p, "dir").map(str::to_string).unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
        let rd = std::fs::read_dir(&dir).map_err(|e| {
            if str_param(p, "dir").is_some() { bad("recovery.list", format!("{dir}: {e}")) } else { bad("recovery.list", e.to_string()) }
        })?;
        let mut files = Vec::new();
        for e in rd.take(1001).flatten() {
            let path = e.path();
            let is_sv = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("sv$"));
            if !is_sv {
                continue;
            }
            let md = e.metadata().ok();
            let bytes = md.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified =
                md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            files.push(json!({ "name": path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), "path": path.to_string_lossy(), "bytes": bytes, "modified": modified }));
            if files.len() >= 1000 {
                break;
            }
        }
        files.sort_by(|a, b| b["modified"].as_u64().cmp(&a["modified"].as_u64()));
        let n = files.len();
        Ok(json!({ "dir": dir, "files": files, "message": format!("{n} recovery file(s) in {dir}.") }))
    }
}

/// Where an audited entity lives (layouts and block definitions hold stores too).
#[derive(Clone, Debug)]
enum StoreLoc {
    Model,
    Layout(usize),
    Block(String),
}

impl StoreLoc {
    fn label(&self, d: &Drawing) -> String {
        match self {
            StoreLoc::Model => "model space".into(),
            StoreLoc::Layout(i) => d.layouts.get(*i).map(|l| format!("layout {}", l.name)).unwrap_or_else(|| "a layout".into()),
            StoreLoc::Block(n) => format!("block {n}"),
        }
    }
}

#[derive(Default)]
pub(crate) struct AuditReport {
    pub errors: usize,
    pub fixed: usize,
    pub details: Vec<String>,
}

/// Cap on returned detail lines (counting continues past it).
const MAX_AUDIT_DETAILS: usize = 200;

fn note(report: &mut AuditReport, fixed: bool, msg: String) {
    report.errors += 1;
    if fixed {
        report.fixed += 1;
    }
    if report.details.len() < MAX_AUDIT_DETAILS {
        report.details.push(msg);
    }
}

fn is_standard_linetype(name: &str) -> bool {
    matches!(name.trim().to_ascii_lowercase().as_str(), "bylayer" | "byblock" | "continuous" | "")
}

#[derive(Clone, Debug)]
struct Issue {
    loc: StoreLoc,
    handle: Handle,
    type_name: String,
    bad_layer: Option<String>,
    bad_linetype: Option<String>,
    non_finite: bool,
}

/// Scan model space, layouts and block definitions for corrupt or dangling data.
/// Read-only: a clean scan never dirties the drawing (no copy-on-write).
fn collect_issues(d: &Drawing) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let mut locs: Vec<StoreLoc> = vec![StoreLoc::Model];
    locs.extend((0..d.layouts.len()).map(StoreLoc::Layout));
    locs.extend(d.blocks.keys().cloned().map(StoreLoc::Block));
    for loc in &locs {
        let store = match loc {
            StoreLoc::Model => &d.model,
            StoreLoc::Layout(i) => match d.layouts.get(*i) {
                Some(l) => &l.entities,
                None => continue,
            },
            StoreLoc::Block(n) => match d.blocks.get(n) {
                Some(b) => &b.entities,
                None => continue,
            },
        };
        // Cap the work per store so a hostile file can't stall the command line.
        for e in store.iter().take(2_000_000) {
            let c = &e.common;
            let common_ok = c.ltscale.is_finite()
                && c.thickness.is_finite()
                && c.extrusion.x.is_finite()
                && c.extrusion.y.is_finite()
                && c.extrusion.z.is_finite();
            let non_finite = !common_ok || !e.kind.is_finite();
            if non_finite {
                issues.push(Issue {
                    loc: loc.clone(),
                    handle: e.handle,
                    type_name: e.kind.type_name().to_string(),
                    bad_layer: None,
                    bad_linetype: None,
                    non_finite: true,
                });
                continue;
            }
            let blank_layer = e.common.layer.trim().is_empty();
            let want_layer = if blank_layer { "0" } else { e.common.layer.as_str() };
            let bad_layer = if blank_layer || d.layer(want_layer).is_none() { Some(want_layer.to_string()) } else { None };
            let bad_linetype = if !is_standard_linetype(&e.common.linetype) && d.linetype(&e.common.linetype).is_none() {
                Some(e.common.linetype.clone())
            } else {
                None
            };
            if bad_layer.is_some() || bad_linetype.is_some() {
                issues.push(Issue {
                    loc: loc.clone(),
                    handle: e.handle,
                    type_name: e.kind.type_name().to_string(),
                    bad_layer,
                    bad_linetype,
                    non_finite: false,
                });
            }
        }
    }
    issues
}

/// Apply collected repairs. Callers must only take `&mut` when `issues` is
/// non-empty: borrowing mutably dirties the drawing even with no changes.
fn apply_fixes(d: &mut Drawing, issues: &[Issue]) -> AuditReport {
    let mut report = AuditReport::default();
    let mut new_layers: Vec<String> = Vec::new();
    for is in issues {
        if is.non_finite {
            let label = is.loc.label(d);
            let removed = match &is.loc {
                StoreLoc::Model => d.model.remove(is.handle).is_some(),
                StoreLoc::Layout(i) => d.layouts.get_mut(*i).and_then(|l| l.entities.remove(is.handle)).is_some(),
                StoreLoc::Block(n) => d.blocks.get_mut(n).map(|b| std::sync::Arc::make_mut(b).entities.remove(is.handle).is_some()).unwrap_or(false),
            };
            note(&mut report, removed, format!("{} {} in {}: non-finite geometry removed.", is.type_name, is.handle.hex(), label));
            continue;
        }
        if let Some(layer) = &is.bad_layer {
            let label = is.loc.label(d);
            if !new_layers.iter().any(|l| l.eq_ignore_ascii_case(layer)) {
                new_layers.push(layer.clone());
            }
            let ok = match &is.loc {
                StoreLoc::Model | StoreLoc::Layout(_) => d.modify_entity(is.handle, |e| e.common.layer = layer.clone()).is_ok(),
                StoreLoc::Block(n) => d
                    .blocks
                    .get_mut(n)
                    .map(|b| std::sync::Arc::make_mut(b).entities.modify(is.handle, |e| e.common.layer = layer.clone()))
                    .unwrap_or(false),
            };
            note(&mut report, ok, format!("{} {} in {}: layer `{layer}` repaired.", is.type_name, is.handle.hex(), label));
        }
        if let Some(lt) = &is.bad_linetype {
            let label = is.loc.label(d);
            let ok = match &is.loc {
                StoreLoc::Model | StoreLoc::Layout(_) => d.modify_entity(is.handle, |e| e.common.linetype = "ByLayer".into()).is_ok(),
                StoreLoc::Block(n) => d
                    .blocks
                    .get_mut(n)
                    .map(|b| std::sync::Arc::make_mut(b).entities.modify(is.handle, |e| e.common.linetype = "ByLayer".into()))
                    .unwrap_or(false),
            };
            note(&mut report, ok, format!("{} {} in {}: linetype `{lt}` reset to ByLayer.", is.type_name, is.handle.hex(), label));
        }
    }
    for layer in new_layers {
        d.ensure_layer(&layer);
    }
    report
}

/// Report collected issues without touching the drawing.
fn report_issues(d: &Drawing, issues: &[Issue]) -> AuditReport {
    let mut report = AuditReport::default();
    for is in issues {
        let label = is.loc.label(d);
        if is.non_finite {
            note(&mut report, false, format!("{} {} in {}: non-finite geometry.", is.type_name, is.handle.hex(), label));
            continue;
        }
        if let Some(layer) = &is.bad_layer {
            note(&mut report, false, format!("{} {} in {}: missing layer `{layer}`.", is.type_name, is.handle.hex(), label));
        }
        if let Some(lt) = &is.bad_linetype {
            note(&mut report, false, format!("{} {} in {}: missing linetype `{lt}`.", is.type_name, is.handle.hex(), label));
        }
    }
    report
}

fn audit_report_value(report: &AuditReport) -> Value {
    let msg = format!("Audit found {} error(s), fixed {}.", report.errors, report.fixed);
    json!({ "errors": report.errors, "fixed": report.fixed, "details": report.details, "message": msg })
}

fn run_audit(s: &mut Session, p: &Value) -> Result<Value> {
    let fix = bool_or(p, "fix", true);
    // Scan read-only first: a clean audit must not dirty the drawing, and an
    // empty issue list needs no mutable borrow at all.
    let issues = collect_issues(s.doc()?);
    let report = if fix && !issues.is_empty() { apply_fixes(s.doc_mut()?, &issues) } else { report_issues(s.doc()?, &issues) };
    if fix {
        // Drop selection handles whose entities were removed by the repair.
        let stale: Vec<Handle> = {
            let d = s.doc()?;
            s.selection().into_iter().filter(|h| d.entity(*h).is_none()).collect()
        };
        if !stale.is_empty() {
            let kept: Vec<Handle> = s.selection().into_iter().filter(|h| !stale.contains(h)).collect();
            s.set_selection(kept);
        }
    }
    Ok(audit_report_value(&report))
}

fn run_recover(s: &mut Session, p: &Value) -> Result<Value> {
    let fix = bool_or(p, "fix", true);
    let (bytes, name, source) = if let Some(path) = str_param(p, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let b = std::fs::read(path).map_err(|e| bad("recover", format!("{path}: {e}")))?;
            (b, path.to_string(), Some(path.to_string()))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            return Err(bad("recover", "paths are not available on the web; pass `data`"));
        }
    } else {
        let data = str_param(p, "data").ok_or_else(|| bad("recover", "`path` or `data` (base64) is required"))?;
        let bytes = base64_decode(data).ok_or_else(|| bad("recover", "invalid base64"))?;
        let name = str_param(p, "name").unwrap_or("Recovered.dxf").to_string();
        (bytes, name, None::<String>)
    };
    if bytes.len() > 512_000_000 {
        return Err(bad("recover", "file is too large to recover"));
    }
    let i = open_bytes(s, &bytes, &name, None)?;
    // A recovery file is consumed by opening it: the next real save removes it.
    // Only `.sv$` files are tagged (never a user's own drawing).
    #[cfg(not(target_arch = "wasm32"))]
    if let (Some(src), Some(st)) = (source.filter(|p| p.to_ascii_lowercase().ends_with(".sv$")), s.docs.get_mut(i)) {
        st.autosave_path = Some(src);
    }
    // A fresh import is clean; only borrow mutably when repairs are needed.
    let issues = collect_issues(s.doc()?);
    let report = if fix && !issues.is_empty() { apply_fixes(s.doc_mut()?, &issues) } else { report_issues(s.doc()?, &issues) };
    let entities = s.docs.get(i).map(|d| d.doc.entity_count()).unwrap_or(0);
    let mut v = audit_report_value(&report);
    v["index"] = json!(i);
    v["entities"] = json!(entities);
    Ok(v)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_doc::{Entity, EntityKind, Line};
    use cadcraft_geom::Vec3;

    static TEST_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn test_session() -> Session {
        // Unique titles: derived recovery paths land in the shared temp dir,
        // so default Drawing1/Drawing2 names would collide across tests.
        let mut s = Session::new();
        let n = TEST_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(st) = s.docs.get_mut(0) {
            st.title = format!("test-{}-{n}", std::process::id());
        }
        s
    }

    /// Test IO hooks (superset of the ones in `layout.rs` tests: same plot format so
    /// whichever test installs first, both suites pass; the loser skips via probe).
    fn install_fake_io() {
        set_io(IoHooks {
            read: |_, _| Err("no reader in tests".into()),
            write: |_, name| Ok(format!("fake {name}").into_bytes()),
            plot: Some(|_, space, opts| Ok(format!("%PDF-1.4 fake {space:?} {opts}").into_bytes())),
        });
    }

    fn writer_available(s: &mut Session) -> bool {
        s.execute("document.bytes", &json!({})).is_ok()
    }

    fn nan_line(handle: Handle) -> Entity {
        let nan = Vec3::new(f64::NAN, 1.0, 0.0);
        Entity { handle, common: Default::default(), kind: EntityKind::Line(Line { a: nan, b: Vec3::new(1.0, 2.0, 0.0) }) }
    }

    #[test]
    fn file_safety_sysvars() {
        let mut s = test_session();
        assert_eq!(crate::sysvars::get(&s, "savetime"), Some(json!(10.0)));
        assert_eq!(crate::sysvars::get(&s, "isavebak"), Some(json!(1)));
        let savefile = crate::sysvars::get(&s, "SAVEFILE").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        assert!(savefile.ends_with(".sv$"), "{savefile}");
        s.execute("setvar", &json!({"name": "SAVETIME", "value": 30})).unwrap();
        assert_eq!(crate::sysvars::get(&s, "savetime"), Some(json!(30.0)));
        // Clamped to a day; negatives clamp to 0 (off).
        s.execute("setvar", &json!({"name": "SAVETIME", "value": 99999})).unwrap();
        assert_eq!(crate::sysvars::get(&s, "savetime"), Some(json!(1440.0)));
        s.execute("setvar", &json!({"name": "SAVETIME", "value": -5})).unwrap();
        assert_eq!(crate::sysvars::get(&s, "savetime"), Some(json!(0.0)));
        assert!(s.execute("setvar", &json!({"name": "SAVETIME", "value": "soon"})).is_err());
        s.execute("setvar", &json!({"name": "ISAVEBAK", "value": 0})).unwrap();
        assert_eq!(crate::sysvars::get(&s, "isavebak"), Some(json!(0)));
        assert!(s.execute("setvar", &json!({"name": "SAVEFILE", "value": "x"})).is_err(), "SAVEFILE is read-only");
    }

    #[test]
    fn recovery_names() {
        assert_eq!(recovery_name("plan.dxf"), "plan.sv$");
        assert_eq!(recovery_name("/tmp/plan.DWG"), "/tmp/plan.sv$");
        assert_eq!(recovery_name("Drawing1"), "Drawing1.sv$");
        assert_eq!(recovery_name(""), "Drawing.sv$");
        assert_eq!(sibling_with_ext("a.dxf", "bak"), "a.bak");
        assert!(!recovery_name(&"x".repeat(500)).contains(".dxf"));
    }

    #[test]
    fn clean_audit_never_dirties() {
        // Regression: taking doc_mut() on a clean drawing clones the Arc and
        // falsely marks it dirty (undo step, DBMOD, spurious autosaves).
        let mut s = test_session();
        let (rev0, undo0) = (s.state().unwrap().revision, s.state().unwrap().undo.len());
        let r = s.execute("audit", &json!({})).unwrap();
        assert_eq!(r["errors"], 0);
        assert_eq!(r["fixed"], 0);
        let st = s.state().unwrap();
        assert!(!st.is_dirty(), "clean audit must not dirty the drawing");
        assert_eq!(st.revision, rev0, "no revision bump");
        assert_eq!(st.undo.len(), undo0, "no undo step");
        let r = s.execute("audit", &json!({"fix": false})).unwrap();
        assert_eq!(r["errors"], 0);
        assert_eq!(r["fixed"], 0);
        assert!(!s.state().unwrap().is_dirty());
    }

    #[test]
    fn audit_fixes_problems() {
        // The audit flags non-finite geometry in any entity variant.
        assert!(!nan_line(Handle(1)).kind.is_finite());
        assert!(Entity::new(Handle(1), EntityKind::Point(cadcraft_doc::Point { p: Vec3::ZERO, angle: 0.0 })).kind.is_finite());
        let mut s = test_session();
        let h1 = s.doc_mut().unwrap().new_handle();
        let mut e1 = Entity::new(h1, EntityKind::Point(cadcraft_doc::Point { p: Vec3::new(1.0, 2.0, 0.0), angle: 0.0 }));
        e1.common.layer = "Missing_Layer_Xyz".into();
        s.doc_mut().unwrap().model.push(e1);
        let h2 = s.doc_mut().unwrap().new_handle();
        s.doc_mut().unwrap().model.push(nan_line(h2));
        // Report-only: finds both, changes nothing.
        let r = s.execute("audit", &json!({"fix": false})).unwrap();
        assert_eq!(r["errors"], 2);
        assert_eq!(r["fixed"], 0);
        assert!(s.doc().unwrap().entity(h2).is_some());
        // Fix: creates the layer, removes the NaN line.
        let r = s.execute("audit", &json!({})).unwrap();
        assert_eq!(r["errors"], 2);
        assert_eq!(r["fixed"], 2);
        assert!(s.doc().unwrap().layer("Missing_Layer_Xyz").is_some());
        assert!(s.doc().unwrap().entity(h2).is_none());
        assert!(s.doc().unwrap().entity(h1).is_some());
        let r = s.execute("audit", &json!({})).unwrap();
        assert_eq!(r["errors"], 0);
    }

    #[test]
    fn audit_repairs_linetype_and_empty_layer() {
        let mut s = test_session();
        let h = s.doc_mut().unwrap().new_handle();
        let mut e = Entity::new(h, EntityKind::Point(cadcraft_doc::Point { p: Vec3::ZERO, angle: 0.0 }));
        e.common.layer = "   ".into();
        e.common.linetype = "Missing_LT".into();
        s.doc_mut().unwrap().model.push(e);
        let r = s.execute("audit", &json!({})).unwrap();
        assert_eq!(r["errors"], 2);
        assert_eq!(r["fixed"], 2);
        let e = s.doc().unwrap().entity(h).unwrap();
        assert_eq!(e.common.layer, "0");
        assert_eq!(e.common.linetype, "ByLayer");
    }

    #[test]
    fn autosave_due_logic() {
        assert!(autosave_due(10.0, 10.0, false));
        assert!(autosave_due(10.0, 60.0, false));
        assert!(!autosave_due(10.0, 9.9, false));
        assert!(!autosave_due(0.0, 1000.0, false));
        assert!(autosave_due(0.0, 0.0, true));
    }

    #[test]
    fn autosave_roundtrip() {
        install_fake_io();
        let mut s = test_session();
        if !writer_available(&mut s) {
            return; // another test installed hooks without a writer first
        }
        // Clean drawing: nothing to do, even forced.
        let r = s.execute("autosave", &json!({"force": true})).unwrap();
        assert_eq!(r["needed"], false);
        s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
        // Not due yet.
        let r = s.execute("autosave", &json!({})).unwrap();
        assert_eq!(r["needed"], false);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dir = std::env::temp_dir().join(format!("cadcraft-autosave-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let dest = dir.join("auto.sv$").to_string_lossy().to_string();
            let r = s.execute("autosave", &json!({"minutes": 10.0, "path": dest})).unwrap();
            assert_eq!(r["autosaved"], true);
            assert!(std::fs::read(&dest).unwrap().starts_with(b"fake"));
            // Recovery is current now: a second call has nothing to do.
            let r = s.execute("autosave", &json!({"minutes": 100.0})).unwrap();
            assert_eq!(r["needed"], false);
            // SAVETIME 0 disables the timer but not an explicit force.
            s.execute("setvar", &json!({"name": "SAVETIME", "value": 0})).unwrap();
            s.execute("line", &json!({"points": [[0, 1], [5, 1]]})).unwrap();
            let r = s.execute("autosave", &json!({"minutes": 1000.0})).unwrap();
            assert_eq!(r["needed"], false);
            let r = s.execute("autosave", &json!({"force": true, "path": dest})).unwrap();
            assert_eq!(r["autosaved"], true);
            let _ = std::fs::remove_dir_all(&dir);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let r = s.execute("autosave", &json!({"minutes": 10.0})).unwrap();
            assert_eq!(r["autosaved"], true);
            assert!(r["suggestion"].as_str().unwrap_or("").ends_with(".sv$"));
            assert!(!r["data"].as_str().unwrap_or("").is_empty());
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn save_creates_bak() {
        install_fake_io();
        let mut s = test_session();
        if !writer_available(&mut s) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cadcraft-bak-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("plan.dxf").to_string_lossy().to_string();
        std::fs::write(&path, b"old").unwrap();
        s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
        let r = s.execute("saveas", &json!({"path": path})).unwrap();
        assert_eq!(r["backup"], true);
        assert_eq!(std::fs::read(dir.join("plan.bak")).unwrap(), b"old");
        // ISAVEBAK 0: overwrite without a backup.
        s.execute("setvar", &json!({"name": "ISAVEBAK", "value": 0})).unwrap();
        let path2 = dir.join("nobak.dxf").to_string_lossy().to_string();
        std::fs::write(&path2, b"old").unwrap();
        let r = s.execute("saveas", &json!({"path": path2})).unwrap();
        assert_eq!(r["backup"], false);
        assert!(!dir.join("nobak.bak").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hostile_file_safety_params() {
        let mut s = test_session();
        let long = "x".repeat(5000);
        for (c, p) in [
            ("audit", json!({"fix": "yes"})),
            ("audit", json!({"fix": [1, 2]})),
            ("autosave", json!({"force": "now"})),
            ("autosave", json!({"minutes": "soon"})),
            ("autosave", json!({"minutes": 1e308})),
            ("autosave", json!({"path": long})),
            ("autosave", json!({"all": "every"})),
            ("autosave", json!({"all": true, "path": "x.sv$"})),
            ("recovery.list", json!(null)),
            ("recovery.list", json!({"dir": 42})),
            ("recovery.list", json!({"dir": "/nonexistent-dir-xyz"})),
            ("recover", json!(null)),
            ("recover", json!({"data": "!!!not-base64!!!"})),
            ("recover", json!({"fix": false})),
            ("saveas", json!({})),
            ("saveas", json!({"path": 42})),
            ("saveas", json!({"path": long})),
        ] {
            let _ = s.execute(c, &p);
        }
        // The session still works afterwards.
        assert!(s.execute("audit", &json!({})).is_ok());
    }

    #[test]
    fn close_refuses_dirty_drawings_without_force() {
        let mut s = test_session();
        // Clean drawings close freely.
        s.execute("close", &json!({})).unwrap();
        assert!(s.docs.is_empty());
        s.execute("new", &json!({})).unwrap();
        s.execute("closeall", &json!({})).unwrap();
        assert!(s.docs.is_empty());
        // Dirty drawings are refused, never silently discarded.
        s.execute("new", &json!({})).unwrap();
        s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
        let err = s.execute("close", &json!({})).unwrap_err().to_string();
        assert!(err.contains("unsaved changes"), "{err}");
        assert_eq!(s.docs.len(), 1);
        s.execute("close", &json!({"force": true})).unwrap();
        assert!(s.docs.is_empty());
        // closeall names every dirty drawing.
        s.execute("new", &json!({})).unwrap();
        s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
        s.execute("new", &json!({})).unwrap();
        let err = s.execute("closeall", &json!({})).unwrap_err().to_string();
        assert!(err.contains("unsaved changes"), "{err}");
        assert_eq!(s.docs.len(), 2);
        s.execute("closeall", &json!({"force": true})).unwrap();
        assert!(s.docs.is_empty());
        // Hostile index values are clean errors.
        assert!(s.execute("close", &json!({"index": 99})).is_err());
        assert!(s.execute("close", &json!({"index": "two"})).is_err());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn autosave_all_saves_every_dirty_drawing() {
        install_fake_io();
        let mut s = test_session();
        if !writer_available(&mut s) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cadcraft-allas-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // Unique titles: derived recovery paths land in the shared temp dir.
        for (i, st) in s.docs.iter_mut().enumerate() {
            st.title = format!("allas-{}-{i}", std::process::id());
        }
        // Two drawings with real paths (unique temp dir: no collisions).
        for name in ["one.dxf", "two.dxf"] {
            let path = dir.join(name).to_string_lossy().to_string();
            s.execute("saveas", &json!({"path": path})).unwrap();
            s.execute("new", &json!({})).unwrap();
        }
        // Dirty both drawings: active is the newest; switch to dirty the other too.
        s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
        s.execute("document.switch", &json!({"index": 0})).unwrap();
        s.execute("line", &json!({"points": [[2, 2], [3, 3]]})).unwrap();
        assert_eq!(s.active, 0, "active drawing must not move");
        let r = s.execute("autosave", &json!({"force": true, "all": true})).unwrap();
        assert_eq!(r["autosaved"], true);
        // Only the two drawings with real paths were dirty (the seed drawing is clean).
        let files = r["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(s.active, 0, "active drawing still untouched");
        for f in files {
            assert!(std::fs::read(f["path"].as_str().unwrap()).unwrap().starts_with(b"fake"));
        }
        // A second sweep has nothing to do.
        let r = s.execute("autosave", &json!({"force": true, "all": true})).unwrap();
        assert_eq!(r["needed"], false);
        // The untitled drawing autosaved to the shared temp dir: clean it up.
        let _ = std::fs::remove_file(std::env::temp_dir().join("Drawing3.sv$"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn save_removes_its_recovery_file() {
        install_fake_io();
        let mut s = test_session();
        if !writer_available(&mut s) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cadcraft-svdel-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("plan.dxf").to_string_lossy().to_string();
        s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
        s.execute("saveas", &json!({"path": path})).unwrap();
        s.execute("line", &json!({"points": [[5, 5], [6, 6]]})).unwrap();
        let sv = dir.join("plan.sv$").to_string_lossy().to_string();
        s.execute("autosave", &json!({"force": true})).unwrap();
        assert!(std::path::Path::new(&sv).exists());
        s.execute("qsave", &json!({})).unwrap();
        assert!(!std::path::Path::new(&sv).exists(), "real save clears recovery data");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn recovery_list_finds_sv_files() {
        let mut s = test_session();
        let dir = std::env::temp_dir().join(format!("cadcraft-reclist-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("a.sv$"), b"1").unwrap();
        std::fs::write(dir.join("b.SV$"), b"12").unwrap();
        std::fs::write(dir.join("notes.txt"), b"zzz").unwrap();
        let ds = dir.to_string_lossy().to_string();
        let r = s.execute("recovery.list", &json!({"dir": ds})).unwrap();
        let files = r["files"].as_array().unwrap();
        assert_eq!(files.len(), 2, "only .sv$ files, any case");
        assert_eq!(r["dir"], ds.as_str());
        assert!(s.execute("recovery.list", &json!({"dir": "/nonexistent-dir-xyz"})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
