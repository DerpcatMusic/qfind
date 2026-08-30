use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use qfind_core::{Catalog, Error, IgnoreMatcher, SearchOpts};
use rayon::prelude::*;

use super::{Row, WorkEvent, reactor};

pub(crate) enum Event {
    Hits(u64, Result<Vec<Row>, String>),
    Sizes(u64, Vec<Row>),
}

struct Request {
    generation: u64,
    query: String,
    opts: SearchOpts,
    show_hidden: bool,
}

pub(crate) struct Session {
    requests: Sender<Request>,
    pending: Receiver<Request>,
    generation: Arc<AtomicU64>,
}

impl Session {
    pub(crate) fn new(
        catalog: Catalog,
        events: reactor::Sender<WorkEvent>,
        respect_gitignore: bool,
        respect_ignore: bool,
    ) -> Self {
        let (requests, pending) = crossbeam_channel::bounded::<Request>(1);
        let worker_pending = pending.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let current = Arc::clone(&generation);
        std::thread::spawn(move || {
            let mut ignores = IgnoreMatcher::new(respect_gitignore, respect_ignore);
            'requests: while let Ok(mut request) = worker_pending.recv() {
                while let Ok(newer) = worker_pending.try_recv() {
                    request = newer;
                }
                let id = request.generation;
                let stale = || current.load(Ordering::Relaxed) != id;
                let limit = request.opts.limit;
                let catalog_len = catalog.len() as usize;
                let mut search_limit = if ignores.is_some() && limit > 0 {
                    limit.saturating_mul(8).min(catalog_len)
                } else {
                    limit
                };
                let mut rows = loop {
                    request.opts.limit = search_limit;
                    let hits = match catalog.search_with_hidden_cancel(
                        &request.query,
                        request.opts,
                        request.show_hidden,
                        stale,
                    ) {
                        Ok(hits) => hits,
                        Err(Error::Cancelled) => continue 'requests,
                        Err(error) => {
                            if !stale() {
                                let _ = events.send(WorkEvent::Query(Event::Hits(
                                    id,
                                    Err(error.to_string()),
                                )));
                            }
                            continue 'requests;
                        }
                    };
                    let exhausted = search_limit == 0
                        || hits.len() < search_limit
                        || search_limit >= catalog_len;
                    let mut rows: Vec<_> = hits.iter().map(Row::from_hit).collect();
                    if let Some(matcher) = ignores.as_mut() {
                        rows.retain(|row| !matcher.is_ignored(&row.path, row.is_dir));
                    }
                    if limit == 0 || rows.len() >= limit || exhausted {
                        break rows;
                    }
                    // Ignore-heavy queries widen only as far as needed instead of
                    // allocating the whole Catalog on every keystroke.
                    search_limit = search_limit.saturating_mul(4).min(catalog_len);
                };
                if limit > 0 && rows.len() > limit {
                    rows.truncate(limit);
                }
                if stale() || !events.send(WorkEvent::Query(Event::Hits(id, Ok(rows.clone())))) {
                    continue;
                }
                rows.par_iter_mut().for_each(|row| {
                    if !stale() && !row.is_dir {
                        row.size = std::fs::metadata(&row.path)
                            .map(|metadata| metadata.len())
                            .unwrap_or(row.size);
                    }
                });
                if !stale() {
                    let _ = events.send(WorkEvent::Query(Event::Sizes(id, rows)));
                }
            }
        });
        Self {
            requests,
            pending,
            generation,
        }
    }

    pub(crate) fn submit(&self, query: String, opts: SearchOpts, show_hidden: bool) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let mut request = Request {
            generation,
            query,
            opts,
            show_hidden,
        };
        loop {
            match self.requests.try_send(request) {
                Ok(()) => return generation,
                Err(TrySendError::Full(returned)) => {
                    request = returned;
                    let _ = self.pending.try_recv();
                }
                Err(TrySendError::Disconnected(_)) => return generation,
            }
        }
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Relaxed) == generation
    }
}
