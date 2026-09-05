use super::*;

#[derive(Clone, PartialEq, Eq)]
struct Query {
    directory: PathBuf,
    selected: Option<PathBuf>,
    staged: bool,
    file: Option<PathBuf>,
    visible: bool,
}

fn selected_path(state: &Rc<RefCell<State>>) -> Option<PathBuf> {
    let rows = selected_rows(&state.borrow().selection);
    if rows.len() == 1 { Some(rows[0].path().into()) } else { None }
}

use qfind_core::components::git;

pub fn new(state: Rc<RefCell<State>>, project: Option<Rc<RefCell<Option<PathBuf>>>>) -> (gtk::Box, gtk::Button) {
    let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    host.append(&root);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_bottom(12);
    let footer = gtk::Button::with_label("Git · checking…");
    footer.add_css_class("flat");
    footer.set_tooltip_text(Some("Open Git changes"));
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mode = gtk::DropDown::from_strings(&["Working tree", "Staged changes"]);
    mode.set_hexpand(true);
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh Git status and diff"));
    controls.append(&mode);
    controls.append(&refresh);
    let expand = gtk::Button::from_icon_name("view-fullscreen-symbolic");
    expand.set_tooltip_text(Some("Expand diff review"));
    controls.append(&expand);
    {
        let (host, root) = (host.downgrade(), root.downgrade());
        expand.connect_clicked(move |button| {
            let (Some(host), Some(root)) = (host.upgrade(), root.upgrade()) else { return; };
            let window = gtk::Window::builder().title("Megaman · Changes").default_width(1100).default_height(760).build();
            window.set_titlebar(Some(&gtk::HeaderBar::new()));
            if let Some(parent) = button.root().and_downcast::<gtk::Window>() { window.set_transient_for(Some(&parent)); }
            host.remove(&root);
            window.set_child(Some(&root));
            button.set_visible(false);
            let (host, root, button) = (host.clone(), root.clone(), button.clone());
            window.connect_close_request(move |window| {
                window.set_child(None::<&gtk::Widget>);
                host.append(&root);
                button.set_visible(true);
                glib::Propagation::Proceed
            });
            window.present();
        });
    }
    root.append(&controls);
    let scope = gtk::Label::new(Some("Select a file to inspect its changes"));
    scope.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    scope.set_xalign(0.0);
    root.append(&scope);
    let file_names = gtk::StringList::new(&["All changes"]);
    let file_picker = gtk::DropDown::new(Some(file_names.clone()), None::<gtk::Expression>);
    file_picker.set_enable_search(true);
    file_picker.set_tooltip_text(Some("Jump to a changed file"));
    let file_controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    file_picker.set_hexpand(true);
    file_controls.append(&file_picker);
    let stage = gtk::Button::with_label("Stage file");
    stage.set_sensitive(false);
    file_controls.append(&stage);
    root.append(&file_controls);
    let action_status = gtk::Label::new(None);
    action_status.set_wrap(true);
    action_status.set_xalign(0.0);
    action_status.set_visible(false);
    root.append(&action_status);
    let buffer = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
    root.append(&diff_view(&buffer));
    let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy.set_tooltip_text(Some("Copy the original patch without display line numbers"));
    controls.append(&copy);
    let patch = buffer.clone();
    copy.connect_clicked(move |button| button.display().clipboard().set_text(&patch.text(&patch.start_iter(), &patch.end_iter(), false)));
    let refresh_requested = Rc::new(Cell::new(true));
    let flag = refresh_requested.clone();
    refresh.connect_clicked(move |_| flag.set(true));
    let reviewed: Rc<RefCell<Option<Query>>> = Rc::new(RefCell::new(None));
    let action_busy = Rc::new(Cell::new(false));
    {
        let reviewed = reviewed.clone();
        let (state, project, mode, file_picker, file_names, refresh, action_status, busy) = (state.clone(), project.clone(), mode.clone(), file_picker.clone(), file_names.clone(), refresh_requested.clone(), action_status.clone(), action_busy.clone());
        stage.connect_clicked(move |button| {
            if busy.replace(true) { return; }
            let Some(file) = (file_picker.selected() > 0).then(|| file_names.string(file_picker.selected())).flatten() else { busy.set(false); return; };
            let directory = project.as_ref().and_then(|path| path.borrow().clone()).unwrap_or_else(|| current_dir(&state));
            let staged = mode.selected() == 1;
            if !reviewed.borrow().as_ref().is_some_and(|query| query.directory == directory && query.staged == staged && query.file.as_deref() == Some(Path::new(file.as_str())) && query.visible) {
                busy.set(false);
                action_status.set_text("Wait for this file’s diff to finish loading");
                action_status.set_visible(true);
                return;
            }
            let (button, status, refresh, busy) = (button.clone(), action_status.clone(), refresh.clone(), busy.clone());
            button.set_sensitive(false);
            status.set_text(if staged { "Unstaging file…" } else { "Staging file…" });
            status.set_visible(true);
            glib::MainContext::default().spawn_local(async move {
                let result = gio::spawn_blocking(move || {
                    let root = PathBuf::from(git(&directory, &["rev-parse", "--show-toplevel"], None)?.trim());
                    let path = Path::new(file.as_str());
                    if path.is_absolute() || path.components().any(|part| !matches!(part, std::path::Component::Normal(_))) { return Err("Invalid repository-relative path".into()); }
                    if staged {
                        if git(&root, &["rev-parse", "--verify", "HEAD"], None).is_ok() {
                            git(&root, &["restore", "--staged"], Some(path))
                        } else { git(&root, &["rm", "--cached"], Some(path)) }
                    } else { git(&root, &["add"], Some(path)) }
                }).await;
                busy.set(false);
                match result {
                    Ok(Ok(_)) => status.set_text(if staged { "File unstaged" } else { "File staged" }),
                    Ok(Err(error)) => status.set_text(&error),
                    Err(_) => status.set_text("Git action failed unexpectedly"),
                }
                refresh.set(true);
                button.set_sensitive(true);
            });
        });
    }
    let weak = root.downgrade();
    let mut last: Option<Query> = None;
    let in_flight = Rc::new(Cell::new(false));
    let mut last_status = std::time::Instant::now();
    let footer_poll = footer.clone();
    // Poll the local selection cheaply; Git only runs on change or every five seconds.
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(root) = weak.upgrade() else { return glib::ControlFlow::Break; };
        if !root.is_mapped() && !footer_poll.is_mapped() { return glib::ControlFlow::Continue; }
        if project.as_ref().is_some_and(|path| path.borrow().is_none()) { return glib::ControlFlow::Continue; }
        if project.is_some() && !root.is_mapped() { return glib::ControlFlow::Continue; }
        stage.set_label(if mode.selected() == 1 { "Unstage file" } else { "Stage file" });
        stage.set_sensitive(file_picker.selected() > 0 && file_picker.selected() < file_names.n_items() && !action_busy.get());
        if in_flight.get() || action_busy.get() { return glib::ControlFlow::Continue; }
        let directory = project.as_ref().and_then(|path| path.borrow().clone()).unwrap_or_else(|| current_dir(&state));
        if last.as_ref().is_some_and(|last| last.directory != directory || last.staged != (mode.selected() == 1)) { file_picker.set_selected(0); }
        let query = Query {
            directory,
            selected: if project.is_none() && root.is_mapped() { selected_path(&state) } else { None },
            staged: mode.selected() == 1,
            file: (file_picker.selected() > 0).then(|| file_names.string(file_picker.selected())).flatten().map(|path| PathBuf::from(path.as_str())),
            visible: root.is_mapped(),
        };
        if last.as_ref() == Some(&query) && !refresh_requested.get() && last_status.elapsed() < Duration::from_secs(5) {
            return glib::ControlFlow::Continue;
        }
        refresh_requested.set(false);
        last_status = std::time::Instant::now();
        last = Some(query.clone());
        in_flight.set(true);
        let project = project.clone();
        let task = query.clone();
        let state = state.clone();
        let in_flight = in_flight.clone();
        let (footer, buffer, scope, mode) = (footer_poll.clone(), buffer.clone(), scope.clone(), mode.clone());
        let file_picker = file_picker.clone();
        let file_names = file_names.clone();
        let reviewed = reviewed.clone();
        glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(move || {
                let root = PathBuf::from(git(&task.directory, &["rev-parse", "--show-toplevel"], None)?.trim());
                let branch = git(&root, &["symbolic-ref", "--short", "HEAD"], None)
                    .or_else(|_| git(&root, &["rev-parse", "--short", "HEAD"], None))?;
                let status = git(&root, &["status", "--short", "--untracked-files=normal"], None)?;
                let count = status.lines().count();
                let label = format!("{} · {}", branch.trim(), if count == 0 { "clean".into() } else { format!("{count} {}", if count == 1 { "change" } else { "changes" }) });
                let mut diff = String::new();
                let mut files = Vec::<String>::new();
                if task.visible {
                    let mut names_args = vec!["diff", "--name-only", "-z"];
                    if task.staged { names_args.push("--cached"); }
                    let mut names = git(&root, &names_args, None)?;
                    if !task.staged { names.push_str(&git(&root, &["ls-files", "--others", "--exclude-standard", "-z"], None)?); }
                    files = names.split('\0').filter(|name| !name.is_empty()).map(str::to_owned).collect();
                    files.sort();
                    files.dedup();
                    let path = task.file.as_deref().or_else(|| task.selected.as_deref().and_then(|path| path.strip_prefix(&root).ok()));
                    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
                    if task.staged { args.push("--cached"); }
                    diff = git(&root, &args, path)?;
                    if diff.is_empty() && !task.staged {
                        if let Some(path) = path.filter(|path| root.join(path).is_file()) {
                            if git(&root, &["ls-files", "--error-unmatch"], Some(path)).is_err() {
                                diff = git(&root, &["diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color", "--", "/dev/null", &path.to_string_lossy()], None).unwrap_or_else(|diff| diff);
                            }
                        }
                    }
                    if diff.is_empty() {
                        diff = if task.staged { "No staged changes for this selection.".into() }
                            else { format!("No unstaged diff for this selection.\nChoose an untracked file above to preview its additions.\n\n{status}") };
                    }
                }
                Ok::<_, String>((root, label, diff, files))
            }).await;
            in_flight.set(false);
            let selected = if project.is_none() && query.visible { selected_path(&state) } else { None };
            let current_file = (file_picker.selected() > 0).then(|| file_names.string(file_picker.selected())).flatten().map(|path| PathBuf::from(path.as_str()));
            if current_file != query.file { return; }
            if project.as_ref().and_then(|path| path.borrow().clone()).unwrap_or_else(|| current_dir(&state)) != query.directory || selected != query.selected || (mode.selected() == 1) != query.staged { return; }
            match result {
                Ok(Ok((root, label, diff, files))) => {
                    *reviewed.borrow_mut() = Some(query.clone());
                    footer.set_label(&label);
                    footer.set_tooltip_text(Some(&format!("{} · Open Git changes", root.display())));
                    let mut names = vec!["All changes".to_owned()];
                    names.extend(files);
                    if (0..file_names.n_items()).filter_map(|i| file_names.string(i)).map(|s| s.to_string()).collect::<Vec<_>>() != names {
                        file_names.splice(0, file_names.n_items(), &names.iter().map(String::as_str).collect::<Vec<_>>());
                        file_picker.set_selected(query.file.as_ref().and_then(|path| names.iter().position(|name| Path::new(name) == path)).unwrap_or(0) as u32);
                    }
                    scope.set_text(&query.file.as_deref().or(query.selected.as_deref()).map(|path| path.strip_prefix(&root).unwrap_or(path))
                        .map(|path| path.to_string_lossy().into_owned()).unwrap_or_else(|| "All tracked changes".into()));
                    if query.visible && buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).as_str() != diff {
                        buffer.set_text(&diff);
                    }
                }
                Ok(Err(error)) => {
                    *reviewed.borrow_mut() = None;
                    footer.set_label(if error.contains("not a git repository") { "Not a Git repo" } else { "Git · unavailable" });
                    scope.set_text("Git status");
                    buffer.set_text(&error);
                }
                Err(_) => { footer.set_label("Git · refresh failed"); }
            }
        });
        glib::ControlFlow::Continue
    });
    (host, footer)
}

// Bound intraline comparison independently of the patch-size limit.
fn word_changes(left: &str, right: &str, budget: &mut usize) -> [Vec<(i32, i32)>; 2] {
    if left.len() > 2048 || right.len() > 2048 { return [vec![], vec![]]; }
    let tokens = |text: &str| {
        let mut result = Vec::new();
        let mut start = 0;
        let mut chars = 0_i32;
        let mut token_start = 0;
        let mut category = None;
        for (byte, ch) in text.char_indices() {
            let kind = if ch.is_alphanumeric() || ch == '_' { 0 } else if ch.is_whitespace() { 1 } else { 2 };
            if category.is_some_and(|old| old != kind || kind == 2) {
                result.push((start, byte, token_start, chars)); start = byte; token_start = chars;
            }
            category = Some(kind); chars += 1;
        }
        if start < text.len() { result.push((start, text.len(), token_start, chars)); }
        result
    };
    let (a,b) = (tokens(left), tokens(right));
    let work = (a.len()+1).saturating_mul(b.len()+1);
    if a.len() > 128 || b.len() > 128 || work > *budget { return [vec![], vec![]]; }
    *budget -= work;
    let width = b.len()+1;
    let mut lcs = vec![0_u16; work];
    for i in (0..a.len()).rev() { for j in (0..b.len()).rev() {
        lcs[i*width+j] = if left[a[i].0..a[i].1] == right[b[j].0..b[j].1] { 1+lcs[(i+1)*width+j+1] } else { lcs[(i+1)*width+j].max(lcs[i*width+j+1]) };
    }}
    let (mut i,mut j) = (0,0);
    let mut changed = [Vec::new(),Vec::new()];
    while i<a.len() || j<b.len() {
        if i<a.len() && j<b.len() && left[a[i].0..a[i].1] == right[b[j].0..b[j].1] { i+=1; j+=1; }
        else if i<a.len() && (j==b.len() || lcs[(i+1)*width+j] >= lcs[i*width+j+1]) { changed[0].push((a[i].2,a[i].3)); i+=1; }
        else { changed[1].push((b[j].2,b[j].3)); j+=1; }
    }
    changed
}

// Both the inspector and expanded review use this view and the same live source.
fn diff_view(source: &gtk::TextBuffer) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let split = gtk::ToggleButton::with_label("Split");
    split.set_tooltip_text(Some("Show old and new lines side by side"));
    let context = gtk::ToggleButton::with_label("Context");
    context.set_active(true);
    context.set_tooltip_text(Some("Show unchanged lines around changes"));
    let wrap = gtk::ToggleButton::with_label("Wrap");
    wrap.set_tooltip_text(Some("Wrap lines in unified view"));
    {
        let wrap = wrap.clone();
        split.connect_toggled(move |button| {
            if button.is_active() { wrap.set_active(false); }
            wrap.set_sensitive(!button.is_active());
        });
    }
    let previous = gtk::Button::from_icon_name("go-previous-symbolic");
    previous.set_tooltip_text(Some("Previous hunk"));
    let next = gtk::Button::from_icon_name("go-next-symbolic");
    next.set_tooltip_text(Some("Next hunk"));
    for button in [&previous, &next, split.upcast_ref(), context.upcast_ref(), wrap.upcast_ref()] { controls.append(button); }
    root.append(&controls);
    let hunk_controls = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let hunk_names = gtk::StringList::new(&[]);
    let hunk_picker = gtk::DropDown::new(Some(hunk_names.clone()), None::<gtk::Expression>);
    hunk_picker.set_hexpand(true);
    hunk_picker.set_enable_search(true);
    hunk_picker.set_tooltip_text(Some("Select a hunk, then collapse or reveal it"));
    let fold = gtk::Button::with_label("Collapse");
    hunk_controls.append(&hunk_picker);
    hunk_controls.append(&fold);
    root.append(&hunk_controls);
    let collapsed = Rc::new(RefCell::new(std::collections::HashSet::<String>::new()));
    let hunk_rows = Rc::new(RefCell::new(Vec::<(String,i32)>::new()));
    let panes = gtk::Paned::new(gtk::Orientation::Horizontal);
    panes.set_vexpand(true);
    let left = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).build();
    let right = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).build();
    left.set_accessible_role(gtk::AccessibleRole::Document);
    right.set_accessible_role(gtk::AccessibleRole::Document);
    let left_scroll = gtk::ScrolledWindow::builder().child(&left).vexpand(true).hexpand(true).build();
    let right_scroll = gtk::ScrolledWindow::builder().child(&right).vexpand(true).hexpand(true).build();
    right_scroll.set_vadjustment(Some(&left_scroll.vadjustment()));
    panes.set_start_child(Some(&left_scroll));
    panes.set_end_child(Some(&right_scroll));
    panes.set_resize_start_child(true);
    panes.set_resize_end_child(true);
    panes.set_shrink_start_child(false);
    panes.set_shrink_end_child(false);
    right_scroll.set_visible(false);
    root.append(&panes);
    for view in [&left, &right] {
        for (name, color) in [("add", "#57b879"), ("del", "#e47e87")] {
            view.buffer().tag_table().add(&gtk::TextTag::builder().name(name).foreground(color).build());
        }
        view.buffer().tag_table().add(&gtk::TextTag::builder().name("word").background_rgba(&gtk::gdk::RGBA::new(0.6, 0.6, 0.25, 0.30)).weight(700).build());
        view.buffer().tag_table().add(&gtk::TextTag::builder().name("header").weight(700).build());
    }
    let render: Rc<dyn Fn()> = {
        let (collapsed, hunk_rows, hunk_names, hunk_picker, fold) = (collapsed.clone(), hunk_rows.clone(), hunk_names.clone(), hunk_picker.downgrade(), fold.downgrade());
        let (left, right, left_scroll, right_scroll, source, split, context) = (left.clone(), right.clone(), left_scroll.clone(), right_scroll.clone(), source.clone(), split.downgrade(), context.downgrade());
        Rc::new(move || {
            let (Some(split), Some(context)) = (split.upgrade(), context.upgrade()) else { return; };
            let value = source.text(&source.start_iter(), &source.end_iter(), false);
            right_scroll.set_visible(split.is_active());
            let adjustment = left_scroll.vadjustment();
            let scroll = adjustment.value();
            let buffers = [left.buffer(), right.buffer()];
            let saved: Vec<_> = buffers.iter().map(|b| b.selection_bounds().map(|(a,z)| (a.offset(),z.offset()))).collect();
            for buffer in &buffers { buffer.set_text(""); }
            let (mut old, mut new) = (0_u64, 0_u64);
            let mut in_hunk = false;
            let (mut removed, mut added) = (Vec::<(String, i32)>::new(), Vec::<(String, i32)>::new());
            let word_budget = Cell::new(200_000_usize);
            let mut file = String::new();
            let mut current_hunk = String::new();
            let mut entries = Vec::new();
            let mut labels = Vec::new();
            let flush = |removed: &mut Vec<(String, i32)>, added: &mut Vec<(String, i32)>| {
                for i in 0..removed.len().max(added.len()) {
                    let mut offsets = [0,0];
                    for (side, (lines, tag)) in [(removed.as_slice(), "del"), (added.as_slice(), "add")].into_iter().enumerate() {
                        if split.is_active() {
                            offsets[side] = buffers[side].char_count();
                            buffers[side].insert_with_tags_by_name(&mut buffers[side].end_iter(), lines.get(i).map(|row| row.0.as_str()).unwrap_or("\n"), &[tag]);
                        } else { offsets[side] = lines.get(i).map(|row| row.1).unwrap_or(0); }
                    }
                    if let (Some(a), Some(b)) = (removed.get(i), added.get(i)) {
                        let left = a.0.split_once("│ ").map(|(_,s)| s).unwrap_or(&a.0);
                        let right = b.0.split_once("│ ").map(|(_,s)| s).unwrap_or(&b.0);
                        let gutters = [&a.0, &b.0].map(|line| line.split_once("│ ").map(|(prefix,_)| prefix.chars().count() as i32 + 2 + i32::from(!split.is_active())).unwrap_or(0));
                        let (left,right) = if split.is_active() { (left,right) } else { (&left[1..],&right[1..]) };
                        let mut budget = word_budget.get();
                        let changes = word_changes(left,right,&mut budget);
                        word_budget.set(budget);
                        for (side, spans) in changes.into_iter().enumerate() {
                            let buffer = &buffers[if split.is_active() { side } else { 0 }];
                            for (a,b) in spans { buffer.apply_tag_by_name("word", &buffer.iter_at_offset(offsets[side]+gutters[side]+a), &buffer.iter_at_offset(offsets[side]+gutters[side]+b)); }
                        }
                    }
                }
                removed.clear(); added.clear();
            };
            for line in value.lines() {
                let header = line.starts_with("@@") || line.starts_with("diff --git");
                if line.starts_with("diff --git") { in_hunk = false; file = line.to_owned(); }
                if let Some(path) = line.strip_prefix("+++ b/") { file = path.to_owned(); }
                if line.starts_with("@@ ") {
                    let mut ranges = line.split_whitespace().skip(1);
                    old = ranges.next().and_then(|s| s.trim_start_matches('-').split(',').next()?.parse().ok()).unwrap_or(0);
                    new = ranges.next().and_then(|s| s.trim_start_matches('+').split(',').next()?.parse().ok()).unwrap_or(0);
                    in_hunk = true;
                    current_hunk = format!("{file}\n{line}");
                }
                let kind = if in_hunk && !header { line.chars().next().unwrap_or(' ') } else { '\0' };
                let old_number = if matches!(kind, '-' | ' ') { let n=old; old+=1; format!("{n:>5}") } else { "     ".into() };
                let new_number = if matches!(kind, '+' | ' ') { let n=new; new+=1; format!("{n:>5}") } else { "     ".into() };
                if !matches!(kind, '+' | '-') { flush(&mut removed, &mut added); }
                if line.starts_with("@@ ") {
                    entries.push((current_hunk.clone(), buffers[0].end_iter().line()));
                    labels.push(format!("{} · {}", file, line));
                }
                if in_hunk && !header && collapsed.borrow().contains(&current_hunk) { continue; }
                if kind == ' ' && !context.is_active() { continue; }
                let folded_header = if line.starts_with("@@ ") && collapsed.borrow().contains(&current_hunk) { Some(format!("{line}  [collapsed]")) } else { None };
                let line = folded_header.as_deref().unwrap_or(line);
                let tag = match kind { '+' => Some("add"), '-' => Some("del"), _ if header => Some("header"), _ => None };
                let append = |buffer: &gtk::TextBuffer, text: &str| {
                    if let Some(tag) = tag { buffer.insert_with_tags_by_name(&mut buffer.end_iter(), text, &[tag]); }
                    else { buffer.insert(&mut buffer.end_iter(), text); }
                };
                if split.is_active() {
                    let content = if matches!(kind, '+' | '-' | ' ') { &line[1..] } else { line };
                    if kind == '-' { removed.push((format!("{old_number} │ {content}\n"), 0)); }
                    else if kind == '+' { added.push((format!("{new_number} │ {content}\n"), 0)); }
                    else {
                        append(&buffers[0], &format!("{old_number} │ {content}\n"));
                        append(&buffers[1], &format!("{new_number} │ {content}\n"));
                    }
                } else {
                    let row = format!("{old_number} {new_number} │ {line}\n");
                    let offset = buffers[0].char_count();
                    append(&buffers[0], &row);
                    if kind == '-' { removed.push((row,offset)); }
                    else if kind == '+' { added.push((row,offset)); }
                }
            }
            flush(&mut removed, &mut added);
            let old_key = hunk_picker.upgrade().and_then(|picker| hunk_rows.borrow().get(picker.selected() as usize).map(|row| row.0.clone()));
            collapsed.borrow_mut().retain(|key| entries.iter().any(|row| &row.0 == key));
            *hunk_rows.borrow_mut() = entries;
            if (0..hunk_names.n_items()).filter_map(|i| hunk_names.string(i)).map(|s| s.to_string()).collect::<Vec<_>>() != labels {
                hunk_names.splice(0,hunk_names.n_items(), &labels.iter().map(String::as_str).collect::<Vec<_>>());
                if let Some(picker) = hunk_picker.upgrade() { picker.set_selected(old_key.and_then(|key| hunk_rows.borrow().iter().position(|row| row.0 == key)).unwrap_or(0) as u32); }
            }
            if let (Some(picker),Some(fold)) = (hunk_picker.upgrade(),fold.upgrade()) {
                let rows = hunk_rows.borrow();
                let current = rows.get(picker.selected() as usize);
                fold.set_sensitive(current.is_some());
                fold.set_label(if current.is_some_and(|row| collapsed.borrow().contains(&row.0)) { "Reveal" } else { "Collapse" });
            }
            glib::idle_add_local_once(move || adjustment.set_value(scroll.min((adjustment.upper()-adjustment.page_size()).max(0.0))));
            for (buffer, selection) in buffers.iter().zip(saved) {
                if let Some((a,z)) = selection { buffer.select_range(&buffer.iter_at_offset(a.min(buffer.char_count())), &buffer.iter_at_offset(z.min(buffer.char_count()))); }
            }
        })
    };
    render();
    // Disconnect when the inspector is destroyed; expanding keeps the same view.
    let weak = root.downgrade();
    let redraw = render.clone();
    let signal = source.connect_changed(move |_| { if weak.upgrade().is_some() { redraw(); } });
    let source_clone = source.clone();
    let signal = RefCell::new(Some(signal));
    root.connect_destroy(move |_| { if let Some(signal) = signal.borrow_mut().take() { source_clone.disconnect(signal); } });
    for toggle in [&split, &context] { let render = render.clone(); toggle.connect_toggled(move |_| render()); }
    {
        let (left,right) = (left.clone(),right.clone());
        wrap.connect_toggled(move |button| {
            let mode = if button.is_active() { gtk::WrapMode::WordChar } else { gtk::WrapMode::None };
            left.set_wrap_mode(mode); right.set_wrap_mode(mode);
        });
    }
    {
        let (rows, collapsed, picker, render) = (hunk_rows.clone(), collapsed.clone(), hunk_picker.downgrade(), render.clone());
        fold.connect_clicked(move |_| {
            let Some(picker) = picker.upgrade() else { return; };
            let Some(key) = rows.borrow().get(picker.selected() as usize).map(|row| row.0.clone()) else { return; };
            let revealed = collapsed.borrow_mut().remove(&key);
            if !revealed { collapsed.borrow_mut().insert(key); }
            render();
        });
    }
    {
        let (rows, collapsed, fold, left) = (hunk_rows.clone(), collapsed.clone(), fold.downgrade(), left.clone());
        hunk_picker.connect_selected_notify(move |picker| {
            let Some(fold) = fold.upgrade() else { return; };
            let rows = rows.borrow();
            let current = rows.get(picker.selected() as usize);
            fold.set_sensitive(current.is_some());
            fold.set_label(if current.is_some_and(|row| collapsed.borrow().contains(&row.0)) { "Reveal" } else { "Collapse" });
            if let Some((_,line)) = current {
                let (left,line) = (left.clone(),*line);
                glib::idle_add_local_once(move || { if let Some(mut iter) = left.buffer().iter_at_line(line) { left.scroll_to_iter(&mut iter,0.1,true,0.0,0.1); } });
            }
        });
    }
    for (button,direction) in [(previous,-1_i32),(next,1)] {
        let (left, rows, collapsed, picker, render) = (left.clone(), hunk_rows.clone(), collapsed.clone(), hunk_picker.clone(), render.clone());
        button.connect_clicked(move |_| {
            let len = rows.borrow().len();
            if len == 0 { return; }
            let index = (picker.selected() as i32 + direction).rem_euclid(len as i32) as usize;
            picker.set_selected(index as u32);
            let key = rows.borrow()[index].0.clone();
            collapsed.borrow_mut().remove(&key);
            render();
            let line = rows.borrow()[index].1;
            let left = left.clone();
            glib::idle_add_local_once(move || {
                if let Some(mut iter) = left.buffer().iter_at_line(line) { left.scroll_to_iter(&mut iter,0.1,true,0.0,0.1); }
            });
        });
    }
    root
}
