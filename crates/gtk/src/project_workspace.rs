use super::*;
use manager_tools::Project;

fn status_pill(project: &Project) -> String {
    let mut pill = project.branch.clone();
    if !pill.is_empty() && !project.target.is_empty() {
        pill.push_str(&format!(" → {}", project.target));
    }
    if project.ahead > 0 || project.behind > 0 {
        pill.push_str(&format!(" ⇡{} ⇣{}", project.ahead, project.behind));
    }
    pill
}

fn health_text(project: &Project) -> String {
    if project.conflicted > 0 {
        return format!(
            "✖ {} conflicted · {} dirty",
            project.conflicted, project.dirty
        );
    }
    if project.dirty == 0 && project.untracked == 0 {
        return "clean".into();
    }
    if project.untracked > 0 && project.dirty == 0 {
        return format!("{} untracked", project.untracked);
    }
    format!("●{} · {} untracked", project.dirty, project.untracked)
}

fn cache_bytes(project: &Project) -> u64 {
    project
        .artifacts
        .iter()
        .filter_map(|(_, bytes)| *bytes)
        .sum()
}

fn cache_text(project: &Project) -> String {
    if project.artifacts.is_empty() {
        return "—".into();
    }
    let bytes = cache_bytes(project);
    if bytes == 0 && project.artifacts.iter().any(|(_, bytes)| bytes.is_none()) {
        return format!("{} dirs", project.artifacts.len());
    }
    actions::human_size(bytes)
}

fn toolchain_text(project: &Project) -> String {
    let mut kinds = Vec::new();
    if project.rust {
        kinds.push("Rust".to_owned());
    }
    if project.node {
        kinds.push(if project.web_tool.is_empty() {
            "JS".into()
        } else {
            project.web_tool.clone()
        });
    }
    if project.git {
        kinds.push("Git".into());
    }
    if kinds.is_empty() {
        kinds.push("local".into());
    }
    kinds.join(" · ")
}

fn parent_text(project: &Project) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let parent = project.path.parent().unwrap_or(&project.path);
    let text = match parent.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => parent.display().to_string(),
    };
    // Keep the tail: "…/Repos/.worktrees" beats "/mnt/Windows11/DEV_PR…".
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    } else {
        text
    }
}

pub fn new(
    window: &gtk::ApplicationWindow,
    state: Rc<RefCell<State>>,
    open: impl Fn(PathBuf) + 'static,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("megaman-projects");
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    toolbar.add_css_class("megaman-project-header");
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search projects, branches, tools…")
        .width_chars(32)
        .build();
    toolbar.append(&search);
    let status = gtk::Label::new(Some("Opening project index…"));
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.add_css_class("dim-label");
    toolbar.append(&status);
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some(
        "Refresh projects (fast — does not rebuild the file index)",
    ));
    toolbar.append(&refresh);
    {
        let state = state.clone();
        let window = window.clone();
        let button = refresh.clone();
        let status = status.clone();
        refresh.connect_clicked(move |_| {
            // Projects-only refresh. Rebuilding the whole Catalog here
            // froze the app for minutes on large Mounts.
            button.set_sensitive(false);
            status.set_text("Refreshing projects…");
            manager_tools::refresh_project_account();
            if let Some(catalog) = state.borrow().catalog.clone() {
                state.borrow().storage.refresh_projects(catalog, true);
            } else {
                status.set_text("No file index yet — rebuilding Catalog…");
                start_rebuild(&state, &window, true);
            }
        });
    }
    root.append(&toolbar);

    let projects: Rc<RefCell<Vec<Project>>> = Rc::new(RefCell::new(Vec::new()));
    let model = gio::ListStore::new::<RowData>();
    let sorted = gtk::SortListModel::new(Some(model.clone()), None::<gtk::Sorter>);
    let selection = gtk::SingleSelection::new(Some(sorted.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let table = gtk::ColumnView::new(Some(selection.clone()));
    table.set_vexpand(true);
    table.add_css_class("megaman-project-table");
    let storage = state.borrow().storage.clone();
    for (column, width) in [
        ("Project", 210),
        ("Branch", 190),
        ("Changes", 110),
        ("Caches", 90),
        ("Worktrees", 80),
        ("Last commit", 170),
        ("Toolchain", 110),
        ("Repository", 150),
        ("Location", 220),
        ("Indexed size", 90),
        ("Modified", 100),
        ("Builds / caches", 130),
    ] {
        let factory = if column == "Indexed size" {
            surface::make_size_factory(Rc::new(Cell::new(false)), storage.clone())
        } else {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_margin_start(8);
                label.set_margin_end(8);
                label.set_margin_top(13);
                label.set_margin_bottom(13);
                if column == "Project" {
                    label.add_css_class("heading");
                }
                item.set_child(Some(&label));
            });
            let projects = projects.clone();
            factory.connect_bind(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(data) = item.item().and_downcast::<RowData>() else {
                    return;
                };
                let Some(label) = item.child().and_downcast::<gtk::Label>() else {
                    return;
                };
                let projects = projects.borrow();
                let path = data.path();
                let Some(project) = projects
                    .iter()
                    .find(|project| project.path == Path::new(&path))
                else {
                    return;
                };
                let text = match column {
                    "Project" => format!("{}\n{}", data.name(), parent_text(project)),
                    "Branch" => {
                        if project.branch.is_empty() {
                            "No branch".into()
                        } else {
                            status_pill(project)
                        }
                    }
                    "Changes" => {
                        if project.conflicted > 0 {
                            format!("Conflicts: {}", project.conflicted)
                        } else if project.dirty + project.untracked > 0 {
                            format!("{} changed", project.dirty + project.untracked)
                        } else {
                            "Clean".into()
                        }
                    }
                    "Caches" => cache_text(project),
                    "Worktrees" => project.worktrees.len().max(1).to_string(),
                    "Last commit" => {
                        if project.last_commit.is_empty() {
                            "—".into()
                        } else {
                            project.last_commit.clone()
                        }
                    }
                    "Toolchain" => toolchain_text(project),
                    "Repository" => project.repository.clone(),
                    "Location" => project.path.to_string_lossy().into_owned(),
                    "Modified" => {
                        if project.modified > 0 {
                            actions::human_mtime(project.modified)
                        } else {
                            "—".into()
                        }
                    }
                    _ => {
                        if project.artifacts.is_empty() {
                            "No local artifacts".into()
                        } else {
                            project
                                .artifacts
                                .iter()
                                .map(|(path, _)| {
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                })
                                .collect::<Vec<_>>()
                                .join(" · ")
                        }
                    }
                };
                label.set_text(&text);
                label.set_tooltip_text(Some(&format!(
                    "{}\n{}\n{}",
                    project.path.to_string_lossy(),
                    status_pill(project),
                    health_text(project)
                )));
                if column == "Changes" && (project.conflicted > 0 || project.dirty > 0) {
                    label.add_css_class("qfind-dirty");
                } else {
                    label.remove_css_class("qfind-dirty");
                }
            });
            factory
        };
        let col = gtk::ColumnViewColumn::new(
            Some(if column == "Indexed size" {
                "Size"
            } else {
                column
            }),
            Some(factory),
        );
        col.set_resizable(true);
        col.set_visible(matches!(
            column,
            "Project" | "Branch" | "Changes" | "Last commit"
        ));
        col.set_fixed_width(width);
        col.set_expand(column == "Project");
        col.connect_fixed_width_notify(|column| column.set_expand(false));
        let records = projects.clone();
        let storage = storage.clone();
        col.set_sorter(Some(&gtk::CustomSorter::new(move |a, b| {
            let (Some(a), Some(b)) = (a.downcast_ref::<RowData>(), b.downcast_ref::<RowData>())
            else {
                return gtk::Ordering::Equal;
            };
            let records = records.borrow();
            let (ap, bp) = (a.path(), b.path());
            let (Some(pa), Some(pb)) = (
                records.iter().find(|p| p.path == Path::new(&ap)),
                records.iter().find(|p| p.path == Path::new(&bp)),
            ) else {
                return gtk::Ordering::Equal;
            };
            let order = match column {
                "Indexed size" => storage
                    .known_size(&pa.path)
                    .cmp(&storage.known_size(&pb.path)),
                "Modified" => pa.modified.cmp(&pb.modified),
                "Toolchain" => toolchain_text(pa).cmp(&toolchain_text(pb)),
                "Branch" => (pa.branch.clone(), pa.ahead, pa.behind).cmp(&(
                    pb.branch.clone(),
                    pb.ahead,
                    pb.behind,
                )),
                "Changes" => (pa.conflicted, pa.dirty, pa.untracked).cmp(&(
                    pb.conflicted,
                    pb.dirty,
                    pb.untracked,
                )),
                "Caches" => cache_bytes(pa).cmp(&cache_bytes(pb)),
                "Worktrees" => pa.worktrees.len().cmp(&pb.worktrees.len()),
                "Last commit" => pa.last_commit.cmp(&pb.last_commit),
                "Repository" => pa
                    .repository
                    .to_lowercase()
                    .cmp(&pb.repository.to_lowercase()),
                "Location" => ap.cmp(&bp),
                "Builds / caches" => pa.artifacts.len().cmp(&pb.artifacts.len()),
                _ => a.name().to_lowercase().cmp(&b.name().to_lowercase()),
            };
            order.then_with(|| ap.cmp(&bp)).into()
        })));
        table.append_column(&col);
    }
    sorted.set_sorter(table.sorter().as_ref());
    let first = table
        .columns()
        .item(0)
        .and_downcast::<gtk::ColumnViewColumn>();
    table.sort_by_column(first.as_ref(), gtk::SortType::Ascending);
    toolbar.append(&columns::configure(&table, "projects"));
    let details = Rc::new(RefCell::new(HashMap::<PathBuf, (gtk::Box, gtk::Box)>::new()));
    let open = Rc::new(open);
    {
        let open = open.clone();
        let selection = selection.clone();
        table.connect_activate(move |_, position| {
            if let Some(row) = selection.item(position).and_downcast::<RowData>() {
                open(PathBuf::from(row.path()));
            }
        });
    }
    let table_scroll = gtk::ScrolledWindow::builder()
        .child(&table)
        .vexpand(true)
        .build();
    let project_list = gtk::Stack::new();
    project_list.set_vexpand(true);
    project_list.add_named(&table_scroll, Some("projects"));
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 12);
    empty.set_valign(gtk::Align::Center);
    empty.set_halign(gtk::Align::Center);
    let empty_icon = gtk::Image::from_icon_name("folder-saved-search-symbolic");
    empty_icon.set_pixel_size(48);
    empty_icon.add_css_class("dim-label");
    empty.append(&empty_icon);
    let empty_title = gtk::Label::new(Some("Opening your workspace"));
    empty_title.add_css_class("title-3");
    empty.append(&empty_title);
    let empty_hint = gtk::Label::new(Some("Reading repositories from your file index…"));
    empty_hint.set_wrap(true);
    empty_hint.set_max_width_chars(38);
    empty_hint.set_justify(gtk::Justification::Center);
    empty_hint.add_css_class("dim-label");
    empty.append(&empty_hint);
    project_list.add_named(&empty, Some("empty"));
    project_list.set_visible_child_name("empty");
    let inspector = gtk::Box::new(gtk::Orientation::Vertical, 10);
    inspector.add_css_class("qfind-inspector");
    inspector.set_width_request(420);
    inspector.add_css_class("megaman-project-inspector");
    let heading = gtk::Label::new(Some("Select a project"));
    heading.add_css_class("megaman-inspector-title");
    heading.set_margin_top(18);
    heading.set_xalign(0.0);
    heading.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    inspector.append(&heading);
    let path_label = gtk::Label::new(Some(
        "Branches, worktrees, caches and sync for one repository.",
    ));
    path_label.set_wrap(true);
    path_label.set_xalign(0.0);
    path_label.set_max_width_chars(44);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path_label.add_css_class("dim-label");
    inspector.append(&path_label);
    let checkout = gtk::DropDown::from_strings(&[]);
    let checkout_factory = gtk::SignalListItemFactory::new();
    checkout_factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_max_width_chars(32);
        item.set_child(Some(&label));
    });
    checkout_factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(text) = item.item().and_downcast::<gtk::StringObject>() else {
            return;
        };
        let Some(label) = item.child().and_downcast::<gtk::Label>() else {
            return;
        };
        label.set_text(&text.string());
        label.set_tooltip_text(Some(&text.string()));
    });
    checkout.set_factory(Some(&checkout_factory));
    checkout.set_list_factory(Some(&checkout_factory));
    checkout.set_tooltip_text(Some("Choose this repository's local checkout or worktree"));
    checkout.set_visible(false);
    inspector.append(&checkout);

    // Action bar: the GitButler-style verbs for the selected checkout.
    let project_path = Rc::new(RefCell::new(None::<PathBuf>));
    let actions_bar = gtk::FlowBox::new();
    actions_bar.set_selection_mode(gtk::SelectionMode::None);
    actions_bar.set_column_spacing(6);
    actions_bar.set_row_spacing(6);
    actions_bar.set_max_children_per_line(4);
    actions_bar.add_css_class("megaman-actions");
    let mut buttons = Vec::new();
    let mut button = |label: &str, tip: &str| {
        let button = gtk::Button::with_label(label);
        button.set_tooltip_text(Some(tip));
        button.set_sensitive(false);
        actions_bar.insert(&button, -1);
        buttons.push(button.clone());
        button
    };
    let open_files = button("Open files", "Browse this checkout (Enter)");
    open_files.add_css_class("suggested-action");
    let terminal = button("Terminal", "Open $TERMINAL here");
    let fetch = button("Fetch", "git fetch --all --prune");
    let pull = button("Pull", "git pull --ff-only");
    let push = button("Push", "git push (sets upstream when missing)");
    let merge = button("Merge ↓", "Merge this branch into its target");
    let add_worktree = button("New worktree…", "git worktree add ../<name> -b <name>");
    let remove_worktree = button(
        "Remove worktree",
        "git worktree remove (refuses dirty trees)",
    );
    inspector.append(&actions_bar);

    let command_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let command = gtk::Entry::new();
    command.set_placeholder_text(Some("git …  (e.g. status, log -3, switch main)"));
    command.set_hexpand(true);
    command.set_sensitive(false);
    command_row.append(&command);
    inspector.append(&command_row);
    let output_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    output_area.add_css_class("megaman-command-output");
    output_area.set_size_request(-1, 120);
    let output = manager_tools::text_view(&output_area);
    output.set_text("Pick a project to see its git state.");
    inspector.append(&output_area);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_margin_bottom(12);
    let section = |title: &str| {
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.0);
        label.set_margin_top(14);
        label.add_css_class("megaman-eyebrow");
        label
    };
    let worktree_head = section("WORKTREES");
    page.append(&worktree_head);
    let worktrees = gtk::ListBox::new();
    worktrees.add_css_class("boxed-list");
    worktrees.set_selection_mode(gtk::SelectionMode::None);
    page.append(&worktrees);
    page.append(&section("OVERVIEW & TASKS"));
    let overview = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.append(&overview);
    page.append(&section("BUILDS & CACHES"));
    let caches = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.append(&caches);
    page.append(&section("CHANGES"));
    let (changes, _) = git_panel::new(state.clone(), Some(project_path.clone()));
    page.append(&changes);
    page.set_visible(false);
    let detail_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&page)
        .vexpand(true)
        .build();
    inspector.append(&detail_scroll);
    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&project_list));
    split.set_end_child(Some(&inspector));
    split.set_position(620);
    split.set_vexpand(true);
    split.set_resize_end_child(true);
    split.set_shrink_start_child(true);
    split.set_shrink_end_child(false);
    root.append(&split);
    status.add_css_class("dim-label");
    root.append(&status);

    // Every git verb funnels through here: run, show output, re-read projects.
    let run_git: Rc<dyn Fn(PathBuf, Vec<String>)> = {
        let output = output.clone();
        let state = state.clone();
        let buttons = buttons.clone();
        Rc::new(move |dir: PathBuf, args: Vec<String>| {
            output.set_text(&format!("$ git {}\n…", args.join(" ")));
            for button in &buttons {
                button.set_sensitive(false);
            }
            let (output, state, buttons) = (output.clone(), state.clone(), buttons.clone());
            glib::MainContext::default().spawn_local(async move {
                let result = gio::spawn_blocking(move || {
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let text = qfind_core::components::git(&dir, &refs, None);
                    (refs.join(" "), text)
                })
                .await;
                let (line, text) = match result {
                    Ok(pair) => pair,
                    Err(_) => (String::new(), Err("git worker failed".into())),
                };
                let body = match text {
                    Ok(ok) if ok.trim().is_empty() => "done".to_owned(),
                    Ok(ok) => ok,
                    Err(error) => error,
                };
                output.set_text(&format!("$ git {line}\n{body}"));
                for button in &buttons {
                    button.set_sensitive(true);
                }
                manager_tools::refresh_project_account();
                if let Some(catalog) = state.borrow().catalog.clone() {
                    state.borrow().storage.refresh_projects(catalog, true);
                }
            });
        })
    };
    {
        let selected = project_path.clone();
        let open = open.clone();
        open_files.connect_clicked(move |_| {
            if let Some(path) = selected.borrow().clone() {
                open(path);
            }
        });
    }
    {
        let selected = project_path.clone();
        let state = state.clone();
        terminal.connect_clicked(move |_| {
            if let Some(path) = selected.borrow().clone() {
                open_terminal_at(&state, path);
            }
        });
    }
    for (button, args) in [
        (&fetch, vec!["fetch", "--all", "--prune"]),
        (&pull, vec!["pull", "--ff-only"]),
    ] {
        let selected = project_path.clone();
        let run_git = run_git.clone();
        button.connect_clicked(move |_| {
            if let Some(path) = selected.borrow().clone() {
                run_git(path, args.iter().map(|s| s.to_string()).collect());
            }
        });
    }
    let current = Rc::new(RefCell::new(None::<Project>));
    {
        let current = current.clone();
        let run_git = run_git.clone();
        push.connect_clicked(move |_| {
            let Some(project) = current.borrow().clone() else {
                return;
            };
            let args = if project.target.is_empty() && !project.branch.is_empty() {
                vec![
                    "push".into(),
                    "-u".into(),
                    "origin".into(),
                    project.branch.clone(),
                ]
            } else {
                vec!["push".into()]
            };
            run_git(project.path, args);
        });
    }
    {
        let current = current.clone();
        let projects = projects.clone();
        let run_git = run_git.clone();
        let window = window.clone();
        merge.connect_clicked(move |_| {
            let Some(project) = current.borrow().clone() else {
                return;
            };
            let target = project
                .target
                .strip_prefix("origin/")
                .unwrap_or(&project.target)
                .to_owned();
            if target.is_empty() || project.branch.is_empty() || project.branch == target {
                return;
            }
            // Prefer a sibling worktree already on the target branch; else switch in place.
            let sibling = projects
                .borrow()
                .iter()
                .find(|p| {
                    p.branch == target
                        && (p.path == project.path
                            || project.worktrees.contains(&p.path)
                            || p.worktrees.contains(&project.path))
                })
                .map(|p| p.path.clone());
            let (dir, args, note) = match sibling {
                Some(dir) => (
                    dir.clone(),
                    vec![
                        "merge".to_owned(),
                        "--no-ff".to_owned(),
                        project.branch.clone(),
                    ],
                    format!("in {}", dir.display()),
                ),
                None => (
                    project.path.clone(),
                    vec!["switch".to_owned(), target.clone()],
                    "after switching this checkout".to_owned(),
                ),
            };
            let dialog = gtk::AlertDialog::builder()
                .message(format!("Merge {} into {target}?", project.branch))
                .detail(format!("git merge --no-ff {} {note}", project.branch))
                .buttons(["Cancel", "Merge"])
                .cancel_button(0)
                .default_button(1)
                .build();
            let run_git = run_git.clone();
            let branch = project.branch.clone();
            let switching = args.first().is_some_and(|word| word == "switch");
            dialog.choose(Some(&window), None::<&gio::Cancellable>, move |choice| {
                if choice != Ok(1) {
                    return;
                }
                if switching {
                    // Two steps: switch, then merge. The second runs once the first has reported.
                    let run_git = run_git.clone();
                    let dir_for_merge = dir.clone();
                    run_git(dir, args);
                    glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                        run_git(
                            dir_for_merge,
                            vec!["merge".into(), "--no-ff".into(), branch],
                        )
                    });
                } else {
                    run_git(dir, args);
                }
            });
        });
    }
    {
        let current = current.clone();
        let run_git = run_git.clone();
        let window = window.clone();
        add_worktree.connect_clicked(move |_| {
            let Some(project) = current.borrow().clone() else {
                return;
            };
            let run_git = run_git.clone();
            prompt_text(&window, "New worktree branch", "", move |name| {
                let name = name.trim().trim_matches('/').to_owned();
                if name.is_empty() {
                    return;
                }
                let dest = format!("../{}", name.rsplit('/').next().unwrap_or(&name));
                run_git(
                    project.path.clone(),
                    vec!["worktree".into(), "add".into(), dest, "-b".into(), name],
                );
            });
        });
    }
    let remove_tree: Rc<dyn Fn(PathBuf, PathBuf)> = {
        let run_git = run_git.clone();
        let window = window.clone();
        Rc::new(move |repo: PathBuf, tree: PathBuf| {
            let dialog = gtk::AlertDialog::builder()
                .message(format!(
                    "Remove worktree {}?",
                    tree.file_name().unwrap_or_default().to_string_lossy()
                ))
                .detail(format!(
                    "{}\nRefused when it has uncommitted changes. The branch stays.",
                    tree.display()
                ))
                .buttons(["Cancel", "Remove"])
                .cancel_button(0)
                .default_button(1)
                .build();
            let run_git = run_git.clone();
            dialog.choose(Some(&window), None::<&gio::Cancellable>, move |choice| {
                if choice == Ok(1) {
                    run_git(
                        repo,
                        vec![
                            "worktree".into(),
                            "remove".into(),
                            tree.to_string_lossy().into_owned(),
                        ],
                    );
                }
            });
        })
    };
    {
        let current = current.clone();
        let remove_tree = remove_tree.clone();
        remove_worktree.connect_clicked(move |_| {
            let Some(project) = current.borrow().clone() else {
                return;
            };
            let main = project
                .worktrees
                .first()
                .cloned()
                .unwrap_or_else(|| project.path.clone());
            remove_tree(main, project.path);
        });
    }
    {
        let selected = project_path.clone();
        let run_git = run_git.clone();
        command.connect_activate(move |entry| {
            let Some(path) = selected.borrow().clone() else {
                return;
            };
            let text = entry.text();
            let words: Vec<String> = text
                .trim()
                .strip_prefix("git ")
                .unwrap_or(text.trim())
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            if words.is_empty() {
                return;
            }
            entry.set_text("");
            run_git(path, words);
        });
    }
    {
        let window = window.clone();
        let state = state.clone();
        let details = details.clone();
        let projects_for_render = projects.clone();
        let render = Rc::new(move |project: Option<Project>| {
            *current.borrow_mut() = project.clone();
            let Some(project) = project else {
                *project_path.borrow_mut() = None;
                for button in &buttons {
                    button.set_sensitive(false);
                }
                command.set_sensitive(false);
                heading.set_text("Select a project");
                path_label.set_text("Branches, worktrees, caches and sync for one repository.");
                output.set_text("Pick a project to see its git state.");
                page.set_visible(false);
                return;
            };
            page.set_visible(true);
            *project_path.borrow_mut() = Some(project.path.clone());
            let title = if project.repository.is_empty() {
                project
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| project.path.to_string_lossy().into_owned())
            } else {
                project
                    .repository
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            };
            heading.set_text(&title);
            path_label.set_text(&project.path.to_string_lossy());
            path_label.set_tooltip_text(Some(&project.path.to_string_lossy()));
            for button in &buttons {
                button.set_sensitive(project.git);
            }
            open_files.set_sensitive(true);
            terminal.set_sensitive(true);
            command.set_sensitive(project.git);
            let target = project
                .target
                .strip_prefix("origin/")
                .unwrap_or(&project.target);
            merge.set_sensitive(
                project.git
                    && !target.is_empty()
                    && !project.branch.is_empty()
                    && project.branch != target,
            );
            merge.set_label(&if target.is_empty() {
                "Merge ↓".to_owned()
            } else {
                format!("Merge → {target}")
            });
            let is_linked = project
                .worktrees
                .first()
                .is_some_and(|main| main != &project.path);
            remove_worktree.set_sensitive(project.git && is_linked);
            output.set_text(&format!(
                "{}\n{}\n{}",
                status_pill(&project),
                health_text(&project),
                if project.last_commit.is_empty() {
                    "no commits"
                } else {
                    &project.last_commit
                }
            ));
            while let Some(child) = worktrees.first_child() {
                worktrees.remove(&child);
            }
            let known = projects_for_render.borrow();
            let trees: Vec<PathBuf> = if project.worktrees.is_empty() {
                vec![project.path.clone()]
            } else {
                project.worktrees.clone()
            };
            for tree in &trees {
                let sibling = known.iter().find(|p| &p.path == tree);
                let branch = sibling
                    .map(|p| p.branch.clone())
                    .filter(|b| !b.is_empty())
                    .unwrap_or_else(|| "?".into());
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.set_margin_start(10);
                row.set_margin_end(6);
                row.set_margin_top(6);
                row.set_margin_bottom(6);
                let text = gtk::Label::new(Some(&format!(
                    "{branch}  ·  {}",
                    tree.file_name().unwrap_or_default().to_string_lossy()
                )));
                text.set_xalign(0.0);
                text.set_hexpand(true);
                text.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                text.set_tooltip_text(Some(&tree.to_string_lossy()));
                if tree == &project.path {
                    text.add_css_class("heading");
                }
                if let Some(sibling) = sibling.filter(|s| s.dirty + s.untracked > 0) {
                    text.set_text(&format!("{}  ·  {}", text.text(), health_text(sibling)));
                }
                row.append(&text);
                let open_button = gtk::Button::from_icon_name("folder-open-symbolic");
                open_button.add_css_class("flat");
                open_button.set_tooltip_text(Some("Open files"));
                {
                    let open = open.clone();
                    let tree = tree.clone();
                    open_button.connect_clicked(move |_| open(tree.clone()));
                }
                row.append(&open_button);
                if Some(tree) != trees.first() {
                    let remove_button = gtk::Button::from_icon_name("user-trash-symbolic");
                    remove_button.add_css_class("flat");
                    remove_button.set_tooltip_text(Some("Remove worktree"));
                    let (remove_tree, main, tree) =
                        (remove_tree.clone(), trees[0].clone(), tree.clone());
                    remove_button.connect_clicked(move |_| remove_tree(main.clone(), tree.clone()));
                    row.append(&remove_button);
                }
                worktrees.append(&row);
            }
            worktree_head.set_text(&format!("WORKTREES · {}", trees.len()));
            drop(known);
            while let Some(child) = overview.first_child() {
                overview.remove(&child);
            }
            while let Some(child) = caches.first_child() {
                caches.remove(&child);
            }
            let panels = details
                .borrow_mut()
                .entry(project.path.clone())
                .or_insert_with(|| {
                    (
                        manager_tools::project_detail_content(&window, &state, project.clone()),
                        manager_tools::project_content_at(&window, &state, project.path.clone()),
                    )
                })
                .clone();
            overview.append(&panels.0);
            caches.append(&panels.1);
        });
        let choices = Rc::new(RefCell::new(Vec::<Project>::new()));
        let changing = Rc::new(Cell::new(false));
        let remembered = Rc::new(RefCell::new(HashMap::<String, PathBuf>::new()));
        {
            let choices = choices.clone();
            let changing = changing.clone();
            let remembered = remembered.clone();
            let render = render.clone();
            checkout.connect_selected_notify(move |checkout| {
                if changing.get() {
                    return;
                }
                let project = choices.borrow().get(checkout.selected() as usize).cloned();
                if let Some(project) = &project {
                    remembered
                        .borrow_mut()
                        .insert(project.repository.clone(), project.path.clone());
                }
                render(project);
            });
        }
        let projects = projects.clone();
        selection.connect_selected_item_notify(move |selection| {
            let selected = selection
                .selected_item()
                .and_downcast::<RowData>()
                .and_then(|row| {
                    let path = row.path();
                    projects
                        .borrow()
                        .iter()
                        .find(|project| project.path == Path::new(&path))
                        .cloned()
                });
            changing.set(true);
            // Group linked worktrees: same non-empty repository, else the
            // explicit worktree list from the backend. Local-only checkouts
            // never group together.
            let mut items: Vec<_> = selected
                .as_ref()
                .map(|selected| {
                    if selected.repository.is_empty() {
                        vec![selected.clone()]
                    } else {
                        projects
                            .borrow()
                            .iter()
                            .filter(|project| {
                                !project.repository.is_empty()
                                    && project
                                        .repository
                                        .eq_ignore_ascii_case(&selected.repository)
                            })
                            .cloned()
                            .collect()
                    }
                })
                .unwrap_or_default();
            // Prefer the backend's sibling list when available.
            if let Some(active) = selected
                .as_ref()
                .filter(|active| !active.worktrees.is_empty())
            {
                {
                    let known: HashSet<PathBuf> =
                        items.iter().map(|project| project.path.clone()).collect();
                    for sibling in &active.worktrees {
                        if !known.contains(sibling)
                            && let Some(extra) = projects
                                .borrow()
                                .iter()
                                .find(|project| &project.path == sibling)
                                .cloned()
                        {
                            items.push(extra);
                        }
                    }
                }
            }
            items.sort_by(|a, b| a.path.cmp(&b.path));
            items.dedup_by(|a, b| a.path == b.path);
            let labels: Vec<_> = items
                .iter()
                .map(|project| {
                    format!(
                        "{} · {} · {}",
                        project.branch,
                        health_text(project),
                        project.path.display()
                    )
                })
                .collect();
            let model =
                gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
            let position = selected
                .as_ref()
                .and_then(|selected| {
                    remembered
                        .borrow()
                        .get(&selected.repository)
                        .cloned()
                        .or_else(|| Some(selected.path.clone()))
                })
                .and_then(|path| items.iter().position(|project| project.path == path))
                .unwrap_or(0);
            let active = items.get(position).cloned();
            *choices.borrow_mut() = items;
            checkout.set_model(Some(&model));
            checkout.set_selected(position as u32);
            checkout.set_visible(model.n_items() > 1);
            changing.set(false);
            render(active);
        });
    }
    let weak = root.downgrade();
    let mut last = None;
    let refresh_button = refresh.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(root) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if !root.is_mapped() {
            return glib::ControlFlow::Continue;
        }
        if let Some(error) = storage.project_error() {
            status.set_text(&error);
            empty_title.set_text("Projects are unavailable");
            empty_hint.set_text(&error);
            project_list.set_visible_child_name("empty");
            model.remove_all();
            last = None;
            refresh_button.set_sensitive(true);
            return glib::ControlFlow::Continue;
        }
        let key = (search.text().to_lowercase(), storage.catalog_revision());
        if last.as_ref() == Some(&key) {
            return glib::ControlFlow::Continue;
        }
        let Some(mut items) = storage.projects(Path::new("/")) else {
            if model.n_items() > 0 {
                model.remove_all();
            }
            status.set_text("Reading local repositories and worktrees…");
            return glib::ControlFlow::Continue;
        };
        // Preserve command output while browsing; refresh invalidates stale metadata.
        if last
            .as_ref()
            .is_some_and(|previous: &(String, u64)| previous.1 != key.1)
        {
            details.borrow_mut().clear();
        }
        *projects.borrow_mut() = items.clone();
        items.retain(|project| {
            let hay = format!(
                "{} {} {} {} {}",
                project.path.to_string_lossy().to_lowercase(),
                project.repository.to_lowercase(),
                project.branch.to_lowercase(),
                project.last_commit.to_lowercase(),
                toolchain_text(project).to_lowercase()
            );
            hay.contains(&key.0)
        });
        {
            items.sort_by_key(|project| {
                (
                    project.path.components().any(|part| {
                        matches!(part.as_os_str().to_str(), Some("actions-runners" | "_work"))
                    }),
                    !matches!(project.branch.as_str(), "main" | "master"),
                    project.path.components().count(),
                )
            });
            // One row per checkout path: linked worktrees stay visible.
            let mut seen = HashSet::new();
            items.retain(|project| seen.insert(project.path.clone()));
        }
        let dirty = items
            .iter()
            .filter(|project| project.dirty > 0 || project.conflicted > 0 || project.untracked > 0)
            .count();
        let conflicts = items
            .iter()
            .filter(|project| project.conflicted > 0)
            .count();
        let outputs: usize = items.iter().map(|project| project.artifacts.len()).sum();
        status.set_text(&format!(
            "{} projects · {} with changes · {} conflicts · {} caches",
            items.len(),
            dirty,
            conflicts,
            outputs
        ));
        if items.is_empty() {
            empty_title.set_text(if key.0.is_empty() {
                "Your next project starts here"
            } else {
                "No matching projects"
            });
            empty_hint.set_text(if key.0.is_empty() {
                "Refresh to discover repositories in your index."
            } else {
                "Try another project name, branch, or toolchain."
            });
            project_list.set_visible_child_name("empty");
        } else {
            project_list.set_visible_child_name("projects");
        }
        let rows: Vec<_> = items
            .iter()
            .map(|project| {
                let name = if project.repository.is_empty() {
                    project
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| project.path.to_string_lossy().into_owned())
                } else {
                    project
                        .repository
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                };
                RowData::new(
                    name,
                    project.path.to_string_lossy(),
                    true,
                    0,
                    project.modified,
                )
            })
            .collect();
        let selected_path = selection
            .selected_item()
            .and_downcast::<RowData>()
            .map(|row| row.path());
        model.splice(0, model.n_items(), &rows);
        let position = (0..sorted.n_items()).find(|&position| {
            sorted
                .item(position)
                .and_downcast::<RowData>()
                .is_some_and(|row| Some(row.path()) == selected_path)
        });
        if let Some(position) = position.or_else(|| (sorted.n_items() > 0).then_some(0)) {
            selection.set_selected(position);
        }
        last = Some(key);
        refresh_button.set_sensitive(true);
        glib::ControlFlow::Continue
    });
    root
}
