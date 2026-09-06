#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use async_channel::Sender as AsyncSender;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::{FileChooserAction, FileChooserDialog, FileFilter, ResponseType};
use zbus::Connection;
use zbus::connection::Builder;
use zbus::interface;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const SERVICE_NAME: &str = "org.freedesktop.impl.portal.desktop.qfind";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

type Options = HashMap<String, OwnedValue>;

#[derive(Default)]
struct RequestState {
    cancelled: AtomicBool,
}

struct PortalRequest {
    state: Arc<RequestState>,
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl PortalRequest {
    fn close(&self) -> zbus::fdo::Result<()> {
        self.state.cancelled.store(true, Ordering::Release);
        Ok(())
    }
}

#[derive(Clone)]
struct FilterSpec {
    name: String,
    rules: Vec<(u32, String)>,
}

struct PickSpec {
    title: String,
    accept_label: String,
    action: FileChooserAction,
    multiple: bool,
    current_folder: Option<PathBuf>,
    current_file: Option<PathBuf>,
    current_name: Option<String>,
    filters: Vec<FilterSpec>,
    save_files: Vec<Vec<u8>>,
}

struct UiRequest {
    handle: String,
    spec: PickSpec,
    state: Arc<RequestState>,
    response: AsyncSender<UiOutcome>,
}

enum UiOutcome {
    Selected(Vec<PathBuf>),
    Cancelled,
}

struct Pending {
    dialog: FileChooserDialog,
    state: Arc<RequestState>,
    response: AsyncSender<UiOutcome>,
}

#[derive(Clone)]
struct Backend {
    requests: Sender<UiRequest>,
    connection: Arc<Mutex<Option<Connection>>>,
}

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl Backend {
    async fn open_file(
        &self,
        handle: OwnedObjectPath,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: Options,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let connection = self.connection();
        let multiple = bool_option(&options, "multiple");
        let directory = bool_option(&options, "directory");
        self.ask(
            &connection,
            handle,
            spec_from_options(
                title,
                options,
                if directory {
                    FileChooserAction::SelectFolder
                } else {
                    FileChooserAction::Open
                },
                multiple && !directory,
            ),
        )
        .await
    }

    async fn save_file(
        &self,
        handle: OwnedObjectPath,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: Options,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let connection = self.connection();
        self.ask(
            &connection,
            handle,
            spec_from_options(title, options, FileChooserAction::Save, false),
        )
        .await
    }

    async fn save_files(
        &self,
        handle: OwnedObjectPath,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: Options,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let connection = self.connection();
        self.ask(
            &connection,
            handle,
            spec_from_options(title, options, FileChooserAction::SelectFolder, false),
        )
        .await
    }
}

impl Backend {
    async fn ask(
        &self,
        connection: &Connection,
        handle: OwnedObjectPath,
        spec: PickSpec,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let state = Arc::new(RequestState::default());
        connection
            .object_server()
            .at(
                handle.as_str(),
                PortalRequest {
                    state: Arc::clone(&state),
                },
            )
            .await
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        let (response, receiver) = async_channel::bounded(1);
        let sent = self.requests.send(UiRequest {
            handle: handle.to_string(),
            spec,
            state,
            response,
        });
        if let Err(error) = sent {
            let _ = connection
                .object_server()
                .remove::<PortalRequest, _>(handle.as_str())
                .await;
            return Err(zbus::fdo::Error::Failed(error.to_string()));
        }
        let outcome = receiver
            .recv()
            .await
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        let _ = connection
            .object_server()
            .remove::<PortalRequest, _>(handle.as_str())
            .await;
        match outcome {
            UiOutcome::Selected(paths) => Ok((0, result_options(paths))),
            UiOutcome::Cancelled => Ok((1, Options::new())),
        }
    }

    fn connection(&self) -> Connection {
        self.connection
            .lock()
            .expect("portal connection lock")
            .clone()
            .expect("portal connection initialized")
    }
}

fn bool_option(options: &Options, key: &str) -> bool {
    options
        .get(key)
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false)
}

fn string_option(options: &Options, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| String::try_from(value).ok())
}

fn bytes_option(options: &Options, key: &str) -> Option<Vec<u8>> {
    options
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<u8>::try_from(value).ok())
        .map(|bytes| bytes.into_iter().take_while(|byte| *byte != 0).collect())
}

fn path_option(options: &Options, key: &str) -> Option<PathBuf> {
    bytes_option(options, key).map(path_from_bytes)
}

fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from(OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn filters_option(options: &Options) -> Vec<FilterSpec> {
    options
        .get("filters")
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<(String, Vec<(u32, String)>)>::try_from(value).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(name, rules)| FilterSpec { name, rules })
        .collect()
}

fn files_option(options: &Options) -> Vec<Vec<u8>> {
    options
        .get("files")
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<Vec<u8>>::try_from(value).ok())
        .unwrap_or_default()
}

fn spec_from_options(
    title: &str,
    options: Options,
    action: FileChooserAction,
    multiple: bool,
) -> PickSpec {
    PickSpec {
        title: if title.is_empty() {
            "Megaman".into()
        } else {
            title.into()
        },
        accept_label: string_option(&options, "accept_label").unwrap_or_else(|| {
            if action == FileChooserAction::Save {
                "Save".into()
            } else {
                "Select".into()
            }
        }),
        action,
        multiple,
        current_folder: path_option(&options, "current_folder"),
        current_file: path_option(&options, "current_file"),
        current_name: string_option(&options, "current_name"),
        filters: filters_option(&options),
        save_files: files_option(&options),
    }
}

fn variant<T>(value: T) -> OwnedValue
where
    Value<'static>: From<T>,
{
    OwnedValue::try_from(Value::from(value)).expect("valid D-Bus variant")
}

fn result_options(paths: Vec<PathBuf>) -> Options {
    let uris = paths
        .into_iter()
        .map(|path| gio::File::for_path(path).uri().to_string())
        .collect::<Vec<_>>();
    Options::from([("uris".into(), variant(uris))])
}

fn add_filters(dialog: &FileChooserDialog, filters: &[FilterSpec]) {
    for filter_spec in filters {
        let filter = FileFilter::new();
        filter.set_name(Some(&filter_spec.name));
        for (kind, value) in &filter_spec.rules {
            if *kind == 1 {
                filter.add_mime_type(value);
            } else {
                filter.add_pattern(value);
            }
        }
        dialog.add_filter(&filter);
    }
}

fn chooser_paths(dialog: &FileChooserDialog) -> Vec<PathBuf> {
    let files = dialog.files();
    (0..files.n_items())
        .filter_map(|index| files.item(index))
        .filter_map(|file| file.downcast::<gio::File>().ok())
        .filter_map(|file| file.path())
        .collect()
}

fn show_picker(request: UiRequest, pending: &Rc<RefCell<HashMap<String, Pending>>>) {
    let dialog = FileChooserDialog::new(
        Some(&request.spec.title),
        None::<&gtk::Window>,
        request.spec.action,
        &[
            ("Cancel", ResponseType::Cancel),
            (&request.spec.accept_label, ResponseType::Accept),
        ],
    );
    dialog.set_modal(true);
    dialog.set_select_multiple(request.spec.multiple);
    if let Some(path) = request
        .spec
        .current_file
        .as_ref()
        .or(request.spec.current_folder.as_ref())
    {
        let file = gio::File::for_path(path);
        let _ = dialog.set_file(&file);
    }
    if let Some(name) = request.spec.current_name.as_deref() {
        dialog.set_current_name(name);
    }
    add_filters(&dialog, &request.spec.filters);
    let handle = request.handle.clone();
    let state = Arc::clone(&request.state);
    let response = request.response.clone();
    let pending_for_response = Rc::clone(pending);
    let save_files = request.spec.save_files.clone();
    dialog.connect_response(move |dialog, response_type| {
        let outcome = if state.cancelled.load(Ordering::Acquire) {
            UiOutcome::Cancelled
        } else if response_type == ResponseType::Accept {
            let mut paths = chooser_paths(dialog);
            if !save_files.is_empty() {
                if let Some(folder) = paths.pop() {
                    paths = save_files
                        .iter()
                        .map(|name| folder.join(path_from_bytes(name.clone())))
                        .collect();
                }
            }
            UiOutcome::Selected(paths)
        } else {
            UiOutcome::Cancelled
        };
        let _ = response.try_send(outcome);
        pending_for_response.borrow_mut().remove(&handle);
        dialog.destroy();
    });
    let handle = request.handle.clone();
    pending.borrow_mut().insert(
        handle,
        Pending {
            dialog: dialog.clone(),
            state: request.state,
            response: request.response,
        },
    );
    dialog.show();
}

fn drain_requests(
    receiver: &Rc<RefCell<Receiver<UiRequest>>>,
    pending: &Rc<RefCell<HashMap<String, Pending>>>,
) -> glib::ControlFlow {
    loop {
        let request = match receiver.borrow().try_recv() {
            Ok(request) => request,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
        };
        show_picker(request, pending);
    }
    let cancelled = pending
        .borrow()
        .iter()
        .filter(|(_, pending)| pending.state.cancelled.load(Ordering::Acquire))
        .map(|(handle, _)| handle.clone())
        .collect::<Vec<_>>();
    for handle in cancelled {
        if let Some(item) = pending.borrow_mut().remove(&handle) {
            item.dialog.destroy();
            let _ = item.response.try_send(UiOutcome::Cancelled);
        }
    }
    glib::ControlFlow::Continue
}

fn main() -> glib::ExitCode {
    // The backend is itself a portal client through GTK settings. Keep its
    // initialization from recursing into the broker that is starting it.
    unsafe {
        std::env::set_var("GIO_USE_PORTALS", "0");
        std::env::set_var("GSETTINGS_BACKEND", "memory");
    }
    let (requests, receiver) = mpsc::channel();
    let connection_cell = Arc::new(Mutex::new(None));
    let builder = Builder::session()
        .and_then(|builder| builder.name(SERVICE_NAME))
        .and_then(|builder| {
            builder.serve_at(
                PORTAL_PATH,
                Backend {
                    requests,
                    connection: Arc::clone(&connection_cell),
                },
            )
        })
        .expect("configure Megaman portal backend on the session bus");
    let _connection =
        zbus::block_on(builder.build()).expect("connect Megaman portal backend to the session bus");
    *connection_cell.lock().expect("portal connection lock") = Some(_connection.clone());
    gtk::init().expect("initialize GTK for the Megaman picker");

    let receiver = Rc::new(RefCell::new(receiver));
    let pending = Rc::new(RefCell::new(HashMap::new()));
    let started = Rc::new(Cell::new(false));
    let app = gtk::Application::builder()
        .application_id("org.qfind.Megaman.Portal")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    gtk::Window::set_default_icon_name("megaman");
    app.connect_activate({
        let receiver = Rc::clone(&receiver);
        let pending = Rc::clone(&pending);
        let started = Rc::clone(&started);
        move |_| {
            if started.replace(true) {
                return;
            }
            glib::timeout_add_local(Duration::from_millis(40), {
                let receiver = Rc::clone(&receiver);
                let pending = Rc::clone(&pending);
                move || drain_requests(&receiver, &pending)
            });
        }
    });
    let _hold = app.hold();
    app.run()
}
