use super::*;
use std::fs;

fn dialog(window: &gtk::ApplicationWindow, title: &str) -> (gtk::Window, gtk::Box) {
    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(window)
        .modal(true)
        .default_width(720)
        .default_height(480)
        .build();
    dialog.add_css_class("qfind-dialog");
    let header = gtk::HeaderBar::new();
    header.add_css_class("qfind-shell");
    let title = gtk::Label::new(Some(title));
    title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    header.set_title_widget(Some(&title));
    dialog.set_titlebar(Some(&header));
    let keys = gtk::EventControllerKey::new();
    let weak = dialog.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            if let Some(dialog) = weak.upgrade() {
                dialog.close();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    dialog.add_controller(keys);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    for set in [
        gtk::Box::set_margin_top,
        gtk::Box::set_margin_bottom,
        gtk::Box::set_margin_start,
        gtk::Box::set_margin_end,
    ] {
        set(&body, 16);
    }
    dialog.set_child(Some(&body));
    (dialog, body)
}

fn text_view(body: &gtk::Box) -> gtk::TextBuffer {
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .vexpand(true)
        .build();
    let buffer = view.buffer();
    body.append(
        &gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .build(),
    );
    buffer
}

fn field(body: &gtk::Box, title: &str, initial: &str) -> gtk::Entry {
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    body.append(&label);
    let entry = gtk::Entry::builder().text(initial).build();
    body.append(&entry);
    entry
}

fn job(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    title: &str,
    work: impl FnOnce() -> Result<String, String> + Send + 'static,
) {
    let (dialog, body) = dialog(window, title);
    let output = text_view(&body);
    output.set_text(
        "Working… You can keep browsing. Closing this window does not cancel the operation.",
    );
    dialog.set_modal(false);
    dialog.present();
    let state = state.clone();
    let window = window.clone();
    glib::MainContext::default().spawn_local(async move {
        let result = gio::spawn_blocking(work).await;
        let message = match result {
            Ok(Ok(message)) => message,
            Ok(Err(error)) => error,
            Err(_) => {
                "Operation interrupted unexpectedly; check the destination before retrying.".into()
            }
        };
        output.set_text(&message);
        refresh_current(&state, &window);
    });
}

pub(super) fn install(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    target: &ActionTarget,
) {
    for name in [
        "select-matching",
        "batch-rename",
        "batch-copy",
        "batch-move",
        "batch-zip",
        "batch-extract",
    ] {
        let action = gio::SimpleAction::new(name, None);
        window.add_action(&action);
        let window = window.clone();
        let state = state.clone();
        let target = target.clone();
        action.connect_activate(move |_, _| {
            if name == "select-matching" {
                select_matching(&window, &state);

            } else {
                let rows = target.rows();
                if rows.is_empty() {
                    state
                        .borrow()
                        .status
                        .set_text("Select files or folders first");
                } else if name == "batch-rename" {
                    rename(&window, &state, rows);
                } else {
                    transfer(&window, &state, rows, name);
                }
            }
        });
    }
}

fn select_matching(window: &gtk::ApplicationWindow, state: &Rc<RefCell<State>>) {
    let (dialog, body) = dialog(window, "Select matching files");
    dialog.set_default_size(560, 300);
    let name = field(&body, "Name contains (case insensitive)", "");
    let extensions = field(
        &body,
        "Extensions, separated by commas (for example rs, toml)",
        "",
    );
    let kind = gtk::DropDown::from_strings(&[
        "Files and folders",
        "Files only",
        "Folders only",
        "Archives",
        "Images",
        "Audio",
        "Video",
    ]);
    body.append(&kind);
    let summary = gtk::Label::new(None);
    body.append(&summary);
    let select = gtk::Button::with_label("Replace selection");
    select.add_css_class("suggested-action");
    body.append(&select);
    let evaluate: Rc<dyn Fn(bool)> = {
        let state = state.clone();
        let name = name.clone();
        let extensions = extensions.clone();
        let kind = kind.clone();
        let summary = summary.clone();
        Rc::new(move |apply| {
            let st = state.borrow();
            let needle = name.text().to_lowercase();
            let ext_text = extensions.text().to_lowercase();
            let exts: Vec<_> = ext_text
                .split(',')
                .map(|s| s.trim().trim_start_matches('.'))
                .filter(|s| !s.is_empty())
                .collect();
            let matches = gtk::Bitset::new_empty();
            let mut count = 0;
            for i in 0..st.model.n_items() {
                let Some(row) = st.model.item(i).and_downcast::<RowData>() else {
                    continue;
                };
                let filename = row.name().to_lowercase();
                let extension = Path::new(&filename)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                let class = match kind.selected() {
                    1 => !row.is_dir(),
                    2 => row.is_dir(),
                    3 => FileClass::Archive.matches(&filename, row.is_dir()),
                    4 => FileClass::Image.matches(&filename, row.is_dir()),
                    5 => FileClass::Audio.matches(&filename, row.is_dir()),
                    6 => FileClass::Video.matches(&filename, row.is_dir()),
                    _ => true,
                };
                if class
                    && filename.contains(&needle)
                    && (exts.is_empty() || (!row.is_dir() && exts.contains(&extension)))
                {
                    count += 1;
                    matches.add(i);
                }
            }
            if apply {
                st.selection
                    .set_selection(&matches, &gtk::Bitset::new_range(0, st.model.n_items()));
            }
            summary.set_text(&format!("{count} matches in the currently loaded results"));
        })
    };
    for entry in [&name, &extensions] {
        let evaluate = evaluate.clone();
        entry.connect_changed(move |_| evaluate(false));
    }
    let update = evaluate.clone();
    kind.connect_selected_notify(move |_| update(false));
    evaluate(false);
    let close = dialog.clone();
    select.connect_clicked(move |_| {
        evaluate(true);
        close.close();
    });
    dialog.present();
}

fn rename_pairs(
    paths: &[PathBuf],
    find: &str,
    replace: &str,
    prefix: &str,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    qfind_core::components::rename_pairs(paths, find, replace, prefix, "", 1)
}

pub(super) fn rename(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    rows: Vec<RowData>,
) {
    let paths: Vec<PathBuf> = rows.iter().map(|row| row.path().into()).collect();
    let (dialog, body) = dialog(window, "Batch rename");
    let find = field(&body, "Find text (leave empty to only add a prefix)", "");
    let replace = field(&body, "Replace with", "");
    let prefix = field(&body, "Prefix · use {n} for numbering", "");
    let output = text_view(&body);
    let apply = gtk::Button::with_label("Rename files");
    apply.add_css_class("suggested-action");
    body.append(&apply);
    let preview: Rc<dyn Fn()> = {
        let (find, replace, prefix, paths, apply) = (
            find.clone(),
            replace.clone(),
            prefix.clone(),
            paths.clone(),
            apply.clone(),
        );
        Rc::new(
            move || match rename_pairs(&paths, &find.text(), &replace.text(), &prefix.text()) {
                Ok(pairs) => {
                    apply.set_sensitive(pairs.iter().any(|(a, b)| a != b));
                    output.set_text(
                        &pairs
                            .iter()
                            .take(200)
                            .map(|(a, b)| {
                                format!(
                                    "{}  →  {}",
                                    a.display(),
                                    b.file_name().unwrap_or_default().to_string_lossy()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                Err(error) => {
                    apply.set_sensitive(false);
                    output.set_text(&error);
                }
            },
        )
    };
    for entry in [&find, &replace, &prefix] {
        let preview = preview.clone();
        entry.connect_changed(move |_| preview());
    }
    preview();
    let window = window.clone();
    let state = state.clone();
    let close = dialog.clone();
    apply.connect_clicked(move |_| {
        let pairs = match rename_pairs(&paths, &find.text(), &replace.text(), &prefix.text()) {
            Ok(pairs) => pairs,
            Err(error) => {
                state.borrow().status.set_text(&error);
                return;
            }
        };
        close.close();
        job(&window, &state, "Batch rename results", move || {
            for (from, to) in &pairs {
                if from != to && fs::symlink_metadata(to).is_ok() {
                    return Err(format!("Nothing renamed: {} already exists", to.display()));
                }
            }
            let mut done = 0;
            for (from, to) in pairs.into_iter().filter(|(a, b)| a != b) {
                qfind_core::rename(&from, &to).map_err(|error| {
                    format!("Renamed {done}; stopped at {}: {error}", from.display())
                })?;
                done += 1;
            }
            Ok(format!("Renamed {done} items."))
        });
    });
    dialog.present();
}

fn transfer(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    rows: Vec<RowData>,
    action: &'static str,
) {
    let paths: Vec<PathBuf> = rows.iter().map(|row| row.path().into()).collect();
    let title = match action {
        "batch-copy" => "Copy selected to folder",
        "batch-move" => "Move selected to folder",
        "batch-zip" => "Compress selected (.zip, .7z, .tar.gz)",
        _ => "Extract archives to folder",
    };
    let initial = current_dir(state).join(if action == "batch-zip" {
        "Archive.zip"
    } else {
        ""
    });
    let win = window.clone();
    let state = state.clone();
    prompt_text(window, title, &initial.to_string_lossy(), move |raw| {
        let paths = paths.clone();
        let dest = PathBuf::from(raw);
        if !dest.is_absolute() {
            state
                .borrow()
                .status
                .set_text("Enter an absolute destination path");
            return;
        }
        job(&win, &state, title, move || {
            if action == "batch-zip" {
                archive::compress(&paths, &dest).map_err(|e| e.to_string())?;
                return Ok(format!("Created {}", dest.display()));
            }
            if !dest.is_dir() {
                return Err("Destination must be an existing folder".into());
            }
            let dest = dest.canonicalize().map_err(|e| e.to_string())?;
            let mut targets = HashSet::new();
            let pairs: Vec<_> = paths
                .iter()
                .map(|path| {
                    let name = path
                        .file_name()
                        .ok_or("Cannot operate on a filesystem root")?;
                    let target = if action == "batch-extract" {
                        dest.join(format!("{}.extracted", name.to_string_lossy()))
                    } else {
                        dest.join(name)
                    };
                    let source = path.canonicalize().map_err(|e| e.to_string())?;
                    if target.starts_with(&source)
                        || paths
                            .iter()
                            .any(|other| other != path && path.starts_with(other))
                    {
                        return Err(
                            "Destination or selection is nested inside another selected folder"
                                .into(),
                        );
                    }
                    if fs::symlink_metadata(&target).is_ok() || !targets.insert(target.clone()) {
                        return Err(format!(
                            "Destination already exists or repeats: {}",
                            target.display()
                        ));
                    }
                    if action == "batch-extract" && !archive::is_archive(path) {
                        return Err(format!("Not a supported archive: {}", path.display()));
                    }
                    Ok((path.clone(), target))
                })
                .collect::<Result<_, String>>()?;
            let mut done = 0;
            for (source, target) in pairs {
                let result = match action {
                    "batch-copy" => qfind_core::copy(&source, &target)
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                    "batch-move" => qfind_core::move_path(&source, &target)
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                    _ => archive::extract(&source, &target).map_err(|e| e.to_string()),
                };
                result.map_err(|e| format!("Completed {done}; stopped at {}: {e}\nCheck the destination for partial output before retrying.", source.display()))?;
                done += 1;
            }
            Ok(format!("Completed {done} items in {}", dest.display()))
        });
    });
}

pub(super) use qfind_core::projects::{Project, active_project_account, index_projects, refresh_project_account};

fn project_details(window: &gtk::ApplicationWindow, state: &Rc<RefCell<State>>, path: PathBuf, rust: bool, node: bool, git: bool) {
    let (dialog, body) = dialog(window, "Project");
    dialog.set_modal(false);
    body.append(&project_detail_content(window, state, path, rust, node, git));
    dialog.present();
}

pub(super) fn project_detail_content(_window: &gtk::ApplicationWindow, _state: &Rc<RefCell<State>>, path: PathBuf, _rust: bool, _node: bool, git: bool) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let output = text_view(&body);
    output.set_text("Reading project…");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let commands = qfind_core::components::task_commands(&path);
    if !commands.is_empty() {
        let choices = gtk::DropDown::from_strings(
            &commands.iter().map(|(_, name, _)| *name).collect::<Vec<_>>(),
        );
        actions.append(&choices);
        let run = gtk::Button::with_label("Run command");
        actions.append(&run);
        let output = output.clone();
        let path = path.clone();
        run.connect_clicked(move |button| {
            button.set_sensitive(false);
            let button = button.clone();
            let command = commands[choices.selected() as usize].0;
            let path = path.clone();
            output.set_text(&format!("Running {}…", command));
            let output = output.clone();
            glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(move || -> Result<String, String> {
                qfind_core::components::run_task(&path, command)
            }).await;
            output.set_text(&match result { Ok(Ok(message)) => message, Ok(Err(error)) => error, Err(_) => "Command worker failed".into() });
            button.set_sensitive(true);
            });
        });
        let hint = gtk::Label::new(Some(
            "Builds and installs run project scripts and may download packages.",
        ));
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        body.append(&hint);
    }
    body.prepend(&actions);
    glib::MainContext::default().spawn_local(async move {
        let result = gio::spawn_blocking(move || {
            let mut report = format!("{}\n", path.display());
            if git {
                match Command::new("git")
                    .args([
                        "--no-optional-locks",
                        "status",
                        "--short",
                        "--branch",
                        "--untracked-files=normal",
                    ])
                    .current_dir(&path)
                    .output()
                {
                    Ok(result) => {
                        report.push_str("\nGit status\n");
                        report.push_str(&String::from_utf8_lossy(&result.stdout));
                        report.push_str(&String::from_utf8_lossy(&result.stderr));
                    }
                    Err(error) => report.push_str(&format!("\nGit unavailable: {error}\n")),
                }
            }
            for name in ["Cargo.toml", "package.json"] {
                if let Ok(file) = fs::File::open(path.join(name)) {
                    use std::io::Read;
                    let mut content = String::new();
                    if file.take(65_536).read_to_string(&mut content).is_ok() {
                        report.push_str(&format!("\n{name} (up to 64 KiB)\n{content}\n"));
                    }
                }
            }
            for name in [
                "Cargo.lock",
                "package-lock.json",
                "bun.lock",
                "bun.lockb",
                "pnpm-lock.yaml",
                "yarn.lock",
            ] {
                if path.join(name).is_file() {
                    report.push_str(&format!("\nLockfile: {name}"));
                }
            }
            report
        })
        .await;
        if output.text(&output.start_iter(), &output.end_iter(), false) == "Reading project…" {
            output.set_text(&result.unwrap_or_else(|_| "Could not read project".into()));
        }
    });
    body
}

fn builds_active() -> Result<bool, String> {
    // Global guard: shared targets and package caches may be used from another project.
    for entry in fs::read_dir("/proc").map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        match fs::read_to_string(entry.path().join("comm")) {
            Ok(name)
                if matches!(
                    name.trim(),
                    "cargo" | "rustc" | "rust-analyzer" | "npm" | "bun" | "node" | "pnpm" | "yarn"
                ) =>
            {
                return Ok(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err("Cannot inspect active processes; cleanup is unavailable".into());
            }
            _ => {}
        }
    }
    Ok(false)
}

pub(super) fn project_content_at(window: &gtk::ApplicationWindow, state: &Rc<RefCell<State>>, root: PathBuf) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_top(8);
    body.set_margin_bottom(12);
    let summary = gtk::Label::new(Some("Reading project index…"));
    summary.set_wrap(true);
    summary.set_xalign(0.0);
    body.append(&summary);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.append(
        &gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .build(),
    );
    let scope_hint = gtk::Label::new(Some("Projects come from the file index. Build sizes are saved; missing sizes are measured in the background."));
    scope_hint.set_wrap(true);
    scope_hint.add_css_class("dim-label");
    body.append(&scope_hint);
    let cleanup = gtk::Button::with_label("Review selected cleanup…");
    cleanup.set_sensitive(false);
    body.append(&cleanup);
    let selected = Rc::new(RefCell::new(Vec::<(gtk::CheckButton, PathBuf, u64)>::new()));
    let win = window.clone();
    let state_for_scan = state.clone();
    let selected_for_scan = selected.clone();
    let cleanup_for_scan = cleanup.clone();
    let storage = state.borrow().storage.clone();
    let weak_body = body.downgrade();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if weak_body.upgrade().is_none() { return glib::ControlFlow::Break; }
        let Some(projects) = storage.projects(&root) else { return glib::ControlFlow::Continue; };
        {
                let bytes: u64 = projects.iter().flat_map(|p| &p.artifacts).filter_map(|(_,n)| *n).sum();
                let unknown = projects.iter().flat_map(|project| &project.artifacts).any(|(_, bytes)| bytes.is_none());
                summary.set_text(&format!("{} projects · {}", projects.len(), if unknown { "build sizes below".into() } else { format!("{} in builds & dependencies", actions::human_size(bytes)) }));
                for project in projects.into_iter().filter(|project| !project.artifacts.is_empty()) {
                    let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
                    row.add_css_class("qfind-project");
                    let title = gtk::Button::with_label(&format!("{}  {}{}{}", project.path.file_name().unwrap_or_default().to_string_lossy(), if project.rust { "Rust " } else { "" }, if project.node { "JS " } else { "" }, if project.git { "Git" } else { "" }));
                    title.add_css_class("flat");
                    title.set_halign(gtk::Align::Fill);
                    title.set_tooltip_text(Some(&project.path.to_string_lossy()));
                    if let Some(label) = title.child().and_downcast::<gtk::Label>() {
                        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                        label.set_xalign(0.0);
                    }
                    row.append(&title);
                    let path = project.path.clone();
                    let win = win.clone();
                    let state = state_for_scan.clone();
                    title.connect_clicked(move |_| project_details(&win, &state, path.clone(), project.rust, project.node, project.git));
                    for (path, bytes) in project.artifacts {
                        let check = gtk::CheckButton::with_label(&format!("{} · {}", path.file_name().unwrap_or_default().to_string_lossy(), storage.indexed_size_text(&path)));
                        let weak = check.downgrade();
                        let size_path = path.clone();
                        let storage = storage.clone();
                        glib::timeout_add_local(Duration::from_millis(500), move || {
                            let Some(check) = weak.upgrade() else { return glib::ControlFlow::Break; };
                            if !check.is_mapped() { return glib::ControlFlow::Continue; }
                            let label = format!("{} · {}", size_path.file_name().unwrap_or_default().to_string_lossy(), storage.indexed_size_text(&size_path));
                            if check.label().as_deref() != Some(&label) { check.set_label(Some(&label)); }
                            glib::ControlFlow::Continue
                        });
                        row.append(&check);
                        let selected = selected_for_scan.clone();
                        let cleanup = cleanup_for_scan.clone();
                        check.connect_toggled(move |_| cleanup.set_sensitive(selected.borrow().iter().any(|(check,_,_)| check.is_active())));
                        selected_for_scan.borrow_mut().push((check, path, bytes.unwrap_or(0)));
                    }
                    list.append(&row);
                }
        }
        glib::ControlFlow::Break
    });
    let win = window.clone();
    let state = state.clone();
    cleanup.connect_clicked(move |_| {
        let paths: Vec<_> = selected.borrow().iter().filter(|(check,_,_)| check.is_active()).map(|(_,path,_)| path.clone()).collect();
        if paths.is_empty() { return; }
        let (review, body) = self::dialog(&win, "Review storage cleanup");
        let output = text_view(&body);
        output.set_text(&format!("Move these folders to your desktop Trash?\n\n{}\n\nRestore them through your desktop Trash if needed. Space is freed only when Trash is emptied. Dependencies/builds will need to be recreated. Cleanup is blocked while build or package processes are active.", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")));
        let apply = gtk::Button::with_label("Move reviewed folders to Trash");
        body.append(&apply);
        let win = win.clone();
        let state = state.clone();
        let close = review.clone();
        apply.connect_clicked(move |button| {
            button.set_sensitive(false);
            let paths = paths.clone();
            close.close();
            job(&win, &state, "Storage cleanup", move || {
                let mut done = 0;
                for path in paths {
                    if builds_active()? { return Err(format!("Moved {done} folders; cleanup blocked by an active build/package process.")); }
                    if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) { return Err(format!("Folder changed since review: {}", path.display())); }
                    if path.canonicalize().map_err(|e| e.to_string())? != path {
                        return Err(format!("Path changed since review: {}", path.display()));
                    }
                    if let Some(parent) = path.parent() {
                        let tracked = Command::new("git").arg("-C").arg(parent)
                            .args(["ls-files", "-z", "--"]).arg(path.file_name().unwrap_or_default()).output()
                            .map_err(|e| format!("Cannot check tracked files: {e}"))?;
                        if tracked.status.success() && !tracked.stdout.is_empty() {
                            return Err(format!("Cleanup blocked: {} contains Git-tracked files", path.display()));
                        }
                    }
                    gio::File::for_path(&path).trash(gio::Cancellable::NONE).map_err(|e| format!("Moved {done}; {}: {e}", path.display()))?;
                    done += 1;
                }
                Ok(format!("Moved {done} folders to your desktop Trash. Empty Trash separately to reclaim the space."))
            });
        });
        review.present();
    });
    body
}
