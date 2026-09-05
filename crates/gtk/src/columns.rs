use super::*;
use std::fs;

fn all(view: &gtk::ColumnView) -> Vec<gtk::ColumnViewColumn> {
    (0..view.columns().n_items()).filter_map(|i| view.columns().item(i).and_downcast()).collect()
}

/// Native header menus and a visible column chooser share the same actions.
pub fn configure(view: &gtk::ColumnView, key: &str) -> gtk::MenuButton {
    view.set_reorderable(true);
    let path = dirs::config_dir().unwrap_or_default().join(format!("qfind/columns-{key}"));
    let columns = all(view);
    let defaults: Vec<_> = columns.iter().map(|c| (c.fixed_width(), c.is_visible(), c.expands())).collect();
    if let Ok(saved) = fs::read_to_string(&path) {
        if saved.starts_with("# columns-v2\n") {
            for (position, line) in saved.lines().skip(1).enumerate() {
                let parts: Vec<_> = line.splitn(4, '\t').collect();
                if parts.len() != 4 { continue; }
                let Some(column) = columns.iter().find(|c| c.title().as_deref() == Some(parts[3])) else { continue; };
                if let Ok(width) = parts[0].parse::<i32>() { column.set_fixed_width(width.clamp(-1, 10000)); }
                column.set_visible(column == &columns[0] || parts[1] == "1");
                column.set_expand(parts[2] == "1");
                view.remove_column(column);
                view.insert_column((position as u32).min(view.columns().n_items()), column);
            }
        } else {
            // Migrate the original hidden-title list without losing preferences.
            for (index, column) in columns.iter().enumerate() {
                column.set_visible(index == 0 || (defaults[index].1 && !saved.lines().any(|line| Some(line) == column.title().as_deref())));
            }
        }
    }
    let pending = Rc::new(RefCell::new(None::<glib::SourceId>));
    let weak_view = view.downgrade();
    let save: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(source) = pending.borrow_mut().take() { source.remove(); }
        let finished = pending.clone();
        let weak_view = weak_view.clone();
        let path = path.clone();
        let source = glib::timeout_add_local_once(Duration::from_millis(350), move || {
            finished.borrow_mut().take();
            let Some(view) = weak_view.upgrade() else { return; };
            let mut text = String::from("# columns-v2\n");
            for column in all(&view) {
                text.push_str(&format!("{}\t{}\t{}\t{}\n", column.fixed_width(), u8::from(column.is_visible()), u8::from(column.expands()), column.title().unwrap_or_default()));
            }
            let result = (|| -> std::io::Result<()> {
                let parent = path.parent().unwrap();
                fs::create_dir_all(parent)?;
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                std::io::Write::write_all(&mut file, text.as_bytes())?;
                file.persist(&path).map_err(|error| error.error)?;
                Ok(())
            })();
            if let Err(error) = result { eprintln!("Could not save column settings: {error}"); }
        });
        pending.replace(Some(source));
    });
    let group = gio::SimpleActionGroup::new();
    let menu = gio::Menu::new();
    for (index, column) in columns.iter().enumerate() {
        let title = column.title().unwrap_or_default();
        let action = gio::SimpleAction::new_stateful(&format!("c{index}"), None, &column.is_visible().to_variant());
        action.set_enabled(index != 0);
        let target = column.clone();
        action.connect_activate(move |_, _| target.set_visible(!target.is_visible()));
        let weak_action = action.downgrade();
        let save_visible = save.clone();
        column.connect_visible_notify(move |column| {
            if let Some(action) = weak_action.upgrade() { action.set_state(&column.is_visible().to_variant()); }
            save_visible();
        });
        let save_width = save.clone();
        column.connect_fixed_width_notify(move |_| save_width());
        let save_expand = save.clone();
        column.connect_expand_notify(move |_| save_expand());
        menu.append(Some(&title), Some(&format!("columns.c{index}")));
        group.add_action(&action);
    }
    let save_order = save.clone();
    view.columns().connect_items_changed(move |_, _, _, _| save_order());
    let reset = gio::SimpleAction::new("reset", None);
    let weak_view = view.downgrade();
    let reset_columns = columns.clone();
    reset.connect_activate(move |_, _| {
        let Some(view) = weak_view.upgrade() else { return; };
        for (index, (column, &(width, visible, expand))) in reset_columns.iter().zip(&defaults).enumerate() {
            view.remove_column(column);
            view.insert_column(index as u32, column);
            column.set_fixed_width(width);
            column.set_visible(visible);
            column.set_expand(expand);
        }
        save();
    });
    group.add_action(&reset);
    let reset_menu = gio::Menu::new();
    reset_menu.append(Some("Reset columns"), Some("columns.reset"));
    menu.append_section(None, &reset_menu);
    for column in columns { column.set_header_menu(Some(&menu)); }
    view.insert_action_group("columns", Some(&group));
    let button = gtk::MenuButton::builder().label("Columns").menu_model(&menu).build();
    button.insert_action_group("columns", Some(&group));
    button.set_tooltip_text(Some("Choose columns · drag headers to reorder · drag dividers to resize · layout saves automatically"));
    button
}

pub fn text_value(data: &RowData, location: bool) -> String {
    text_key(&data.name(), Path::new(&data.path()), data.is_dir(), location)
}

pub fn text_key(name: &str, path: &Path, is_dir: bool, location: bool) -> String {
    if location {
        path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    } else if is_dir {
        "Folder".into()
    } else {
        Path::new(name).extension().filter(|extension| !extension.is_empty())
            .map(|extension| format!("{} file", extension.to_string_lossy().to_uppercase()))
            .unwrap_or_else(|| "File".into())
    }
}

pub fn text_factory(location: bool, zebra: Rc<Cell<bool>>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return; };
        let label = gtk::Label::builder().hexpand(true).xalign(0.0).single_line_mode(true)
            .ellipsize(gtk::pango::EllipsizeMode::End).margin_end(6).css_classes(["dim-label"]).build();
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return; };
        let Some(data) = item.item().and_downcast::<RowData>() else { return; };
        let Some(label) = item.child().and_downcast::<gtk::Label>() else { return; };
        let value = text_value(&data, location);
        label.set_text(&value);
        label.set_tooltip_text(Some(&value));
        if zebra.get() && item.position() % 2 == 1 { label.add_css_class("qfind-odd"); }
        else { label.remove_css_class("qfind-odd"); }
    });
    factory
}
