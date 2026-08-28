//! Settings window: Exclude/include, spacing, PreviewMode, reset.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use qfind_core::{Config, MatchMode, PreviewMode, Zoom};

pub struct Live {
    pub zoom: Rc<Cell<Zoom>>,
    pub spacing: Rc<Cell<u8>>,
    pub preview: Rc<Cell<PreviewMode>>,
    pub zebra: Rc<Cell<bool>>,
    pub weight: Rc<Cell<bool>>,
    pub match_mode: Rc<Cell<MatchMode>>,
    pub on_rebuild: Box<dyn Fn()>,
}

pub fn open(parent: &gtk::ApplicationWindow, live: Live) {
    let cfg = Config::load();
    let win = gtk::Window::builder()
        .transient_for(parent)
        .title("Qfind Settings")
        .default_width(520)
        .default_height(560)
        .modal(true)
        .build();

    let exclude = list_editor("Exclude (names or globs, extra junk skipped on Rebuild)", &cfg.exclude);
    let include = list_editor(
        "Include Mounts (empty = discover all local disks)",
        &cfg.include
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>(),
    );

    let spacing = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 24.0, 1.0);
    spacing.set_value(f64::from(cfg.spacing));
    spacing.set_draw_value(true);
    spacing.set_hexpand(true);
    let compact = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    compact.set_value(f64::from(cfg.zoom));
    compact.set_draw_value(true);
    compact.set_hexpand(true);
    compact.set_tooltip_text(Some("Default Zoom (Ctrl+scroll still works in the list)"));

    let preview_drop = gtk::DropDown::from_strings(&["Hovered Hit (Space)", "Selected Hit (Space)"]);
    preview_drop.set_selected(match cfg.preview {
        PreviewMode::Hovered => 0,
        PreviewMode::Selected => 1,
    });
    let match_drop = gtk::DropDown::from_strings(&[
        "Fuzzy (hlo → hello.txt)",
        "Substring (contiguous)",
        "Exact filename",
    ]);
    match_drop.set_tooltip_text(Some(
        "Fuzzy is on by default. Substring turns gaps off. Exact is the whole name.",
    ));
    match_drop.set_selected(match cfg.match_mode {
        MatchMode::Fuzzy => 0,
        MatchMode::Substring => 1,
        MatchMode::Exact => 2,
    });

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
    vbox.set_margin_start(14);
    vbox.set_margin_end(14);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.append(&exclude.root);
    vbox.append(&include.root);
    vbox.append(&label("Compactness / default Zoom"));
    vbox.append(&compact);
    vbox.append(&label("Extra spacing (px)"));
    vbox.append(&spacing);
    vbox.append(&label("Space preview"));
    vbox.append(&preview_drop);
    vbox.append(&label("Query matching"));
    vbox.append(&match_drop);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let reset = gtk::Button::with_label("Reset to default");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    buttons.append(&reset);
    buttons.append(&save);
    vbox.append(&buttons);

    let scroll = gtk::ScrolledWindow::builder()
        .child(&vbox)
        .hexpand(true)
        .vexpand(true)
        .build();
    win.set_child(Some(&scroll));

    let live = Rc::new(live);
    {
        let live = Rc::clone(&live);
        let exclude = exclude.clone();
        let include = include.clone();
        let spacing = spacing.clone();
        let compact = compact.clone();
        let preview_drop = preview_drop.clone();
        let match_drop = match_drop.clone();
        let win = win.clone();
        save.connect_clicked(move |_| {
            let mut cfg = Config::default();
            cfg.exclude = exclude.items();
            cfg.include = include
                .items()
                .into_iter()
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
            cfg.spacing = spacing.value() as u8;
            cfg.zoom = compact.value() as u8;
            cfg.preview = if preview_drop.selected() == 1 {
                PreviewMode::Selected
            } else {
                PreviewMode::Hovered
            };
            cfg.zebra = live.zebra.get();
            cfg.weight_map = live.weight.get();
            cfg.match_mode = match match_drop.selected() {
                1 => MatchMode::Substring,
                2 => MatchMode::Exact,
                _ => MatchMode::Fuzzy,
            };
            let _ = cfg.save();
            live.zoom.set(Zoom::new(cfg.zoom));
            live.spacing.set(cfg.spacing);
            live.preview.set(cfg.preview);
            live.match_mode.set(cfg.match_mode);
            (live.on_rebuild)();
            win.close();
        });
    }
    {
        let exclude = exclude.clone();
        let include = include.clone();
        let spacing = spacing.clone();
        let compact = compact.clone();
        let preview_drop = preview_drop.clone();
        let match_drop = match_drop.clone();
        reset.connect_clicked(move |_| {
            let cfg = Config::default();
            exclude.set_items(&cfg.exclude);
            include.set_items(&[]);
            spacing.set_value(0.0);
            compact.set_value(f64::from(cfg.zoom));
            preview_drop.set_selected(0);
            match_drop.set_selected(0);
        });
    }

    win.present();
}

fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("heading");
    l
}

#[derive(Clone)]
struct ListEdit {
    root: gtk::Box,
    rows: Rc<RefCell<gtk::Box>>,
}

impl ListEdit {
    fn items(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut child = self.rows.borrow().first_child();
        while let Some(row) = child {
            if let Some(entry) = row.first_child().and_downcast::<gtk::Entry>() {
                let t = entry.text().to_string();
                if !t.trim().is_empty() {
                    out.push(t.trim().to_string());
                }
            }
            child = row.next_sibling();
        }
        out
    }

    fn set_items(&self, items: &[String]) {
        while let Some(c) = self.rows.borrow().first_child() {
            self.rows.borrow().remove(&c);
        }
        if items.is_empty() {
            self.rows.borrow().append(&entry_row(""));
        } else {
            for i in items {
                self.rows.borrow().append(&entry_row(i));
            }
        }
    }
}

fn list_editor(title: &str, items: &[String]) -> ListEdit {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.append(&label(title));
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 4);
    if items.is_empty() {
        rows.append(&entry_row(""));
    } else {
        for i in items {
            rows.append(&entry_row(i));
        }
    }
    let rows = Rc::new(RefCell::new(rows));
    root.append(&*rows.borrow());
    let add = gtk::Button::with_label("Add");
    {
        let rows = Rc::clone(&rows);
        add.connect_clicked(move |_| {
            rows.borrow().append(&entry_row(""));
        });
    }
    root.append(&add);
    ListEdit { root, rows }
}

fn entry_row(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let entry = gtk::Entry::new();
    entry.set_text(text);
    entry.set_hexpand(true);
    let rm = gtk::Button::from_icon_name("list-remove-symbolic");
    {
        let row = row.clone();
        rm.connect_clicked(move |_| {
            if let Some(parent) = row.parent() {
                if let Ok(box_) = parent.downcast::<gtk::Box>() {
                    box_.remove(&row);
                }
            }
        });
    }
    row.append(&entry);
    row.append(&rm);
    row
}
