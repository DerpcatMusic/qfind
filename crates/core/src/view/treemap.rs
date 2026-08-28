//! WizTree-style weight map: folder rectangles for the current Hits.

use super::tree::HitRef;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Weighted {
    pub name: String,
    pub path: String,
    pub weight: u64,
    pub id: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub name: String,
    pub path: String,
    pub weight: u64,
    pub id: Option<u32>,
}

/// Group Hits by parent folder. Weight is size, or 1 per Hit if size is 0.
pub fn folder_weights(items: &[HitRef]) -> Vec<Weighted> {
    let mut map: BTreeMap<String, Weighted> = BTreeMap::new();
    for it in items {
        let parent = parent_of(&it.path);
        let name = folder_name(&parent);
        let e = map.entry(parent.clone()).or_insert_with(|| Weighted {
            name,
            path: parent,
            weight: 0,
            id: None,
        });
        e.weight = e.weight.saturating_add(it.weight.max(1));
    }
    let mut v: Vec<_> = map.into_values().collect();
    v.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.path.cmp(&b.path)));
    v
}

fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((p, _)) if p.is_empty() => "/".into(),
        Some((p, _)) => p.to_string(),
        None => "/".into(),
    }
}

fn folder_name(path: &str) -> String {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("/").to_string()
}

/// Squarified treemap (Bruls / Huizing / van Wijk). Coordinates in pixels.
pub fn squarify(mut items: Vec<Weighted>, width: f64, height: f64) -> Vec<Tile> {
    items.retain(|i| i.weight > 0);
    if items.is_empty() || width <= 1.0 || height <= 1.0 {
        return Vec::new();
    }
    let total: f64 = items.iter().map(|i| i.weight as f64).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(items.len());
    squarify_rec(&items, 0.0, 0.0, width, height, total, &mut out);
    out
}

fn squarify_rec(
    items: &[Weighted],
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    total: f64,
    out: &mut Vec<Tile>,
) {
    if items.is_empty() {
        return;
    }
    if items.len() == 1 || w < 2.0 || h < 2.0 {
        fill_slice(items, x, y, w, h, total, out);
        return;
    }
    let vertical = w >= h;
    let side = if vertical { h } else { w };
    let mut row_w = 0.0;
    let mut end = 0usize;
    let mut best_worst = f64::INFINITY;
    for i in 0..items.len() {
        row_w += items[i].weight as f64;
        let worst = row_worst(items, end + 1, row_w, side, total, w * h);
        if worst <= best_worst {
            best_worst = worst;
            end = i + 1;
        } else {
            break;
        }
    }
    let row_sum: f64 = items[..end].iter().map(|i| i.weight as f64).sum();
    let frac = (row_sum / total).clamp(0.0, 1.0);
    if vertical {
        let col_w = w * frac;
        fill_slice(&items[..end], x, y, col_w, h, row_sum, out);
        squarify_rec(&items[end..], x + col_w, y, w - col_w, h, total - row_sum, out);
    } else {
        let row_h = h * frac;
        fill_slice(&items[..end], x, y, w, row_h, row_sum, out);
        squarify_rec(&items[end..], x, y + row_h, w, h - row_h, total - row_sum, out);
    }
}

fn row_worst(items: &[Weighted], n: usize, row_w: f64, side: f64, total: f64, area: f64) -> f64 {
    if n == 0 || row_w <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let row_area = area * (row_w / total);
    let thickness = row_area / side;
    if thickness <= 0.0 {
        return f64::INFINITY;
    }
    let mut worst: f64 = 1.0;
    for it in items.iter().take(n) {
        let a = row_area * (it.weight as f64 / row_w);
        let other = a / thickness;
        let aspect = (thickness.max(other)) / thickness.min(other).max(1e-9);
        worst = worst.max(aspect);
    }
    worst
}

fn fill_slice(items: &[Weighted], x: f64, y: f64, w: f64, h: f64, total: f64, out: &mut Vec<Tile>) {
    if items.is_empty() || total <= 0.0 {
        return;
    }
    let vertical = w >= h;
    let mut offset = 0.0;
    for it in items {
        let frac = it.weight as f64 / total;
        if vertical {
            let hh = h * frac;
            out.push(tile(it, x, y + offset, w, hh));
            offset += hh;
        } else {
            let ww = w * frac;
            out.push(tile(it, x + offset, y, ww, h));
            offset += ww;
        }
    }
}

fn tile(it: &Weighted, x: f64, y: f64, w: f64, h: f64) -> Tile {
    Tile {
        x,
        y,
        w,
        h,
        name: it.name.clone(),
        path: it.path.clone(),
        weight: it.weight,
        id: it.id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::HitRef;

    #[test]
    fn groups_by_parent() {
        let items = [
            HitRef {
                id: Some(1),
                path: "/a/x.txt".into(),
                is_dir: false,
                weight: 10,
            },
            HitRef {
                id: Some(2),
                path: "/a/y.txt".into(),
                is_dir: false,
                weight: 5,
            },
            HitRef {
                id: Some(3),
                path: "/b/z.txt".into(),
                is_dir: false,
                weight: 1,
            },
        ];
        let w = folder_weights(&items);
        assert_eq!(w[0].path, "/a");
        assert_eq!(w[0].weight, 15);
        assert_eq!(w[1].path, "/b");
    }

    #[test]
    fn empty_and_zero_size_are_blank() {
        assert!(squarify(Vec::new(), 100.0, 50.0).is_empty());
        assert!(squarify(
            vec![Weighted {
                name: "z".into(),
                path: "/z".into(),
                weight: 0,
                id: None,
            }],
            100.0,
            50.0
        )
        .is_empty());
        assert!(squarify(
            vec![Weighted {
                name: "a".into(),
                path: "/a".into(),
                weight: 1,
                id: None,
            }],
            0.0,
            10.0
        )
        .is_empty());
    }

    #[test]
    fn tiles_cover_area() {
        let items = vec![
            Weighted {
                name: "a".into(),
                path: "/a".into(),
                weight: 50,
                id: None,
            },
            Weighted {
                name: "b".into(),
                path: "/b".into(),
                weight: 30,
                id: None,
            },
            Weighted {
                name: "c".into(),
                path: "/c".into(),
                weight: 20,
                id: None,
            },
        ];
        let tiles = squarify(items, 100.0, 50.0);
        let area: f64 = tiles.iter().map(|t| t.w * t.h).sum();
        assert!((area - 5000.0).abs() < 1.0, "{area}");
        assert_eq!(tiles.len(), 3);
    }
}
