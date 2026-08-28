"""Nautilus 4 MenuProvider: Search with Qfind.

Cannot replace Files' in-window search (Tracker/LocalSearch).
Opens qfind-gtk for the current or selected folder via QFIND_ROOT.

Install: ~/.local/share/nautilus-python/extensions/qfind.py
Reload: nautilus -q
Do not gi.require_version('Nautilus') — the loader sets it (4.0 or 4.1).
"""

from __future__ import annotations

import os
import shutil
import subprocess
from typing import List, Optional

from gi.repository import GObject, Nautilus

QFIND_BIN = "qfind-gtk"


def _local_path(file_info: Nautilus.FileInfo) -> Optional[str]:
    if file_info.get_uri_scheme() != "file":
        return None
    loc = file_info.get_location()
    return loc.get_path() if loc is not None else None


def _root_for(file_info: Nautilus.FileInfo) -> Optional[str]:
    path = _local_path(file_info)
    if path is None:
        return None
    if file_info.is_directory():
        return path
    parent = file_info.get_parent_location()
    if parent is not None:
        return parent.get_path()
    return os.path.dirname(path)


class QfindMenu(GObject.GObject, Nautilus.MenuProvider):
    def _launch(self, _item: Nautilus.MenuItem, root: Optional[str] = None) -> None:
        exe = shutil.which(QFIND_BIN)
        if exe is None:
            return
        env = os.environ.copy()
        if root:
            env["QFIND_ROOT"] = root
        subprocess.Popen(
            [exe],
            env=env,
            start_new_session=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def _item(self, name: str, root: Optional[str]) -> Nautilus.MenuItem:
        item = Nautilus.MenuItem(name=name, label="Search with Qfind")
        item.connect("activate", self._launch, root)
        return item

    def get_background_items(self, *args) -> List[Nautilus.MenuItem]:
        current_folder = args[-1]
        root = _root_for(current_folder)
        if root is None:
            return []
        return [self._item("QfindMenuProvider::SearchHere", root)]

    def get_file_items(self, *args) -> List[Nautilus.MenuItem]:
        files = args[-1]
        if not files or len(files) != 1:
            return []
        root = _root_for(files[0])
        if root is None:
            return []
        return [self._item("QfindMenuProvider::Search", root)]
