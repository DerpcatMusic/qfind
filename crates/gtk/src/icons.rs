use std::cell::Cell;
use std::path::Path;

thread_local! {
    static BUNDLED: Cell<bool> = const { Cell::new(true) };
    static SEARCH_PATH_ADDED: Cell<bool> = const { Cell::new(false) };
}

/// Whether folders are drawn with Megaman's own art instead of the icon theme.
pub fn bundled() -> bool {
    BUNDLED.get()
}

/// Make our icons resolvable and, when `bundled`, lay Megaman's set over the
/// current icon theme. Call after the icon theme name is set.
///
/// The logo goes into a private `hicolor`, which every theme falls back to:
/// it used to live only inside the Megaman overlay, so with any other icon
/// theme the header showed a broken image.
pub fn install(bundled: bool) {
    BUNDLED.set(bundled);
    let (Some(display), Some(settings), Some(cache)) = (
        gtk::gdk::Display::default(),
        gtk::Settings::default(),
        dirs::cache_dir(),
    ) else {
        return;
    };
    let root = cache.join("qfind/icons");
    let logo = root.join("hicolor/scalable/apps");
    if write_if_changed(
        &logo,
        "megaman.svg",
        include_str!("../../../assets/megaman.svg"),
    )
    .is_ok()
        && !SEARCH_PATH_ADDED.replace(true)
    {
        gtk::IconTheme::for_display(&display).add_search_path(&root);
    }
    if !bundled {
        return;
    }
    let theme = root.join("QfindWorkspace");
    let directory = theme.join("scalable/actions");
    let inherited = settings
        .gtk_icon_theme_name()
        .filter(|name| name != "QfindWorkspace")
        .unwrap_or_else(|| "Adwaita".into());
    let index = format!(
        "[Icon Theme]\nName=QfindWorkspace\nInherits={inherited},Adwaita,hicolor\nDirectories=scalable/actions\n\n[scalable/actions]\nSize=24\nMinSize=16\nMaxSize=256\nType=Scalable\nContext=Actions\n"
    );
    let write = || -> std::io::Result<()> {
        write_if_changed(&theme, "index.theme", &index)?;
        for line in include_str!("icons.tsv").lines() {
            let Some((names, svg)) = line.split_once('\t') else {
                continue;
            };
            for name in names.split_whitespace() {
                write_if_changed(&directory, &format!("{name}.svg"), svg)?;
            }
        }
        Ok(())
    };
    if write().is_ok() {
        settings.set_gtk_icon_theme_name(Some("QfindWorkspace"));
    }
}

fn write_if_changed(dir: &Path, name: &str, content: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(content) {
        std::fs::write(path, content)?;
    }
    Ok(())
}
