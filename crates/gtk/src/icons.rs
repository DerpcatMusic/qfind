
pub fn install() {
    let (Some(display), Some(settings), Some(cache)) = (gtk::gdk::Display::default(), gtk::Settings::default(), dirs::cache_dir()) else { return; };
    let root = cache.join("qfind/icons");
    let theme = root.join("QfindWorkspace");
    let directory = theme.join("scalable/actions");
    let inherited = settings.gtk_icon_theme_name().unwrap_or_else(|| "Adwaita".into());
    if inherited == "QfindWorkspace" { return; }
    let index = format!("[Icon Theme]\nName=QfindWorkspace\nInherits={inherited},Adwaita,hicolor\nDirectories=scalable/actions\n\n[scalable/actions]\nSize=24\nMinSize=16\nMaxSize=256\nType=Scalable\nContext=Actions\n");
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&directory)?;
        if std::fs::read_to_string(theme.join("index.theme")).ok().as_deref() != Some(&index) {
            std::fs::write(theme.join("index.theme"), &index)?;
        }
        for line in include_str!("icons.tsv").lines() {
            let Some((names, svg)) = line.split_once('\t') else { continue; };
            for name in names.split_whitespace() {
                let path = directory.join(format!("{name}.svg"));
                if std::fs::read_to_string(&path).ok().as_deref() != Some(svg) { std::fs::write(path, &svg)?; }
            }
        }
        Ok(())
    };
    if write().is_ok() {
        gtk::IconTheme::for_display(&display).add_search_path(root);
        settings.set_gtk_icon_theme_name(Some("QfindWorkspace"));
    }
}
