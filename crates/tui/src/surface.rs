use qfind_core::Surface;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};

use super::{App, BrowserPane};

pub(crate) fn prepare(app: &mut App, area: Rect) {
    app.frame_area = area;
    let weight_height = (area.height / 4).clamp(7, 12);
    let chunks = if app.show_weight {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(weight_height),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area)
    };
    app.header_area = chunks[0];
    app.content_area = chunks[1];
    app.weight_panel_area = if app.show_weight {
        chunks[2]
    } else {
        Rect::default()
    };
    app.prompt_area = chunks[if app.show_weight { 3 } else { 2 }];
    app.footer_area = chunks[if app.show_weight { 4 } else { 3 }];
    app.preview_area = Rect::default();
    app.preview_divider = Rect::default();

    if app.surface == Surface::Tree {
        prepare_tree(app);
    } else {
        let hits = if app.content_area.width >= 96 {
            let preview = u16::from(app.preview_width.clamp(20, 70));
            let panes = Layout::horizontal([
                Constraint::Percentage(100 - preview),
                Constraint::Percentage(preview),
            ])
            .split(app.content_area);
            app.preview_area = panes[1];
            app.preview_divider = Rect::new(panes[1].x, panes[1].y, 1, panes[1].height);
            panes[0]
        } else {
            app.content_area
        };
        if app.zoom.is_grid() {
            prepare_grid(app, hits);
        } else {
            prepare_list(app, hits);
        }
    }
}

fn prepare_list(app: &mut App, area: Rect) {
    let bar_width = u16::from(area.height as usize + 1 < app.rows.len() || app.rows.len() > 8) * 2;
    app.hits_area = Rect::new(
        area.x,
        area.y,
        area.width.saturating_sub(bar_width),
        area.height,
    );
    app.scroll_bar = if bar_width > 0 {
        Rect::new(area.right().saturating_sub(1), area.y, 1, area.height)
    } else {
        Rect::default()
    };
    app.view_h = app.hits_area.height as usize;
    ensure_visible(app.selected, &mut app.scroll, app.rows.len(), app.view_h);
    app.list_start = app.scroll;
    app.grid_cols = 1;
    app.grid_cell_w = app.hits_area.width.max(1);
    app.grid_cell_h = 1;
}

fn prepare_grid(app: &mut App, area: Rect) {
    let density = app.zoom.get().saturating_sub(qfind_core::Zoom::GRID_FROM);
    let target_width = (12 + u16::from(density) / 2).min(area.width).max(1);
    let target_height = (4 + u16::from(density) / 8).min(area.height).max(1);
    let columns = area.width.div_ceil(target_width).max(1) as usize;
    let rows = area.height.div_ceil(target_height).max(1) as usize;
    let cell_width = area.width.div_ceil(columns as u16).max(1);
    let cell_height = area.height.div_ceil(rows as u16).max(1);
    app.hits_area = area;
    app.scroll_bar = Rect::default();
    app.grid_cols = columns;
    app.grid_cell_w = cell_width;
    app.grid_cell_h = cell_height;
    app.view_h = columns * rows;
    ensure_visible(app.selected, &mut app.scroll, app.rows.len(), app.view_h);
    app.scroll -= app.scroll % columns;
    app.list_start = app.scroll;
}

fn prepare_tree(app: &mut App) {
    app.scroll_bar = Rect::default();
    let panes = Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(app.content_area);
    app.folder_pane = panes[0];
    app.item_pane = panes[1];
    let folder_inner = inset(panes[0]);
    let item_inner = inset(panes[1]);
    let folder_bar = u16::from(app.browser_folders.len() > folder_inner.height as usize) * 2;
    let item_bar = u16::from(app.browser_items.len() > item_inner.height as usize) * 2;
    app.folders_area = Rect::new(
        folder_inner.x,
        folder_inner.y,
        folder_inner.width.saturating_sub(folder_bar),
        folder_inner.height,
    );
    app.items_area = Rect::new(
        item_inner.x,
        item_inner.y,
        item_inner.width.saturating_sub(item_bar),
        item_inner.height,
    );
    app.folders_bar = scrollbar(folder_inner, folder_bar > 0);
    app.items_bar = scrollbar(item_inner, item_bar > 0);
    ensure_visible(
        app.folder_selected,
        &mut app.folder_scroll,
        app.browser_folders.len(),
        app.folders_area.height as usize,
    );
    ensure_visible(
        app.item_selected,
        &mut app.item_scroll,
        app.browser_items.len(),
        app.items_area.height as usize,
    );
}

fn inset(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn scrollbar(area: Rect, visible: bool) -> Rect {
    if visible {
        Rect::new(area.right().saturating_sub(1), area.y, 1, area.height)
    } else {
        Rect::default()
    }
}

pub(crate) fn ensure_visible(selected: usize, scroll: &mut usize, total: usize, view: usize) {
    let view = view.max(1);
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= scroll.saturating_add(view) {
        *scroll = selected.saturating_add(1).saturating_sub(view);
    }
    *scroll = (*scroll).min(total.saturating_sub(view));
}

pub(crate) fn result_at(app: &App, position: Position) -> Option<usize> {
    let local_y = position.y.saturating_sub(app.hits_area.y) as usize;
    let index = if app.zoom.is_grid() {
        let local_x = position.x.saturating_sub(app.hits_area.x) as usize;
        app.list_start
            + (local_y / app.grid_cell_h.max(1) as usize) * app.grid_cols.max(1)
            + local_x / app.grid_cell_w.max(1) as usize
    } else {
        app.scroll + local_y
    };
    (index < app.rows.len()).then_some(index)
}

pub(crate) fn bar_jump(app: &mut App, row: u16) {
    let (bar, scroll, total, view) = if app.surface == Surface::Tree {
        match app.browser_pane {
            BrowserPane::Folders => (
                app.folders_bar,
                &mut app.folder_scroll,
                app.browser_folders.len(),
                app.folders_area.height as usize,
            ),
            BrowserPane::Items => (
                app.items_bar,
                &mut app.item_scroll,
                app.browser_items.len(),
                app.items_area.height as usize,
            ),
        }
    } else {
        (app.scroll_bar, &mut app.scroll, app.rows.len(), app.view_h)
    };
    let height = bar.height.max(1) as usize;
    let max_scroll = total.saturating_sub(view.max(1));
    let offset = row.saturating_sub(bar.y) as usize;
    *scroll = if height <= 1 {
        0
    } else {
        (offset * max_scroll) / (height - 1)
    }
    .min(max_scroll);
}
