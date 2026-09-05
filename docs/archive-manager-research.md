# Archive-manager research

Research snapshot: 2026-09-02. Primary sources only: current Qfind source,
upstream project documentation and source at pinned commits, OS vendor APIs, and
the POSIX specification. Context7 was checked first for libarchive and .NET; the
current upstream sources were then used where Context7 was incomplete.

## Verdict

Yes. Qfind can present archives as folders, list their contents without
extracting everything, extract selected entries, and create or rewrite formats
the selected engine can write. Opening or executing a member is also feasible,
but it is never truly executed *inside* the archive: the member, and sometimes
its sibling files, must first be materialized on disk.

The smallest credible design is:

1. **libarchive as the one built-in engine** for structured listing, streaming
   preview, extraction, and rewrite-based creation/editing.
2. **One Qfind-owned archive location in the shared manager**, rendered by every
   frontend. Do not build separate GTK, WinUI, SwiftUI, and TUI implementations.
3. **One small detached lease helper** for extracted-and-opened members. Qfind
   may exit; the helper owns the temporary directory and its cleanup policy.
4. **Optional `7zz` compatibility later**, only for a demonstrated gap such as
   encrypted 7z interoperability. Do not start with two engines.

The requested exact rule—“delete when the opened file or all its descendants
close, even if Qfind exits”—cannot be guaranteed for arbitrary desktop
applications. Directly started processes can be tracked well. A file handed to
an already-running application, a D-Bus/DDE/LaunchServices activation, or a
daemon that deliberately detaches has no portable per-document lifetime. The
honest fallback is a persistent extraction lease with conservative expiry and a
manual **Clear extracted archive files** action.

## What Qfind has today

Qfind is close to the correct seam but is still filesystem-path-only:

- `ManagerSession`, its history, and `ManagerRow` store `PathBuf`; navigation
  rejects anything that is not an indexed filesystem directory
  ([manager](../crates/core/src/manager.rs#L26-L36),
  [row and navigation](../crates/core/src/manager.rs#L143-L188)).
- The native bridge exposes the same rows to macOS and Windows, but only as a
  path and `is_dir` flag ([native ABI](../crates/native/src/lib.rs#L13-L21)).
- Activation currently bypasses the manager: GTK calls `FileLauncher`, Windows
  calls `Launcher.LaunchFileAsync`, and macOS calls `NSWorkspace.open`
  ([GTK](../crates/gtk/src/actions.rs#L125-L142),
  [Windows](../apps/windows/MainWindow.xaml.cs#L131-L137),
  [macOS](../apps/macos/Sources/QfindMac/QfindMacApp.swift#L101-L104)).
- The accepted file-manager direction already places archive browse/extract
  before archive editing and notes that ordinary file operations still need
  progress, cancellation, conflicts, partial-failure reporting, and undo
  ([current gap table](file-manager-plan.md#L65-L75),
  [delivery order](file-manager-plan.md#L255-L275)).

Archive work therefore belongs behind `Manager`, not in click handlers. A row
needs an opaque item identity and kind—local path or archive entry—and frontends
must ask the manager to activate/extract it. Do not encode an archive member as
a fake filesystem path; it will leak into reveal, drag, preview, and native APIs
that require a real file.

## Engine comparison

| Choice | Browse without full extraction | Extract one member | Add/delete | Format reach | Integration and license | Verdict |
|---|---|---|---|---|---|---|
| **libarchive** | Yes; iterate headers and skip data | Yes; stream the selected entry | Rewrite a new archive for writable formats | Reads ZIP/ZIPX, 7z, RAR/RAR5, CAB, ISO, tar/cpio, LHA/LZH, XAR and more; writes ZIP/ZIPX, 7z, tar/cpio, ISO, XAR and more | Stable C API; permissive BSD-style license | **Built-in engine** |
| **Official `7zz` CLI** | Yes, via `l`/technical listing | Yes, via `x` or `-so` | `a`, `u`, `d`, and `rn` where the handler supports them | Excellent, including encrypted 7z/ZIP and many disk/package formats | Separate process and text protocol; LGPL 2.1-or-later plus BSD pieces and unRAR restriction | Optional compatibility backend |
| **7-Zip C++/DLL API** | Yes | Yes | `IOutArchive::UpdateItems` | Same handlers as the compiled build | Nonstandard COM-like C++ interfaces, callbacks, version coupling, no small stable C ABI | Too much ownership for Qfind |
| **7-Zip LZMA SDK** | Reduced 7z support | Reduced decoder | Not a full archive-manager backend | LZMA/LZMA2/XZ and reduced 7z scope | Public domain | Wrong product despite attractive license |
| **Native platform APIs** | Only platform-specific subsets | Subset | Subset | .NET is ZIP-focused; AppleArchive is Apple-only; Compression frameworks are codecs, not a shared archive manager | Would create three implementations | Reject as the core |

libarchive's own documentation says it is stream-oriented, uses one API across
formats, can read entry data directly to memory or another stream, and **does
not directly support in-place modification or true random access**
([formats and design](https://github.com/libarchive/libarchive/blob/9ad95ee5a70f2cccdb0cb0c68c43a3a874e571ec/README.md#L76-L182)).
That is a good fit for listing and extraction. Add/delete/rename must copy all
retained entries into a new sibling archive, finish it, sync it, and replace the
original only after success. The license permits binary redistribution with its
notice ([COPYING](https://github.com/libarchive/libarchive/blob/9ad95ee5a70f2cccdb0cb0c68c43a3a874e571ec/COPYING#L43-L65)).

libarchive's limitations must remain visible in the UI. RAR is read-only and
has proprietary-format limitations; 7z writing exists but encrypted 7z is not
equivalent to 7-Zip. KDE Ark likewise keeps its libarchive plugin as the required
base, but recommends external 7z for full 7z/ZIP support and notes that its
libarchive 7z path does not handle encryption
([Ark packager notes](https://invent.kde.org/utilities/ark/-/blob/3387b80bc1ed02cb7f444000482b90cd5927aa91/README.packagers),
[plugin capabilities](https://invent.kde.org/utilities/ark/-/blob/3387b80bc1ed02cb7f444000482b90cd5927aa91/README.md#L65-L79)).

Official 7-Zip provides add, update, delete, extract, list, and rename command
types ([command source](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/CPP/7zip/UI/Common/ArchiveCommandLine.h#L17-L30))
and broad packing/unpacking support ([official format list](https://www.7-zip.org/)).
Its developer FAQ explicitly offers either the DLL's nonstandard COM interfaces
or the command-line executable
([FAQ](https://www.7-zip.org/faq.html#developer_faq)). The CLI is the smaller
fallback, but `-slt` remains human-oriented text rather than a promised
versioned JSON protocol. Ark's sizeable parser and separate handling for
upstream 7-Zip versus old p7zip are useful warning signs
([Ark CLI plugin](https://invent.kde.org/utilities/ark/-/blob/3387b80bc1ed02cb7f444000482b90cd5927aa91/plugins/cli7zplugin/cliplugin.cpp)).

Full 7-Zip is LGPL 2.1-or-later with BSD-licensed pieces and an unRAR restriction
([official license](https://www.7-zip.org/license.txt)). The public-domain LZMA
SDK is only a subset, not a licensing shortcut to the full engine
([SDK scope](https://www.7-zip.org/sdk.html),
[reduced decoder limits](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/DOC/7zC.txt#L45-L52)).

Platform APIs do not replace the shared engine. .NET `ZipArchive` can read,
create, and update ZIP, but Microsoft's current guidance warns that update mode
can buffer large uncompressed entries and that extraction still requires path
validation
([ZIP guidance](https://github.com/dotnet/docs/blob/main/docs/standard/io/zip-tar-best-practices.md),
[traversal rule](https://github.com/dotnet/docs/blob/main/docs/fundamentals/code-analysis/quality-rules/ca5389.md)).
AppleArchive offers header/blob encode/decode streams, while Apple's Compression
framework exposes compression algorithms rather than a portable ZIP/7z/RAR
container API
([AppleArchive stream](https://developer.apple.com/documentation/applearchive/archivestreamprotocol),
[Compression](https://developer.apple.com/documentation/compression/)). Using
these in their respective frontends would make behavior and supported formats
diverge.

## What PeaZip, Ark, and File Roller actually do

These applications validate the product shape, not a reusable implementation.

**PeaZip** is an LGPLv3 Lazarus/FreePascal file and archive manager. Its breadth
comes from orchestrating 7-Zip/p7zip, FreeArc, ZPAQ, Brotli, Zstd, and other
backends rather than one embedded universal library
([overview](https://github.com/peazip/PeaZip/blob/a6219e9f3b7ded013c558b900d292c919ac3dc1d/README.md#L12-L39),
[source/build notes](https://github.com/peazip/PeaZip/blob/a6219e9f3b7ded013c558b900d292c919ac3dc1d/peazip-sources/readme.txt)).
It extracts a selected item before opening it. Current source clears preview and
work directories during exit; it does not establish that every external app or
descendant has stopped first
([open flow](https://github.com/peazip/PeaZip/blob/a6219e9f3b7ded013c558b900d292c919ac3dc1d/peazip-sources/dev/peach.pas#L45445-L45526),
[exit cleanup](https://github.com/peazip/PeaZip/blob/a6219e9f3b7ded013c558b900d292c919ac3dc1d/peazip-sources/dev/peach.pas#L34747-L34818)).

**Ark** extracts one entry into `QTemporaryDir`, rejects symlink opens, imposes a
preview-size limit, opens the real temporary path, watches writable members for
changes, and may offer to put a changed file back
([extract/open flow](https://invent.kde.org/utilities/ark/-/blob/3387b80bc1ed02cb7f444000482b90cd5927aa91/part/part.cpp#L950-L1038),
[safe temporary path](https://invent.kde.org/utilities/ark/-/blob/3387b80bc1ed02cb7f444000482b90cd5927aa91/kerfuffle/jobs.cpp#L612-L670)).
Ordinary open directories live until Ark's archive part is destroyed. Its
special external preview path asks KIO to delete temporary files when the
viewer application exits, which is still application lifetime, not document or
detached-descendant lifetime
([viewer](https://invent.kde.org/utilities/ark/-/blob/3387b80bc1ed02cb7f444000482b90cd5927aa91/part/arkviewer.cpp#L72-L86)).

**File Roller** also extracts to a work directory, launches through `GAppInfo`,
monitors writable extracted members, and removes recorded work directories at
application release
([open/extract path](https://gitlab.gnome.org/GNOME/file-roller/-/blob/c186fe9f7b0699819249a2f25047ad05fabdd877/src/fr-window.c#L8215-L8585),
[shutdown cleanup](https://gitlab.gnome.org/GNOME/file-roller/-/blob/c186fe9f7b0699819249a2f25047ad05fabdd877/src/fr-init.c#L620-L648)).
It combines libarchive with command backends, reinforcing that fallback engines
are compatibility work, not the first abstraction
([backend registry](https://gitlab.gnome.org/GNOME/file-roller/-/blob/c186fe9f7b0699819249a2f25047ad05fabdd877/src/fr-init.c#L340-L375)).

Copying any of these applications would import GPL/LGPL application code,
toolkit architecture, and years of backend conditionals. Reuse the behavior and
primary libraries, not their UI source.

## Archive-as-folder behavior

An opened archive should become a normal Qfind location with breadcrumbs such
as `Downloads / kit.7z / bin`, not a second application window.

- **Open archive:** scan headers on a worker and build an in-memory entry index.
  No entry payload is written to disk. Cache it only by source identity
  `(path, size, modified time)` and discard it when that identity changes.
- **Navigate:** folders may be explicit entries or synthesized from path
  prefixes. Back/Forward operate on archive locations exactly like filesystem
  locations. Parent from the archive root returns to the containing folder.
- **Preview:** stream a regular member into Qfind's bounded built-in preview
  where possible. A helper that requires a path receives a lease extraction.
- **Extract:** run as the same cancellable operation job required for ordinary
  copy/move. Apply conflict decisions and report partial completion.
- **Create/add/delete/rename:** expose only capabilities reported for the
  current archive. Never enable a generic Edit button merely from its suffix.
  Rebuild to a sibling temporary archive and replace on success; keep the
  original on cancellation or error.
- **Nested archive:** extract that member into a lease, then open the extracted
  file as another archive location. Do not invent recursive random access.
- **Drag out:** extraction. **Drag in:** add/update, only after archive mutation
  jobs exist. A virtual member is not a `text/uri-list` file until extracted.

Do not add archive entries to Qfind's persistent filename Catalog initially.
The archive source is already indexed; indexing every member introduces stale
identities, password prompts during rebuild, and potentially enormous indexes.
Search the current archive's in-memory entry index instead.

## Open and execute leases

Each open operation gets a private directory under Qfind's cache, mode `0700`
where POSIX permissions apply, plus a small manifest containing source archive
identity, member names, creation time, launch mode, and observed process state.
The lease helper, not the GUI, performs the launch and owns cleanup.

For a document, extract the selected member. For active content, extracting only
the `.exe` or script is often insufficient: it may load sibling DLLs, modules,
assets, or configuration later. The safe default is the selected member's
containing subtree, with a visible size/count estimate and confirmation. A
whole-archive option handles programs whose dependency layout is unknowable.

### What can be tracked

| Launch | Best available tracking | Honest cleanup |
|---|---|---|
| Direct child executable | Wait for direct child; also track descendants where the OS supports it | Delete when tracked set is empty, then retry if files are locked |
| Windows process tree | Job Object; children join by default unless breakaway rules or compatibility prevent it | Strong best effort while detached helper owns the Job handle |
| Linux process tree | Direct `wait`, pidfd for race-free identity, process group, and helper as child subreaper | Strong best effort for descendants; not for external D-Bus owners or deliberate escape |
| macOS direct app/process | `NSRunningApplication` or direct process wait/group | App/process lifetime only; no document-close signal |
| Associated document on any OS | May activate an already-running app with no owned process | Persistent lease; age/manual cleanup, never claim exact close detection |

Windows Job Objects manage a process group and normally include descendants,
but processes can break away and some nested-job situations affect assignment
([Microsoft Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)).
`ShellExecuteEx` with `SEE_MASK_NOCLOSEPROCESS` still returns no process handle
when an existing application or DDE handles the document
([SHELLEXECUTEINFO](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfoa)).
The official Russian and Spanish Job Object/ShellExecute documentation mirrors
state the same limitations
([Russian](https://learn.microsoft.com/ru-ru/windows/win32/procthread/job-objects),
[Spanish](https://learn.microsoft.com/es-es/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfoa));
there is no additional localized API that supplies document lifetime.

On Linux, a pidfd tracks one process without PID-reuse races
([`pidfd_open`](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)). A
child subreaper can adopt orphaned descendants, including a conventional
double-forked daemon
([`PR_SET_CHILD_SUBREAPER`](https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html)).
Neither identifies an already-running desktop app that receives the file over
D-Bus. GLib says a successful `GAppInfo` launch can still fail later, and its
launch signal may report PID 0 for D-Bus activation
([launch](https://docs.gtk.org/gio/method.AppInfo.launch.html),
[launch signal](https://docs.gtk.org/gio/signal.AppLaunchContext.launched.html)).

On macOS, `NSWorkspace` may return an `NSRunningApplication`, but that object
represents an application instance, not an open document
([open API](https://developer.apple.com/documentation/appkit/nsworkspace/openapplication(at:configuration:completionhandler:)),
[`NSRunningApplication`](https://developer.apple.com/documentation/appkit/nsrunningapplication)).
Closing the document while the editor stays open is therefore invisible.

If Qfind or the helper crashes, the lease remains. On the next start, Qfind may
remove old leases only after validating that each path is inside its exact
cache root, that recorded direct processes are gone, and that the lease has
passed a conservative age threshold. Always expose size, age, source archive,
**Open extracted folder**, **Keep**, and **Clear** controls. This is more honest
than deleting a file still in use or retaining hidden gigabytes forever.

## Security boundary

Archives are untrusted input. Before any disk write:

- normalize entry paths and reject absolute paths, `..`, Windows drive/UNC
  paths, alternate data streams, NULs, and platform separator tricks;
- reject device nodes and FIFOs; do not follow archive symlinks or hard links
  outside the exact destination;
- cap actual expanded bytes, entry count, per-entry size, nesting depth, CPU
  time, and output filesystem free-space use; declared sizes are only hints;
- never pass passwords on a process command line or retain them in the lease;
- never preserve setuid/setgid bits or elevate an extracted executable;
- require explicit confirmation for executables, scripts, installers,
  shortcuts, and other active content, showing archive and member paths;
- keep opened previews read-only initially. Writing a modified temp copy back
  into the archive is a separate, explicit later feature.

libarchive exposes secure-extraction flags for symlink traversal, `..`, and
absolute paths
([public API](https://github.com/libarchive/libarchive/blob/9ad95ee5a70f2cccdb0cb0c68c43a3a874e571ec/libarchive/archive.h#L714-L746)),
but Qfind still needs its own validation and quotas. Microsoft's ZIP guidance
also requires resolving each destination path and proving it remains under the
chosen root
([CA5389](https://github.com/dotnet/docs/blob/main/docs/fundamentals/code-analysis/quality-rules/ca5389.md)).

## Recommended delivery boundary

1. **Browse and extract:** libarchive, virtual archive location, bounded preview,
   and explicit extraction. This already makes Qfind useful as an archive
   browser without risking archive mutation.
2. **Lease-open:** detached helper, active-content confirmation, direct-process
   tracking, persistent fallback leases, and cleanup UI.
3. **Create and rewrite:** only after Qfind's ordinary operation jobs have
   cancellation, conflicts, partial-failure reporting, and safe replacement.
   Start with ZIP, 7z, and tar families that libarchive writes.
4. **Compatibility evidence:** add optional `7zz` only when actual archives show
   a required gap—most likely encrypted 7z/ZIP or a format libarchive cannot
   faithfully rewrite. Keep it behind the same archive interface and show which
   engine/capability is active.

Skipped deliberately: embedding PeaZip/Ark/File Roller, FUSE archive mounts, a
plugin ABI, archive-member Catalog indexing, exact document-close claims, and
automatic write-back of edited temporary files. Add one only after real use
proves the smaller design insufficient.
