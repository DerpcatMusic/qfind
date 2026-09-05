use super::*;
use manager_tools::Project;

pub fn new(window: &gtk::ApplicationWindow, state: Rc<RefCell<State>>, open: impl Fn(PathBuf) + 'static) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    toolbar.add_css_class("qfind-address");
    let title = gtk::Label::new(Some(qfind_core::components::title("projects")));
    title.add_css_class("title-3");
    toolbar.append(&title);
    let search = gtk::SearchEntry::builder().placeholder_text("Find a project…").hexpand(true).build();
    toolbar.append(&search);
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh the project and file index"));
    toolbar.append(&refresh);
    {
        let state = state.clone();
        let window = window.clone();
        refresh.connect_clicked(move |_| {
            manager_tools::refresh_project_account();
            start_rebuild(&state, &window, true);
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
    let storage = state.borrow().storage.clone();
    for (column, width) in [("Project", 180), ("Repository", 180), ("Branch", 115), ("Location", 220), ("Kind", 70), ("Indexed size", 90), ("Modified", 100), ("Builds / caches", 130)] {
        let factory = if column == "Indexed size" {
            surface::make_size_factory(Rc::new(Cell::new(false)), storage.clone())
        } else {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return; };
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_margin_start(8);
                label.set_margin_end(8);
                label.set_margin_top(4);
                label.set_margin_bottom(4);
                item.set_child(Some(&label));
            });
            let projects = projects.clone();
            factory.connect_bind(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return; };
                let Some(data) = item.item().and_downcast::<RowData>() else { return; };
                let Some(label) = item.child().and_downcast::<gtk::Label>() else { return; };
                let projects = projects.borrow();
                let path = data.path();
                let Some(project) = projects.iter().find(|project| project.path == Path::new(&path)) else { return; };
                let text = match column {
                    "Project" => data.name(),
                    "Repository" => project.repository.clone(),
                    "Branch" => project.branch.clone(),
                    "Location" => project.path.to_string_lossy().into_owned(),
                    "Kind" => [project.rust.then_some("Rust"), project.node.then_some("JS"), project.git.then_some("Git")].into_iter().flatten().collect::<Vec<_>>().join(" · "),
                    "Modified" => if project.modified > 0 { actions::human_mtime(project.modified) } else { "—".into() },
                    _ => if project.artifacts.is_empty() { "No local artifacts".into() } else { project.artifacts.iter().map(|(path, _)| path.file_name().unwrap_or_default().to_string_lossy()).collect::<Vec<_>>().join(" · ") },
                };
                label.set_text(&text);
                label.set_tooltip_text(Some(&project.path.to_string_lossy()));
            });
            factory
        };
        let col = gtk::ColumnViewColumn::new(Some(if column == "Indexed size" { "Size" } else { column }), Some(factory));
        col.set_resizable(true);
        col.set_visible(!matches!(column, "Repository" | "Location"));
        col.set_fixed_width(width);
        col.set_expand(column == "Project");
        col.connect_fixed_width_notify(|column| column.set_expand(false));
        let records = projects.clone();
        let storage = storage.clone();
        col.set_sorter(Some(&gtk::CustomSorter::new(move |a, b| {
            let (Some(a), Some(b)) = (a.downcast_ref::<RowData>(), b.downcast_ref::<RowData>()) else { return gtk::Ordering::Equal; };
            let records = records.borrow();
            let (ap, bp) = (a.path(), b.path());
            let (Some(pa), Some(pb)) = (records.iter().find(|p| p.path == Path::new(&ap)), records.iter().find(|p| p.path == Path::new(&bp))) else { return gtk::Ordering::Equal; };
            let order = match column {
                "Indexed size" => storage.known_size(&pa.path).cmp(&storage.known_size(&pb.path)),
                "Modified" => pa.modified.cmp(&pb.modified),
                "Kind" => (pa.rust, pa.node).cmp(&(pb.rust, pb.node)),
                "Repository" => pa.repository.to_lowercase().cmp(&pb.repository.to_lowercase()),
                "Branch" => pa.branch.cmp(&pb.branch),
                "Location" => ap.cmp(&bp),
                "Builds / caches" => pa.artifacts.len().cmp(&pb.artifacts.len()),
                _ => a.name().to_lowercase().cmp(&b.name().to_lowercase()),
            };
            order.then_with(|| ap.cmp(&bp)).into()
        })));
        table.append_column(&col);
    }
    sorted.set_sorter(table.sorter().as_ref());
    let first = table.columns().item(0).and_downcast::<gtk::ColumnViewColumn>();
    table.sort_by_column(first.as_ref(), gtk::SortType::Ascending);
    toolbar.append(&columns::configure(&table, "projects"));
    let open = Rc::new(open);
    {
        let open = open.clone();
        let selection = selection.clone();
        table.connect_activate(move |_, position| {
            if let Some(row) = selection.item(position).and_downcast::<RowData>() { open(PathBuf::from(row.path())); }
        });
    }
    let table_scroll = gtk::ScrolledWindow::builder().child(&table).vexpand(true).build();
    let inspector = gtk::Box::new(gtk::Orientation::Vertical, 10);
    inspector.add_css_class("qfind-inspector");
    inspector.set_width_request(350);
    let heading = gtk::Label::new(Some("Select a project"));
    heading.add_css_class("title-3");
    heading.set_margin_top(18);
    heading.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    inspector.append(&heading);
    let path_label = gtk::Label::new(Some("Inspect changes, run builds, and review generated storage."));
    path_label.set_wrap(true);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path_label.add_css_class("dim-label");
    inspector.append(&path_label);
    let checkout = gtk::DropDown::from_strings(&[]);
    let checkout_factory = gtk::SignalListItemFactory::new();
    checkout_factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return; };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_max_width_chars(32);
        item.set_child(Some(&label));
    });
    checkout_factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return; };
        let Some(text) = item.item().and_downcast::<gtk::StringObject>() else { return; };
        let Some(label) = item.child().and_downcast::<gtk::Label>() else { return; };
        label.set_text(&text.string());
        label.set_tooltip_text(Some(&text.string()));
    });
    checkout.set_factory(Some(&checkout_factory));
    checkout.set_list_factory(Some(&checkout_factory));
    checkout.set_tooltip_text(Some("Choose this repository's local checkout or worktree"));
    checkout.set_visible(false);
    inspector.append(&checkout);
    let open_files = gtk::Button::with_label("Open project files");
    open_files.set_sensitive(false);
    inspector.append(&open_files);
    let pages = gtk::Stack::new();
    pages.set_vexpand(true);
    pages.set_hhomogeneous(false);
    pages.set_vhomogeneous(false);
    let overview = gtk::Box::new(gtk::Orientation::Vertical, 8);
    overview.set_margin_start(12);
    overview.set_margin_end(12);
    overview.set_margin_bottom(12);
    let caches = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let project_path = Rc::new(RefCell::new(None::<PathBuf>));
    let (changes, _) = git_panel::new(state.clone(), Some(project_path.clone()));
    pages.add_titled(&overview, Some("overview"), qfind_core::components::title("tasks"));
    pages.add_titled(&changes, Some("changes"), qfind_core::components::title("git"));
    pages.add_titled(&caches, Some("caches"), qfind_core::components::title("storage"));
    let tabs = gtk::StackSwitcher::new();
    tabs.set_stack(Some(&pages));
    tabs.set_halign(gtk::Align::Center);
    inspector.append(&tabs);
    inspector.append(&pages);
    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&table_scroll));
    split.set_end_child(Some(&inspector));
    split.set_position(720);
    split.set_resize_end_child(true);
    split.set_shrink_start_child(true);
    split.set_shrink_end_child(false);
    root.append(&split);
    let status = gtk::Label::new(Some("Opening project index…"));
    status.set_xalign(0.0);
    status.set_margin_start(16);
    status.set_margin_top(6);
    status.set_margin_bottom(6);
    root.append(&status);
    {
        let selected = project_path.clone();
        open_files.connect_clicked(move |_| { if let Some(path) = selected.borrow().clone() { open(path); } });
    }
    {
        let window = window.clone();
        let state = state.clone();
        let details = RefCell::new(HashMap::<PathBuf, (gtk::Box, gtk::Box)>::new());
        let render = Rc::new(move |project: Option<Project>| {
            let Some(project) = project else {
                *project_path.borrow_mut() = None;
                open_files.set_sensitive(false);
                heading.set_text("Select a project");
                path_label.set_text("Inspect changes, run builds, and review generated storage.");
                pages.set_visible(false);
                return;
            };
            pages.set_visible(true);
            *project_path.borrow_mut() = Some(project.path.clone());
            heading.set_text(project.repository.rsplit('/').next().unwrap_or_default());
            path_label.set_text(&project.path.to_string_lossy());
            path_label.set_tooltip_text(Some(&project.path.to_string_lossy()));
            open_files.set_sensitive(true);
            while let Some(child) = overview.first_child() { overview.remove(&child); }
            while let Some(child) = caches.first_child() { caches.remove(&child); }
            let panels = details.borrow_mut().entry(project.path.clone()).or_insert_with(|| (
                manager_tools::project_detail_content(&window, &state, project.path.clone(), project.rust, project.node, project.git),
                manager_tools::project_content_at(&window, &state, project.path.clone()),
            )).clone();
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
                if changing.get() { return; }
                let project = choices.borrow().get(checkout.selected() as usize).cloned();
                if let Some(project) = &project { remembered.borrow_mut().insert(project.repository.clone(), project.path.clone()); }
                render(project);
            });
        }
        let projects = projects.clone();
        selection.connect_selected_item_notify(move |selection| {
            let selected = selection.selected_item().and_downcast::<RowData>().and_then(|row| {
                let path = row.path();
                projects.borrow().iter().find(|project| project.path == Path::new(&path)).cloned()
            });
            changing.set(true);
            let mut items: Vec<_> = selected.as_ref().map(|selected| projects.borrow().iter().filter(|project| project.repository.eq_ignore_ascii_case(&selected.repository)).cloned().collect()).unwrap_or_default();
            items.sort_by(|a, b| a.path.cmp(&b.path));
            let labels: Vec<_> = items.iter().map(|project| format!("{} · {}", project.branch, project.path.display())).collect();
            let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
            let position = selected.as_ref().and_then(|selected| remembered.borrow().get(&selected.repository).cloned().or_else(|| Some(selected.path.clone())))
                .and_then(|path| items.iter().position(|project| project.path == path)).unwrap_or(0);
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
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(root) = weak.upgrade() else { return glib::ControlFlow::Break; };
        if !root.is_mapped() { return glib::ControlFlow::Continue; }
        if let Some(error) = storage.project_error() {
            status.set_text(&error);
            model.remove_all();
            last = None;
            return glib::ControlFlow::Continue;
        }
        let key = (search.text().to_lowercase(), storage.catalog_revision());
        if last.as_ref() == Some(&key) { return glib::ControlFlow::Continue; }
        let Some(mut items) = storage.projects(Path::new("/")) else {
            if model.n_items() > 0 { model.remove_all(); }
            status.set_text("Matching local repositories and worktrees to your GitHub account…");
            return glib::ControlFlow::Continue;
        };
        *projects.borrow_mut() = items.clone();
        items.retain(|project| project.path.to_string_lossy().to_lowercase().contains(&key.0) || project.repository.to_lowercase().contains(&key.0));
        {
            items.sort_by_key(|project| (
                project.path.components().any(|part| matches!(part.as_os_str().to_str(), Some("actions-runners" | "_work"))),
                !matches!(project.branch.as_str(), "main" | "master"), project.path.components().count(),
            ));
            let mut seen = HashSet::new();
            items.retain(|project| seen.insert(project.repository.to_lowercase()));
        }
        status.set_text(&format!("{} connected GitHub repositories · double-click to open a project file manager", items.len()));
        let rows: Vec<_> = items.iter().map(|project| RowData::new(project.repository.rsplit('/').next().unwrap_or_default().to_owned(), project.path.to_string_lossy(), true, 0, project.modified)).collect();
        model.splice(0, model.n_items(), &rows);
        last = Some(key);
        glib::ControlFlow::Continue
    });
    root
}
