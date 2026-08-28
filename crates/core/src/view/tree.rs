//! Experimental tree: fold Hits by path prefix. Expand/collapse is the adapter’s job.

use std::collections::BTreeMap;

/// One Hit (or a folder implied by a Hit path) fed into [`fold_stems`].
#[derive(Clone, Debug)]
pub struct HitRef {
    pub id: Option<u32>,
    pub path: String,
    pub is_dir: bool,
    pub weight: u64,
}

/// One node in the experimental tree.
#[derive(Clone, Debug)]
pub struct Stem {
    pub name: String,
    pub path: String,
    pub id: Option<u32>,
    pub is_dir: bool,
    pub weight: u64,
    pub kids: Vec<Stem>,
}

/// Visible row after expand/collapse.
#[derive(Clone, Debug)]
pub struct Flat {
    pub stem: Stem,
    pub depth: u32,
    pub has_kids: bool,
}

pub fn fold_stems(items: &[HitRef]) -> Vec<Stem> {
    #[derive(Default)]
    struct Node {
        id: Option<u32>,
        is_dir: bool,
        weight: u64,
        kids: BTreeMap<String, Node>,
    }
    let mut root = Node::default();
    for it in items {
        let mut parts = it.path.split('/').filter(|p| !p.is_empty()).peekable();
        if parts.peek().is_none() {
            continue;
        }
        let mut cur = &mut root;
        while let Some(part) = parts.next() {
            let last = parts.peek().is_none();
            let child = cur.kids.entry(part.to_string()).or_default();
            if last {
                child.id = it.id;
                child.is_dir = it.is_dir;
                child.weight = it.weight.max(1);
            } else {
                child.is_dir = true;
                child.weight = child.weight.saturating_add(it.weight.max(1));
            }
            cur = child;
        }
    }
    fn into_stems(map: BTreeMap<String, Node>, prefix: &str) -> Vec<Stem> {
        map.into_iter()
            .map(|(name, node)| {
                let path = if prefix.is_empty() {
                    format!("/{name}")
                } else if prefix == "/" {
                    format!("/{name}")
                } else {
                    format!("{prefix}/{name}")
                };
                let kids = into_stems(node.kids, &path);
                Stem {
                    name,
                    path,
                    id: node.id,
                    is_dir: node.is_dir || !kids.is_empty(),
                    weight: node.weight,
                    kids,
                }
            })
            .collect()
    }
    into_stems(root.kids, "")
}

pub fn walk_visible(stems: &[Stem], expanded: &impl Fn(&str) -> bool) -> Vec<Flat> {
    fn walk(stems: &[Stem], depth: u32, expanded: &impl Fn(&str) -> bool, out: &mut Vec<Flat>) {
        for stem in stems {
            let has_kids = !stem.kids.is_empty();
            out.push(Flat {
                has_kids,
                depth,
                stem: Stem {
                    name: stem.name.clone(),
                    path: stem.path.clone(),
                    id: stem.id,
                    is_dir: stem.is_dir,
                    weight: stem.weight,
                    kids: Vec::new(),
                },
            });
            if has_kids && expanded(&stem.path) {
                walk(&stem.kids, depth + 1, expanded, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(stems, 0, expanded, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_nested_paths() {
        let items = [
            HitRef {
                id: Some(1),
                path: "/a/b/c.txt".into(),
                is_dir: false,
                weight: 3,
            },
            HitRef {
                id: Some(2),
                path: "/a/d.txt".into(),
                is_dir: false,
                weight: 1,
            },
        ];
        let tree = fold_stems(&items);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "a");
        assert!(tree[0].is_dir);
        assert_eq!(tree[0].kids.len(), 2);
        let shown = walk_visible(&tree, &|_| true);
        assert!(
            shown.iter().any(|f| f.stem.name == "c.txt"),
            "expanded tree must show the file, not only folders"
        );
        let collapsed = walk_visible(&tree, &|p| p != "/a/b");
        assert!(!collapsed.iter().any(|f| f.stem.name == "c.txt"));
        assert!(collapsed.iter().any(|f| f.stem.name == "d.txt"));
    }
}
