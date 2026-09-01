"""Nautilus 4: Ctrl+F opens Qfind scoped to the current directory.

Nautilus 43+ has no search-provider hook (Tracker/LocalSearch is compiled in).
We capture Ctrl+F on each Files window (GTK4 ShortcutController, capture phase)
and launch qfind-gtk --here <current folder>.

Install: ~/.local/share/nautilus-python/extensions/qfind.py
Reload: nautilus -q
Do not gi.require_version('Nautilus') — the loader sets it (4.0 or 4.1).
"""

from __future__ import annotations

import os
import shutil
import subprocess
from typing import List, Optional

from gi.repository import GLib, GObject, Nautilus

QFIND_BIN = "qfind-gtk"


def _local_path(file_info: Nautilus.FileInfo) -> Optional[str]:
    if file_info.get_uri_scheme() != "file":
        return None
    loc = file_info.get_location()
    return loc.get_path() if loc is not None else None


def _parent_path(file_info: Nautilus.FileInfo) -> Optional[str]:
    parent = file_info.get_parent_location()
    if parent is not None:
        return parent.get_path()
    path = _local_path(file_info)
    if path is None:
        return None
    return os.path.dirname(path)


def _launch(root: Optional[str]) -> None:
    exe = shutil.which(QFIND_BIN)
    if exe is None:
        return
    cmd = [exe]
    env = os.environ.copy()
    if root:
        cmd.extend(["--here", root])
        env["QFIND_ROOT"] = root
    subprocess.Popen(
        cmd,
        env=env,
        start_new_session=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


class _Keys:
    here: Optional[str] = None
    hooked: set = set()
    started = False

    @classmethod
    def remember(cls, folder: Optional[str]) -> None:
        if folder:
            cls.here = folder

    @classmethod
    def ensure(cls) -> None:
        if cls.started:
            return
        cls.started = True
        GLib.timeout_add(350, cls._hook)

    @classmethod
    def _hook(cls) -> bool:
        try:
            from gi.repository import Gtk
        except ImportError:
            return True
        app = Gtk.Application.get_default()
        if app is None:
            return True
        for win in app.get_windows():
            wid = id(win)
            if wid in cls.hooked:
                continue
            cls.hooked.add(wid)
            cls._bind(win)
        return True

    @classmethod
    def _bind(cls, win) -> None:
        from gi.repository import Gtk

        ctrl = Gtk.ShortcutController()
        ctrl.set_scope(Gtk.ShortcutScope.GLOBAL)
        try:
            ctrl.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        except AttributeError:
            pass
        trigger = Gtk.ShortcutTrigger.parse_string("<Control>f")
        if trigger is None:
            return

        def on_search(*_args):
            _launch(cls.here)
            return True

        action = Gtk.CallbackAction.new(on_search)
        ctrl.add_shortcut(Gtk.Shortcut.new(trigger, action))
        win.add_controller(ctrl)


class QfindMenu(GObject.GObject, Nautilus.MenuProvider):
    def __init__(self) -> None:
        super().__init__()
        _Keys.ensure()

    def _item(self, name: str, root: Optional[str]) -> Nautilus.MenuItem:
        item = Nautilus.MenuItem(name=name, label="Search with Qfind")
        item.connect("activate", lambda *_: _launch(root))
        return item

    def get_background_items(self, *args) -> List[Nautilus.MenuItem]:
        current_folder = args[-1]
        root = _local_path(current_folder)
        _Keys.remember(root)
        if root is None:
            return []
        return [self._item("QfindMenuProvider::SearchHere", root)]

    def get_file_items(self, *args) -> List[Nautilus.MenuItem]:
        files = args[-1]
        if not files:
            return []
        target = files[0]
        root = _local_path(target) if target.is_directory() else _parent_path(target)
        _Keys.remember(_parent_path(target) or root)
        if root is None:
            return []
        return [self._item("QfindMenuProvider::Search", root)]


class QfindInfo(GObject.GObject, Nautilus.InfoProvider):
    """Track the folder Nautilus is showing so Ctrl+F has a --here path."""

    def __init__(self) -> None:
        super().__init__()
        _Keys.ensure()

    def update_file_info(self, file: Nautilus.FileInfo, *args) -> object:
        path = _parent_path(file)
        if path:
            _Keys.remember(path)
        if hasattr(Nautilus, "OperationResult"):
            return Nautilus.OperationResult.COMPLETE
        return None
