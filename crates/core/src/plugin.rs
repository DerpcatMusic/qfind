//! Narrow plugin boundary. Deliberately not a marketplace or scripting
//! runtime (see `docs/file-manager-plan.md` §5): plugins observe navigation
//! and Hits through read-only hooks and offer named actions. They never touch
//! the Catalog, the filesystem, or widgets directly — every action flows
//! through the single dispatch point ([`crate::Manager::dispatch`]).

use std::path::Path;

/// What a plugin may do. All methods have no-op defaults; the trait is
/// object safe so hosts can hold `Box<dyn Plugin>`.
pub trait Plugin {
    /// Stable id, e.g. `"places"` or `"git-status"`. First registration wins.
    fn id(&self) -> &str;
    /// Called after navigation settles (`None` = back to global Catalog).
    fn on_navigate(&mut self, _path: Option<&Path>) {}
    /// Called after a Hits list is produced for `query`.
    fn on_hits(&mut self, _query: &str, _hit_count: usize) {}
    /// Handle [`crate::Action::Plugin`]. Return true when handled.
    fn on_action(&mut self, _name: &str, _arg: &str) -> bool {
        false
    }
}

/// Registry every frontend threads through [`crate::Manager::dispatch`].
#[derive(Default)]
pub struct PluginHost {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin. A duplicate `id` is ignored (first wins).
    pub fn register(&mut self, plugin: impl Plugin + 'static) {
        if !self.plugins.iter().any(|p| p.id() == plugin.id()) {
            self.plugins.push(Box::new(plugin));
        }
    }

    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.id()).collect()
    }

    pub fn notify_navigate(&mut self, path: Option<&Path>) {
        for plugin in &mut self.plugins {
            plugin.on_navigate(path);
        }
    }

    pub fn notify_hits(&mut self, query: &str, hit_count: usize) {
        for plugin in &mut self.plugins {
            plugin.on_hits(query, hit_count);
        }
    }

    /// Route a named action. The first plugin returning true wins.
    pub fn dispatch_action(&mut self, name: &str, arg: &str) -> bool {
        for plugin in &mut self.plugins {
            if plugin.on_action(name, arg) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rec {
        navs: usize,
        hits: Vec<(String, usize)>,
        actions: Vec<(String, String)>,
    }

    impl Plugin for Rec {
        fn id(&self) -> &str {
            "rec"
        }
        fn on_navigate(&mut self, _path: Option<&Path>) {
            self.navs += 1;
        }
        fn on_hits(&mut self, query: &str, hit_count: usize) {
            self.hits.push((query.to_owned(), hit_count));
        }
        fn on_action(&mut self, name: &str, arg: &str) -> bool {
            if name == "ping" {
                self.actions.push((name.to_owned(), arg.to_owned()));
                return true;
            }
            false
        }
    }

    #[test]
    fn host_routes_and_dedupes() {
        let mut host = PluginHost::new();
        host.register(Rec {
            navs: 0,
            hits: Vec::new(),
            actions: Vec::new(),
        });
        host.register(Rec {
            navs: 0,
            hits: Vec::new(),
            actions: Vec::new(),
        });
        assert_eq!(host.ids(), ["rec"]);
        host.notify_navigate(None);
        host.notify_hits("wav", 3);
        assert!(host.dispatch_action("ping", "x"));
        assert!(!host.dispatch_action("unknown", "x"));
    }
}
