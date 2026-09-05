import AppKit
import CQfind
import Darwin
import QuickLookUI
import SwiftUI
import UniformTypeIdentifiers

// MARK: - Model

enum ViewMode: String, CaseIterable, Identifiable {
    case icon, list, columns, gallery
    var id: String { rawValue }
    var title: String {
        switch self {
        case .icon: "Icon"
        case .list: "List"
        case .columns: "Columns"
        case .gallery: "Gallery"
        }
    }
    var symbol: String {
        switch self {
        case .icon: "square.grid.2x2"
        case .list: "list.bullet"
        case .columns: "rectangle.split.3x1"
        case .gallery: "photo.stack"
        }
    }
}

enum SortKey: String, CaseIterable, Identifiable {
    case name, size, newest, oldest
    var id: String { rawValue }
    var title: String {
        switch self {
        case .name: "Name"
        case .size: "Size"
        case .newest: "Newest"
        case .oldest: "Oldest"
        }
    }
}

private enum Workspace: String, CaseIterable, Identifiable {
    case storage, projects
    var id: String { rawValue }
    var title: String { rawValue.capitalized }
    var symbol: String { self == .storage ? "externaldrive" : "hammer" }
}

private struct ComponentCommand: Identifiable {
    let id: String
    let title: String
    let mutating: Bool
}

private struct ComponentDescriptor: Identifiable {
    let id: String
    let title: String
    let icon: String
    let commands: [ComponentCommand]
}

private func nativeSymbol(_ icon: String) -> String {
    switch icon {
    case "repository": "arrow.triangle.branch"
    case "branch": "arrow.triangle.branch"
    case "terminal": "terminal"
    case "disk": "internaldrive"
    case "files": "square.stack.3d.up"
    case "archive": "archivebox"
    default: icon
    }
}

private struct ProjectRecord: Identifiable {
    let path: String
    let repository: String
    let branch: String
    let rust: Bool
    let node: Bool
    let bytes: UInt64
    let artifacts: [(path: String, bytes: UInt64)]
    var id: String { path }
    var name: String { URL(fileURLWithPath: path).lastPathComponent }
    var kind: String {
        [rust ? "Rust" : nil, node ? "JS" : nil].compactMap { $0 }.joined(separator: " · ")
    }
}

private struct StorageRecord: Identifiable {
    let name: String
    let path: String
    let bytes: UInt64
    let isDirectory: Bool
    var id: String { path }
}

private struct BatchRecord: Identifiable {
    let from: String
    let to: String
    var id: String { from + "\u{0}" + to }
}

private struct TaskCommand: Identifiable {
    let id: String
    let title: String
}

private struct FileRow: Identifiable, Hashable {
    let coreID: UInt32
    let name: String
    let path: String
    let bytes: UInt64
    let entries: UInt64
    let isDirectory: Bool
    var modified: Date = .distantPast
    var id: String { path }
    var url: URL { URL(fileURLWithPath: path) }
    var parent: String { (path as NSString).deletingLastPathComponent }
    var ext: String { (name as NSString).pathExtension.uppercased() }
    var kind: String { isDirectory ? "Folder" : (ext.isEmpty ? "Document" : "\(ext) document") }
    var sizeString: String {
        if isDirectory {
            if bytes > 0 { return ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file) }
            return entries > 1 ? "\(entries) items" : "—"
        }
        return ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }
    var dateString: String {
        modified == .distantPast ? "—" : FileRow.dates.string(from: modified)
    }
    private static let dates: DateFormatter = {
        let format = DateFormatter()
        format.dateStyle = .medium
        format.timeStyle = .short
        return format
    }()
}

private final class RowsBox {
    var rows: [FileRow] = []
}

private let collectRow: QfindRowCallback = { context, pointer in
    guard let context, let row = pointer?.pointee else { return }
    Unmanaged<RowsBox>.fromOpaque(context).takeUnretainedValue().rows.append(FileRow(
        coreID: row.id,
        name: String(cString: row.name!),
        path: String(cString: row.path!),
        bytes: row.bytes,
        entries: row.entries,
        isDirectory: row.is_dir != 0
    ))
}

private final class TextBox {
    var value = ""
}

private let collectText: QfindTextCallback = { context, pointer in
    guard let context, let pointer else { return }
    Unmanaged<TextBox>.fromOpaque(context).takeUnretainedValue().value = String(cString: pointer)
}

/// Serializes the non-Sendable C handle's ownership across the native queue.
/// Captured operations keep this box alive until the Rust manager call returns.
private final class NativeManager: @unchecked Sendable {
    let pointer: OpaquePointer

    init(_ pointer: OpaquePointer) { self.pointer = pointer }
    deinit { qfind_manager_free(pointer) }
}

// MARK: - Liquid Glass accent

extension View {
    /// Liquid Glass on macOS 26+, translucent material fallback below.
    /// Standard toolbars, sidebars and search fields already render as
    /// Liquid Glass on macOS 26, so this only accents custom cards.
    @ViewBuilder
    func qfindGlass(cornerRadius: CGFloat = 12) -> some View {
        #if compiler(>=6.2)
        if #available(macOS 26, *) {
            self.glassEffect(.regular, in: RoundedRectangle(cornerRadius: cornerRadius))
        } else {
            self.background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: cornerRadius))
        }
        #else
        self.background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: cornerRadius))
        #endif
    }
}

// MARK: - Browser (Rust core via the stable C ABI)

@MainActor
private final class Browser: ObservableObject {
    @Published var rows: [FileRow] = []
    @Published var subtree: [FileRow] = []
    @Published var chart: [FileRow] = []
    @Published var selectedPath: String?
    @Published var selectedPaths: Set<String> = []
    @Published var hoveredPath: String?
    @Published var query = ""
    @Published var directory: String
    @Published var workspace: Workspace = .storage
    @Published var recursive = false
    @Published var globalSearch = false
    @Published var viewMode: ViewMode = .icon
    @Published var sortKey: SortKey = .name
    @Published var ascending = true
    @Published var foldersFirst = true
    @Published var showKindColumn = false
    @Published var showModifiedColumn = true
    @Published var showSizeColumn = true
    @Published var nameColumnWidth = 280.0
    @Published var kindColumnWidth = 120.0
    @Published var modifiedColumnWidth = 150.0
    @Published var sizeColumnWidth = 100.0
    @Published var showInspector = true
    @Published var showChart = false
    @Published var showPathBar = true
    @Published var showStatusBar = true
    @Published var density = 128.0
    @Published var favorites: [String] = []
    @Published var recents: [String] = []
    @Published var operationError: String?
    @Published var components: [ComponentDescriptor] = []
    @Published var projects: [ProjectRecord] = []
    @Published var selectedProjectPath: String?
    @Published var projectDiff = ""
    @Published var projectGitStatus = ""
    @Published var projectGitFiles: [String] = []
    @Published var projectDiffMode = false
    @Published var projectGitFile = ""
    @Published var gitFooterStatus = ""
    @Published var projectTasks: [TaskCommand] = []
    @Published var taskOutput = ""
    @Published var storageEntries: [StorageRecord] = []
    @Published var storageFree: UInt64 = 0
    @Published var storageTotal: UInt64 = 0
    @Published var showBatch = false
    @Published var batchAction = "rename"
    @Published var batchPaths = ""
    @Published var batchDestination = ""
    @Published var batchFind = ""
    @Published var batchReplace = ""
    @Published var batchPrefix = ""
    @Published var batchSuffix = ""
    @Published var batchPreview: [BatchRecord] = []
    @Published var batchOutput = ""
    @Published var selectionName = ""
    @Published var selectionExtension = ""
    @Published var selectionKind = "all"
    @Published var showArchive = false
    @Published var archiveAction = "open"
    @Published var archivePath = ""
    @Published var archiveDestination = ""
    @Published var archivePaths = ""
    @Published var archiveOutput = ""

    private var manager: NativeManager?
    private let ioQueue = DispatchQueue(label: "music.derpcat.megaman.native", qos: .userInitiated)
    private let componentQueue = DispatchQueue(label: "music.derpcat.megaman.components", qos: .userInitiated)
    private let taskQueue = DispatchQueue(label: "music.derpcat.megaman.tasks", qos: .userInitiated)
    private let fileQueue = DispatchQueue(label: "music.derpcat.megaman.files", qos: .userInitiated)
    private var generation = 0
    private var chartGeneration = 0
    private var storageGeneration = 0
    private var folderSizeRevision: UInt64 = 0
    private var folderSizePollTask: Task<Void, Never>?
    private var directoryWatcher: DispatchSourceFileSystemObject?
    private var directoryWatcherGeneration = 0
    private var directoryRefreshTask: Task<Void, Never>?
    private var gitFooterPath = ""
    private var gitFooterLastRequest = Date.distantPast
    private var gitFooterGeneration = 0

    var selectedRow: FileRow? {
        guard let selectedPath else { return nil }
        return (rows + subtree).first(where: { $0.path == selectedPath })
    }

    var selectedRows: [FileRow] {
        let paths = selectedPaths.isEmpty ? Set(selectedPath.map { [$0] } ?? []) : selectedPaths
        var seen = Set<String>()
        return (rows + subtree).filter { paths.contains($0.path) && seen.insert($0.path).inserted }
    }

    var footerGitSummary: String {
        if selectedProjectPath == directory, !projectGitStatus.isEmpty {
            return Self.gitFooterLabel(projectGitStatus)
        }
        return gitFooterStatus
    }

    func select(_ path: String) {
        let modifiers = NSApp.currentEvent?.modifierFlags ?? []
        if modifiers.contains(.shift), let anchor = selectedPath,
           let first = rows.firstIndex(where: { $0.path == anchor }),
           let last = rows.firstIndex(where: { $0.path == path }) {
            let range = min(first, last)...max(first, last)
            selectedPaths = Set(rows[range].map(\.path))
        } else if modifiers.contains(.command) {
            if selectedPaths.contains(path) { selectedPaths.remove(path) }
            else { selectedPaths.insert(path) }
        } else {
            selectedPaths = [path]
        }
        selectedPath = selectedPaths.contains(path) ? path : selectedPaths.first
    }

    func selectByCriteria() {
        let name = selectionName.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let ext = selectionExtension.trimmingCharacters(in: CharacterSet(charactersIn: ". ")).uppercased()
        let matches = rows.filter { row in
            let kindMatches = selectionKind == "all" || (selectionKind == "folders" && row.isDirectory) || (selectionKind == "files" && !row.isDirectory)
            let nameMatches = name.isEmpty || row.name.lowercased().contains(name)
            let extensionMatches = ext.isEmpty || row.ext == ext
            return kindMatches && nameMatches && extensionMatches
        }
        selectedPaths = Set(matches.map(\.path))
        selectedPath = matches.first?.path
    }

    func clearSelection() {
        selectedPath = nil
        selectedPaths.removeAll()
    }

    var sortedRows: [FileRow] {
        rows.sorted {
            if foldersFirst, $0.isDirectory != $1.isDirectory { return $0.isDirectory }
            switch sortKey {
            case .name: return ascending ? $0.name.localizedStandardCompare($1.name) == .orderedAscending : $0.name.localizedStandardCompare($1.name) == .orderedDescending
            case .size: return ascending ? $0.bytes < $1.bytes : $0.bytes > $1.bytes
            case .newest, .oldest: return ascending ? $0.modified < $1.modified : $0.modified > $1.modified
            }
        }
    }

    /// Column trail derived from one recursive fetch: no C-ABI change needed.
    /// Flat results replace columns while a Query is current (Finder behavior).
    var columns: [[FileRow]] {
        if subtree.isEmpty {
            return query.isEmpty && !rows.isEmpty ? [sortedRows] : []
        }
        var trail: [[FileRow]] = []
        var current = directory
        var depth = 0
        while depth < 8 {
            let kids = subtree.filter { $0.parent == current }
                .sorted { $0.isDirectory != $1.isDirectory ? $0.isDirectory : $0.name.localizedStandardCompare($1.name) == .orderedAscending }
            guard !kids.isEmpty else { break }
            trail.append(kids)
            guard let selectedPath, selectedPath.hasPrefix(current + "/") else { break }
            let comps = (selectedPath as NSString).pathComponents
            let base = (current as NSString).pathComponents.count
            guard comps.count > base else { break }
            let next = NSString.path(withComponents: Array(comps.prefix(base + 1)))
            guard next != current,
                  subtree.contains(where: { $0.path == next && $0.isDirectory }) else { break }
            current = next
            depth += 1
        }
        return trail
    }

    convenience init() {
        self.init(directory: FileManager.default.homeDirectoryForCurrentUser.path)
    }

    init(directory initialDirectory: String) {
        directory = initialDirectory
        favorites = UserDefaults.standard.stringArray(forKey: "qfind.favorites") ?? []
        recents = UserDefaults.standard.stringArray(forKey: "qfind.recents") ?? []
        let defaults = UserDefaults.standard
        showKindColumn = defaults.object(forKey: "megaman.column.kind") as? Bool ?? false
        showModifiedColumn = defaults.object(forKey: "megaman.column.modified") as? Bool ?? true
        showSizeColumn = defaults.object(forKey: "megaman.column.size") as? Bool ?? true
        nameColumnWidth = defaults.object(forKey: "megaman.column.nameWidth") as? Double ?? 280
        kindColumnWidth = defaults.object(forKey: "megaman.column.kindWidth") as? Double ?? 120
        modifiedColumnWidth = defaults.object(forKey: "megaman.column.modifiedWidth") as? Double ?? 150
        sizeColumnWidth = defaults.object(forKey: "megaman.column.sizeWidth") as? Double ?? 100
        manager = nil
        ioQueue.async { [weak self] in
            guard let handle = initialDirectory.withCString({ qfind_manager_open($0) }) else {
                DispatchQueue.main.async { [weak self] in self?.operationError = "Megaman could not open its Rust catalog bridge." }
                return
            }
            let manager = NativeManager(handle)
            let folderSizeRevision = qfind_folder_sizes_revision()
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.manager = manager
                self.startFolderSizePolling(initialRevision: folderSizeRevision)
                self.startDirectoryWatcher()
                self.refreshNow()
                self.loadComponents()
            }
        }
    }

    /// Watch only the current directory; the Rust query remains the source of truth.
    private func startDirectoryWatcher() {
        directoryWatcherGeneration += 1
        let current = directoryWatcherGeneration
        directoryWatcher?.cancel()
        directoryWatcher = nil
        let path = directory
        ioQueue.async { [weak self] in
            guard let self else { return }
            let descriptor = path.withCString { Darwin.open($0, O_EVTONLY) }
            guard descriptor >= 0 else { return }
            let source = DispatchSource.makeFileSystemObjectSource(
                fileDescriptor: descriptor,
                eventMask: [.write, .delete, .rename, .extend, .attrib, .link, .revoke],
                queue: self.ioQueue
            )
            source.setEventHandler { [weak self] in
                DispatchQueue.main.async { [weak self] in
                    guard let self,
                          self.directoryWatcherGeneration == current,
                          self.directory == path else { return }
                    self.scheduleDirectoryRefresh()
                }
            }
            source.setCancelHandler { _ = Darwin.close(descriptor) }
            source.resume()
            DispatchQueue.main.async { [weak self] in
                guard let self,
                      self.directoryWatcherGeneration == current,
                      self.directory == path else {
                    source.cancel()
                    return
                }
                self.directoryWatcher = source
            }
        }
    }

    private func scheduleDirectoryRefresh() {
        directoryRefreshTask?.cancel()
        directoryRefreshTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 150_000_000)
            guard let self, !Task.isCancelled else { return }
            self.refreshNow()
        }
    }

    private func startFolderSizePolling(initialRevision: UInt64? = nil) {
        guard folderSizePollTask == nil else { return }
        folderSizeRevision = initialRevision ?? qfind_folder_sizes_revision()
        folderSizePollTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                guard let self else { return }
                let revision = qfind_folder_sizes_revision()
                guard revision != self.folderSizeRevision else { continue }
                self.folderSizeRevision = revision
                self.generation += 1
                self.performFetch()
            }
        }
    }

    /// Debounced: only the most recently typed Query may replace its Hits.
    func refresh() {
        generation += 1
        let current = generation
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 120_000_000)
            guard let self, current == self.generation else { return }
            self.performFetch()
        }
    }

    func refreshNow() {
        generation += 1
        performFetch()
        refreshFooterGitStatus(force: true)
    }

    private struct NativeRows {
        let rows: [FileRow]
        let error: String?
    }

    private nonisolated static func json(_ data: Data) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }

    private nonisolated static func text(_ value: Any?) -> String {
        value as? String ?? ""
    }

    private nonisolated static func unsigned(_ value: Any?) -> UInt64 {
        if let number = value as? NSNumber { return number.uint64Value }
        if let number = value as? UInt64 { return number }
        if let number = value as? Int, number >= 0 { return UInt64(number) }
        return 0
    }

    private nonisolated static func callComponent(_ manager: NativeManager, id: String, request: [String: Any]) -> (Data?, String?) {
        guard let json = try? JSONSerialization.data(withJSONObject: request),
              let request = String(data: json, encoding: .utf8) else {
            return (nil, "Megaman could not encode the component request.")
        }
        let box = TextBox()
        let context = Unmanaged.passUnretained(box).toOpaque()
        let status = id.withCString { component in
            request.withCString { payload in
                qfind_manager_component(manager.pointer, component, payload, collectText, context)
            }
        }
        guard status == 0 else {
            return (nil, box.value.isEmpty ? error(manager, status: status, fallback: "Megaman component failed.") : box.value)
        }
        return (Data(box.value.utf8), nil)
    }

    private nonisolated static func parseComponents(_ data: Data) -> [ComponentDescriptor] {
        let root = json(data)
        return (root["components"] as? [[String: Any]] ?? []).compactMap { value in
            let id = text(value["id"])
            guard !id.isEmpty else { return nil }
            let commands = (value["commands"] as? [[String: Any]] ?? []).compactMap { command -> ComponentCommand? in
                let commandID = text(command["id"])
                guard !commandID.isEmpty else { return nil }
                return ComponentCommand(id: commandID, title: text(command["title"]).isEmpty ? commandID : text(command["title"]), mutating: command["mutating"] as? Bool ?? false)
            }
            return ComponentDescriptor(id: id, title: text(value["title"]).isEmpty ? id.capitalized : text(value["title"]), icon: text(value["icon"]).isEmpty ? "square.grid.2x2" : text(value["icon"]), commands: commands)
        }
    }

    private nonisolated static func parseProjects(_ data: Data) -> [ProjectRecord] {
        let root = json(data)
        return (root["projects"] as? [[String: Any]] ?? []).compactMap { value in
            let path = text(value["path"])
            guard !path.isEmpty else { return nil }
            let artifacts = (value["artifacts"] as? [[String: Any]] ?? []).compactMap { artifact -> (path: String, bytes: UInt64)? in
                let artifactPath = text(artifact["path"])
                guard !artifactPath.isEmpty else { return nil }
                return (artifactPath, unsigned(artifact["bytes"]))
            }
            return ProjectRecord(path: path, repository: text(value["repository"]), branch: text(value["branch"]), rust: value["rust"] as? Bool ?? false, node: value["node"] as? Bool ?? false, bytes: unsigned(value["bytes"]), artifacts: artifacts)
        }.sorted { $0.repository.localizedStandardCompare($1.repository) == .orderedAscending }
    }

    private nonisolated static func parseStorage(_ data: Data) -> ([StorageRecord], UInt64, UInt64) {
        let root = json(data)
        let entries = (root["entries"] as? [[String: Any]] ?? []).compactMap { value -> StorageRecord? in
            let path = text(value["path"])
            guard !path.isEmpty else { return nil }
            return StorageRecord(name: text(value["name"]), path: path, bytes: unsigned(value["bytes"]), isDirectory: value["is_dir"] as? Bool ?? false)
        }
        return (entries, unsigned(root["free"]), unsigned(root["total"]))
    }

    private nonisolated static func parseBatch(_ data: Data) -> ([BatchRecord], String) {
        let root = json(data)
        let items = (root["items"] as? [[String: Any]] ?? []).compactMap { value -> BatchRecord? in
            let from = text(value["from"])
            let to = text(value["to"])
            guard !from.isEmpty || !to.isEmpty else { return nil }
            return BatchRecord(from: from, to: to)
        }
        return (items, text(root["text"]))
    }

    private nonisolated static func gitFooterLabel(_ status: String) -> String {
        let lines = status.split(whereSeparator: \.isNewline).map(String.init)
        guard !lines.isEmpty else { return "" }
        let header = lines.first(where: { $0.hasPrefix("##") })
        let branch: String
        if let header {
            let value = header.dropFirst(2).trimmingCharacters(in: .whitespaces)
            branch = value.components(separatedBy: "...").first ?? value
        } else if let line = lines.first(where: { $0.hasPrefix("On branch ") }) {
            branch = String(line.dropFirst("On branch ".count))
        } else {
            branch = "repository"
        }
        let changed = lines.filter { !$0.hasPrefix("##") && !$0.trimmingCharacters(in: .whitespaces).isEmpty }.count
        return "Git · " + (branch.isEmpty ? "repository" : branch) + " · " + (changed == 0 ? "clean" : "\(changed) changed")
    }

    private nonisolated static func fetchRows(_ manager: NativeManager, query: String, recursive: Bool) -> NativeRows {
        let box = RowsBox()
        let context = Unmanaged.passUnretained(box).toOpaque()
        let status = query.withCString { qfind_manager_rows(manager.pointer, $0, recursive ? 1 : 0, 5_000, collectRow, context) }
        var dates: [String: Date] = [:]
        for row in box.rows {
            if let values = try? row.url.resourceValues(forKeys: [.contentModificationDateKey]),
               let date = values.contentModificationDate {
                dates[row.path] = date
            }
        }
        let rows = box.rows.map { var row = $0; row.modified = dates[row.path] ?? .distantPast; return row }
        return NativeRows(rows: rows, error: status == 0 ? nil : error(manager, status: status, fallback: "Megaman could not read this folder."))
    }

    private func performFetch() {
        guard let manager else { return }
        let current = generation
        let query = query
        let recursive = recursive
        let columns = viewMode == .columns && query.isEmpty
        ioQueue.async { [weak self, manager] in
            let result = Self.fetchRows(manager, query: query, recursive: recursive)
            let subtree = columns ? Self.fetchRows(manager, query: "", recursive: true) : NativeRows(rows: [], error: nil)
            DispatchQueue.main.async {
                guard let self, current == self.generation else { return }
                self.rows = result.rows
                self.subtree = subtree.rows
                if let error = result.error ?? subtree.error { self.operationError = error }
                let visiblePaths = Set((self.rows + self.subtree).map(\.path))
                self.selectedPaths.formIntersection(visiblePaths)
                if let selectedPath = self.selectedPath, !visiblePaths.contains(selectedPath) {
                    self.selectedPath = self.selectedPaths.first
                }
                if self.showChart { self.refreshChart(); self.refreshStorageComponent() }
            }
        }
    }

    private func refreshFooterGitStatus(force: Bool = false) {
        guard let manager else { return }
        let path = directory
        if !force, path == gitFooterPath,
           Date().timeIntervalSince(gitFooterLastRequest) < 5 { return }
        gitFooterPath = path
        gitFooterLastRequest = Date()
        gitFooterGeneration += 1
        let current = gitFooterGeneration
        componentQueue.async { [weak self, manager] in
            let (data, _) = Self.callComponent(manager, id: "git", request: ["action": "status", "path": path, "staged": false])
            let status = data.map { Self.text(Self.json($0)["status"]) } ?? ""
            DispatchQueue.main.async {
                guard let self, current == self.gitFooterGeneration, self.directory == path else { return }
                if !status.isEmpty { self.gitFooterStatus = Self.gitFooterLabel(status) }
            }
        }
    }

    func refreshChart() {
        guard let manager else { return }
        chartGeneration += 1
        let current = chartGeneration
        let path = directory
        ioQueue.async { [weak self, manager] in
            let box = RowsBox()
            let context = Unmanaged.passUnretained(box).toOpaque()
            let status = qfind_manager_chart(manager.pointer, 0, 24, collectRow, context)
            let error = status == 0 ? nil : Self.error(manager, status: status, fallback: "Megaman could not load the storage map.")
            DispatchQueue.main.async {
                guard let self, current == self.chartGeneration, self.directory == path else { return }
                self.chart = box.rows
                if let error { self.operationError = error }
            }
        }
    }

    func loadComponents() {
        guard let manager else { return }
        componentQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "shell", request: ["action": "list"])
            DispatchQueue.main.async {
                guard let self else { return }
                if let error { self.operationError = error; return }
                self.components = data.map(Self.parseComponents) ?? []
            }
        }
    }

    func refreshProjects(force: Bool = false) {
        guard let manager else { return }
        componentQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "projects", request: ["action": force ? "refresh" : "list"])
            DispatchQueue.main.async {
                guard let self else { return }
                if let error { self.operationError = error; return }
                self.projects = data.map(Self.parseProjects) ?? []
                if let selectedProjectPath = self.selectedProjectPath,
                   !self.projects.contains(where: { $0.path == selectedProjectPath }) {
                    self.selectedProjectPath = nil
                }
            }
        }
    }

    var selectedProject: ProjectRecord? {
        guard let selectedProjectPath else { return nil }
        return projects.first(where: { $0.path == selectedProjectPath })
    }

    func selectProject(_ project: ProjectRecord) {
        selectedProjectPath = project.path
        projectGitFile = ""
        projectDiff = ""
        projectGitStatus = ""
        projectGitFiles = []
        projectTasks = []
        taskOutput = ""
        refreshProjectGit(project, action: "diff")
        refreshTasks(project)
    }

    private func refreshProjectGit(_ project: ProjectRecord, action: String) {
        guard let manager else { return }
        var request: [String: Any] = ["action": action, "path": project.path, "staged": false]
        let file = projectGitFile.trimmingCharacters(in: .whitespacesAndNewlines)
        if !file.isEmpty { request["file"] = file }
        componentQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "git", request: request)
            let root = data.map(Self.json) ?? [:]
            let text = Self.text(root["text"])
            let status = Self.text(root["status"])
            let files = (root["files"] as? [String]) ?? []
            DispatchQueue.main.async {
                guard let self, self.selectedProjectPath == project.path else { return }
                if let error { self.operationError = error }
                else if action == "stage" || action == "unstage" { self.refreshProjectGit(project, action: "diff") }
                else {
                    self.projectDiff = text
                    self.projectGitStatus = status
                    self.projectGitFiles = files
                    if self.directory == project.path {
                        self.gitFooterGeneration += 1
                        self.gitFooterPath = project.path
                        self.gitFooterLastRequest = Date()
                        self.gitFooterStatus = Self.gitFooterLabel(status)
                    }
                }
            }
        }
    }

    func refreshProjectDiff() {
        guard let project = selectedProject else { return }
        refreshProjectGit(project, action: "diff")
    }

    func runGit(_ action: String) {
        guard let project = selectedProject else { return }
        refreshProjectGit(project, action: action)
    }

    private func refreshTasks(_ project: ProjectRecord) {
        guard let manager else { return }
        componentQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "tasks", request: ["action": "list", "path": project.path])
            let root = data.map(Self.json) ?? [:]
            let tasks = (root["commands"] as? [[String: Any]] ?? []).compactMap { command -> TaskCommand? in
                let id = Self.text(command["id"])
                guard !id.isEmpty else { return nil }
                let title = Self.text(command["title"])
                return TaskCommand(id: id, title: title.isEmpty ? id : title)
            }
            DispatchQueue.main.async {
                guard let self, self.selectedProjectPath == project.path else { return }
                if let error { self.operationError = error }
                else { self.projectTasks = tasks }
            }
        }
    }

    func runTask(_ task: TaskCommand) {
        guard let manager, let project = selectedProject else { return }
        taskQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "tasks", request: ["action": "run", "path": project.path, "command": task.id])
            let text = data.map { Self.text(Self.json($0)["text"]) } ?? ""
            DispatchQueue.main.async {
                guard let self, self.selectedProjectPath == project.path else { return }
                if let error { self.operationError = error }
                else { self.taskOutput = text }
            }
        }
    }

    func refreshStorageComponent() {
        guard let manager else { return }
        let path = directory
        storageGeneration += 1
        let current = storageGeneration
        componentQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "storage", request: ["action": "map", "path": path])
            let result = data.map(Self.parseStorage) ?? ([], 0, 0)
            DispatchQueue.main.async {
                guard let self, current == self.storageGeneration, self.directory == path else { return }
                if let error {
                    self.storageEntries = []
                    self.storageFree = 0
                    self.storageTotal = 0
                    self.operationError = error
                } else {
                    self.storageEntries = result.0
                    self.storageFree = result.1
                    self.storageTotal = result.2
                }
            }
        }
    }

    func runComponent(_ component: ComponentDescriptor, command: ComponentCommand) {
        if component.id == "projects" {
            refreshProjects(force: true)
            return
        }
        if component.id == "storage", command.id == "map" {
            refreshStorageComponent()
            return
        }
        if component.id == "tasks", command.id == "list", let project = selectedProject {
            refreshTasks(project)
            return
        }
        if component.id == "git", ["status", "diff"].contains(command.id), let project = selectedProject {
            refreshProjectGit(project, action: command.id)
            return
        }
        if component.id == "batch" {
            showBatch = true
            showArchive = false
            if ["rename", "copy", "move"].contains(command.id) { batchAction = command.id }
            if command.id == "rename_preview" { runBatch(preview: true) }
            return
        }
        if component.id == "archives" {
            showArchive = true
            showBatch = false
            archiveAction = command.id
            return
        }
        guard let manager else { return }
        let request: [String: Any] = ["action": command.id, "path": directory]
        componentQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: component.id, request: request)
            let text = data.map { Self.text(Self.json($0)["text"]) } ?? ""
            DispatchQueue.main.async {
                guard let self else { return }
                if let error { self.operationError = error }
                else if !text.isEmpty { self.batchOutput = text }
            }
        }
    }

    func runBatch(preview: Bool) {
        guard let manager else { return }
        let typedPaths = batchPaths.split(whereSeparator: { $0.isNewline }).map(String.init)
        let paths = typedPaths.isEmpty ? selectedRows.map(\.path) : typedPaths
        guard !paths.isEmpty else { return }
        let action = batchAction == "rename" && preview ? "rename_preview" : batchAction
        var request: [String: Any] = [
            "action": action,
            "paths": paths,
            "destination": batchDestination,
            "find": batchFind,
            "replace": batchReplace,
            "prefix": batchPrefix,
            "suffix": batchSuffix,
            "start": 1,
        ]
        if batchAction == "rename" && !preview { request["action"] = "rename" }
        fileQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "batch", request: request)
            let result = data.map(Self.parseBatch) ?? ([], "")
            DispatchQueue.main.async {
                guard let self else { return }
                if let error { self.operationError = error }
                else {
                    self.batchPreview = result.0
                    self.batchOutput = result.1
                }
            }
        }
    }

    func runArchive() {
        guard let manager else { return }
        let selected = selectedRows.map(\.path)
        let path = archivePath.trimmingCharacters(in: .whitespacesAndNewlines)
        let paths = archivePaths.split(whereSeparator: { $0.isNewline }).map(String.init)
        let request: [String: Any] = [
            "action": archiveAction,
            "path": path.isEmpty ? (selected.first ?? "") : path,
            "destination": archiveDestination,
            "paths": paths.isEmpty ? selected : paths,
        ]
        fileQueue.async { [weak self, manager] in
            let (data, error) = Self.callComponent(manager, id: "archives", request: request)
            let root = data.map(Self.json) ?? [:]
            let output = Self.text(root["text"])
            let openedPath = Self.text(root["path"])
            DispatchQueue.main.async {
                guard let self else { return }
                if let error {
                    self.operationError = error
                } else {
                    self.archiveOutput = output
                    if self.archiveAction == "open", !openedPath.isEmpty { self.navigate(openedPath) }
                }
            }
        }
    }

    func openProject(_ project: ProjectRecord) {
        ProjectWindowStore.shared.open(project)
    }

    func saveColumnLayout() {
        let defaults = UserDefaults.standard
        defaults.set(showKindColumn, forKey: "megaman.column.kind")
        defaults.set(showModifiedColumn, forKey: "megaman.column.modified")
        defaults.set(showSizeColumn, forKey: "megaman.column.size")
        defaults.set(nameColumnWidth, forKey: "megaman.column.nameWidth")
        defaults.set(kindColumnWidth, forKey: "megaman.column.kindWidth")
        defaults.set(modifiedColumnWidth, forKey: "megaman.column.modifiedWidth")
        defaults.set(sizeColumnWidth, forKey: "megaman.column.sizeWidth")
    }

    private nonisolated static func error(_ manager: NativeManager, status: Int32, fallback: String) -> String {
        let box = TextBox()
        let read = qfind_manager_error(manager.pointer, collectText, Unmanaged.passUnretained(box).toOpaque())
        return read == 0 && !box.value.isEmpty ? box.value : fallback + " (error " + String(status) + ")."
    }

    func setGlobalSearch(_ enabled: Bool) {
        guard let manager else { return }
        ioQueue.async { [weak self, manager] in
            guard qfind_manager_search_scope(manager.pointer, enabled ? 1 : 0) == 0 else { return }
            DispatchQueue.main.async { [weak self] in self?.refreshNow() }
        }
    }

    func applySort() {
        if sortKey == .newest, ascending { sortKey = .oldest; return }
        if sortKey == .oldest, !ascending { sortKey = .newest; return }
        let sort: UInt32
        switch sortKey {
        case .name: sort = ascending ? 1 : 2
        case .newest: sort = 3
        case .oldest: sort = 4
        case .size: sort = ascending ? 6 : 5
        }
        guard let manager else { return }
        ioQueue.async { [weak self, manager] in
            guard qfind_manager_sort(manager.pointer, sort) == 0 else { return }
            DispatchQueue.main.async { [weak self] in self?.refreshNow() }
        }
    }

    func navigate(_ path: String) {
        guard let manager else { return }
        generation += 1
        let current = generation
        ioQueue.async { [weak self, manager] in
            let status = path.withCString { qfind_manager_navigate(manager.pointer, $0) }
            let error = status == 0 ? nil : Self.error(manager, status: status, fallback: "Megaman could not open this folder.")
            DispatchQueue.main.async {
                guard let self, current == self.generation else { return }
                if let error { self.operationError = error; return }
                self.globalSearch = false
                self.chartGeneration += 1
                self.storageGeneration += 1
                self.chart = []
                self.storageEntries = []
                self.gitFooterStatus = ""
                self.directory = path
                self.startDirectoryWatcher()
                self.query = ""
                self.selectedPath = nil
                self.selectedPaths.removeAll()
                self.pushRecent(path)
                self.performFetch()
                self.refreshFooterGitStatus(force: true)
            }
        }
    }

    func goParent() {
        let parent = (directory as NSString).deletingLastPathComponent
        guard parent != directory, !parent.isEmpty else { return }
        navigate(parent)
    }

    func back() { move(backward: true) }
    func forward() { move(backward: false) }

    private func move(backward: Bool) {
        guard let manager else { return }
        generation += 1
        let current = generation
        ioQueue.async { [weak self, manager] in
            let status = backward ? qfind_manager_back(manager.pointer) : qfind_manager_forward(manager.pointer)
            if status != 0 {
                let message = Self.error(manager, status: status, fallback: "Megaman could not move through history.")
                DispatchQueue.main.async {
                    guard let self, current == self.generation else { return }
                    self.operationError = message
                }
                return
            }
            let box = TextBox()
            _ = qfind_manager_directory(manager.pointer, collectText, Unmanaged.passUnretained(box).toOpaque())
            DispatchQueue.main.async {
                guard let self, current == self.generation else { return }
                self.globalSearch = false
                self.chartGeneration += 1
                self.storageGeneration += 1
                self.chart = []
                self.storageEntries = []
                self.gitFooterStatus = ""
                self.directory = box.value
                self.startDirectoryWatcher()
                self.query = ""
                self.selectedPath = nil
                self.selectedPaths.removeAll()
                self.pushRecent(box.value)
                self.performFetch()
                self.refreshFooterGitStatus(force: true)
            }
        }
    }

    func activate(_ row: FileRow) {
        if row.isDirectory { navigate(row.path) }
        else { NSWorkspace.shared.open(row.url) }
    }

    func activateSelection() {
        guard let selectedRow else { return }
        activate(selectedRow)
    }

    func revealSelection() {
        guard let selectedRow else { return }
        NSWorkspace.shared.activateFileViewerSelecting([selectedRow.url])
    }

    func trashSelection() {
        let selected = selectedRows
        guard !selected.isEmpty else { return }
        selectedPaths.removeAll()
        selectedPath = nil
        self.fileQueue.async { [weak self] in
            var message: String?
            var failedPath: String?
            for row in selected {
                do {
                    try FileManager.default.trashItem(at: row.url, resultingItemURL: nil)
                } catch {
                    failedPath = row.path
                    message = "Could not move " + row.name + " to the Trash: " + error.localizedDescription
                    break
                }
            }
            DispatchQueue.main.async {
                guard let self else { return }
                if let message {
                    self.selectedPath = failedPath
                    if let failedPath { self.selectedPaths = [failedPath] }
                    self.operationError = message
                } else {
                    self.refreshNow()
                }
            }
        }
    }

    func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "Open"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        navigate(url.path)
    }

    func newFolder() {
        let alert = NSAlert()
        alert.messageText = "New Folder"
        alert.informativeText = "Create a folder in " + URL(fileURLWithPath: directory).lastPathComponent + "."
        let field = NSTextField(string: "Untitled Folder")
        field.frame.size.width = 260
        alert.accessoryView = field
        alert.addButton(withTitle: "Create")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, !name.contains("/") else {
            operationError = "Folder names cannot be empty or contain '/'."
            return
        }
        let destination = URL(fileURLWithPath: directory).appendingPathComponent(name)
        fileQueue.async { [weak self] in
            var message: String?
            do { try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: false) }
            catch { message = "Could not create " + name + ": " + error.localizedDescription }
            DispatchQueue.main.async {
                guard let self else { return }
                if let message { self.operationError = message } else { self.refreshNow() }
            }
        }
    }

    func rename(_ row: FileRow) {
        let alert = NSAlert()
        alert.messageText = "Rename " + row.name
        let field = NSTextField(string: row.name)
        field.frame.size.width = 260
        alert.accessoryView = field
        alert.addButton(withTitle: "Rename")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, !name.contains("/"), name != row.name else { return }
        let destination = row.url.deletingLastPathComponent().appendingPathComponent(name)
        fileQueue.async { [weak self] in
            var message: String?
            do { try FileManager.default.moveItem(at: row.url, to: destination) }
            catch { message = "Could not rename " + row.name + ": " + error.localizedDescription }
            DispatchQueue.main.async {
                guard let self else { return }
                if let message { self.operationError = message }
                else { self.selectedPath = destination.path; self.selectedPaths = [destination.path]; self.refreshNow() }
            }
        }
    }

    func duplicate(_ row: FileRow) {
        let base = row.url.deletingPathExtension().lastPathComponent
        let ext = row.url.pathExtension
        let suffix = ext.isEmpty ? "" : "." + ext
        let folder = row.url.deletingLastPathComponent()
        var index = 0
        var destination: URL
        repeat {
            index += 1
            let copyName = index == 1 ? base + " copy" + suffix : base + " copy " + String(index) + suffix
            destination = folder.appendingPathComponent(copyName)
        } while FileManager.default.fileExists(atPath: destination.path)
        self.fileQueue.async { [weak self] in
            var message: String?
            do {
                try FileManager.default.copyItem(at: row.url, to: destination)
            } catch {
                message = "Could not duplicate " + row.name + ": " + error.localizedDescription
            }
            DispatchQueue.main.async {
                guard let self else { return }
                if let message { self.operationError = message }
                else { self.refreshNow() }
            }
        }
    }

    func acceptDrop(_ providers: [NSItemProvider], to destination: URL) -> Bool {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: destination.path, isDirectory: &isDirectory), isDirectory.boolValue else { return false }
        for provider in providers {
            provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { [weak self] item, error in
                guard error == nil else { return }
                let source: URL?
                if let data = item as? Data { source = URL(dataRepresentation: data, relativeTo: nil) }
                else if let url = item as? URL { source = url }
                else if let url = item as? NSURL { source = url as URL }
                else { source = nil }
                guard let self, let source, source != destination else { return }
                let target = destination.appendingPathComponent(source.lastPathComponent)
                let sourcePath = source.resolvingSymlinksInPath().standardizedFileURL.path
                let targetPath = target.resolvingSymlinksInPath().standardizedFileURL.path
                guard targetPath != sourcePath, !targetPath.hasPrefix(sourcePath + "/") else {
                    DispatchQueue.main.async { self.operationError = "A folder cannot be copied into itself." }
                    return
                }
                self.fileQueue.async { [weak self] in
                    var message: String?
                    do {
                        try FileManager.default.copyItem(at: source, to: target)
                    } catch {
                        message = "Could not copy " + source.lastPathComponent + ": " + error.localizedDescription
                    }
                    DispatchQueue.main.async {
                        guard let self else { return }
                        if let message {
                            self.operationError = message
                        } else {
                            self.refreshNow()
                        }
                    }
                }
            }
        }
        return true
    }

    func toggleFavorite(_ path: String) {
        if favorites.contains(path) { favorites.removeAll(where: { $0 == path }) }
        else { favorites.append(path) }
        UserDefaults.standard.set(favorites, forKey: "qfind.favorites")
    }

    private func pushRecent(_ path: String) {
        recents.removeAll(where: { $0 == path })
        recents.insert(path, at: 0)
        recents = Array(recents.prefix(10))
        UserDefaults.standard.set(recents, forKey: "qfind.recents")
    }
}

@MainActor
private final class ProjectWindowStore {
    static let shared = ProjectWindowStore()
    private var windows: [NSWindow] = []

    func open(_ project: ProjectRecord) {
        let browser = Browser(directory: project.path)
        let window = NSWindow(contentViewController: NSHostingController(rootView: ContentView(browser: browser)))
        window.title = "Megaman — " + project.name
        window.styleMask = [.titled, .closable, .resizable, .miniaturizable]
        window.setContentSize(NSSize(width: 1180, height: 760))
        window.isReleasedWhenClosed = false
        windows.append(window)
        NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) { [weak self, weak window] _ in
            guard let window else { return }
            self?.windows.removeAll { $0 === window }
        }
        window.makeKeyAndOrderFront(nil)
    }
}

private struct ProjectGroup: Identifiable {
    let repository: String
    let projects: [ProjectRecord]
    var id: String { repository }
}

private struct ProjectsWorkspace: View {
    @ObservedObject var browser: Browser

    private var groups: [ProjectGroup] {
        Dictionary(grouping: browser.projects) { project in
            project.repository.isEmpty ? "Local projects" : project.repository
        }.map { ProjectGroup(repository: $0.key, projects: $0.value.sorted { $0.path < $1.path }) }
            .sorted { $0.repository.localizedStandardCompare($1.repository) == .orderedAscending }
    }

    var body: some View {
        HSplitView {
            List {
                ForEach(groups) { group in
                    Section(group.repository) {
                        ForEach(group.projects) { project in
                            HStack(spacing: 8) {
                                Image(systemName: project.repository.isEmpty ? "folder" : "arrow.triangle.branch")
                                    .foregroundStyle(.tint)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(project.name).lineLimit(1)
                                    Text(project.branch.isEmpty ? project.kind : project.branch)
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                if project.bytes > 0 {
                                    Text(ByteCountFormatter.string(fromByteCount: Int64(project.bytes), countStyle: .file))
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                            }
                            .contentShape(Rectangle())
                            .background(browser.selectedProjectPath == project.path ? Color.accentColor.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 8))
                            .onTapGesture { browser.selectProject(project) }
                            .onTapGesture(count: 2) { browser.openProject(project) }
                            .contextMenu {
                                Button("Open Project Window") { browser.openProject(project) }
                                Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: project.path)]) }
                            }
                        }
                    }
                }
            }
            .listStyle(.sidebar)
            .frame(minWidth: 320, idealWidth: 380)
            if let project = browser.selectedProject {
                ProjectDetail(browser: browser, project: project)
                    .frame(minWidth: 520)
            } else {
                ContentUnavailableView("No project selected", systemImage: "hammer",
                                       description: Text("Select a GitHub project or worktree to inspect it."))
                    .frame(minWidth: 520)
            }
        }
        .onAppear { browser.refreshProjects() }
    }
}

private struct ProjectDetail: View {
    @ObservedObject var browser: Browser
    let project: ProjectRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text(project.name).font(.title2.weight(.semibold))
                    Text(project.repository.isEmpty ? project.path : project.repository + " · " + project.branch)
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Menu {
                    ForEach(browser.projectTasks) { task in
                        Button(task.title) { browser.runTask(task) }
                    }
                    if browser.projectTasks.isEmpty { Text("No build commands found") }
                } label: {
                    Label("Build", systemImage: "hammer")
                }
                Button { browser.refreshProjectDiff() } label: { Image(systemName: "arrow.clockwise") }
                    .help("Refresh Git diff")
            }
            HStack(spacing: 8) {
                Image(systemName: "circle.lefthalf.filled").foregroundStyle(.tint)
                Text(browser.projectGitStatus.isEmpty ? "Working tree status unavailable" : browser.projectGitStatus)
                    .font(.system(.caption, design: .monospaced)).lineLimit(2).textSelection(.enabled)
                Spacer()
                if !browser.projectGitFiles.isEmpty {
                    Menu("Changed files") {
                        ForEach(browser.projectGitFiles, id: \.self) { file in
                            Button(file) {
                                browser.projectGitFile = file
                                browser.refreshProjectDiff()
                            }
                        }
                    }
                }
            }
            HStack {
                TextField("Relative file for stage / unstage (optional)", text: $browser.projectGitFile)
                Button("Stage") { browser.runGit("stage") }.disabled(browser.projectGitFile.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                Button("Unstage") { browser.runGit("unstage") }.disabled(browser.projectGitFile.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                Toggle("Split", isOn: $browser.projectDiffMode).toggleStyle(.switch)
            }
            if !project.artifacts.isEmpty {
                GroupBox("Builds & caches") {
                    ForEach(Array(project.artifacts.enumerated()), id: \.offset) { _, artifact in
                        HStack {
                            Image(systemName: "shippingbox")
                            Text(artifact.path).lineLimit(1)
                            Spacer()
                            Text(artifact.bytes > 0 ? ByteCountFormatter.string(fromByteCount: Int64(artifact.bytes), countStyle: .file) : "—")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            if !browser.taskOutput.isEmpty {
                Text(browser.taskOutput).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    .frame(maxHeight: 120, alignment: .topLeading).padding(8).qfindGlass()
            }
            DiffView(text: browser.projectDiff, split: browser.projectDiffMode)
                .frame(maxHeight: .infinity)
                .qfindGlass()
        }
        .padding(14)
    }
}

private struct DiffLine: Identifiable {
    let id: Int
    let oldNumber: Int?
    let newNumber: Int?
    let left: String
    let right: String
    let marker: String
}

private struct DiffHunk: Identifiable {
    let id: Int
    let lines: [DiffLine]
    var header: DiffLine? { lines.first(where: { $0.marker == "hunk" }) }
}

private struct DiffView: View {
    let text: String
    let split: Bool
    @State private var collapsedHunks: Set<Int> = []
    @State private var selectedHunk = 0

    private var lines: [DiffLine] {
        var old = 0
        var new = 0
        return text.split(separator: "\n", omittingEmptySubsequences: false).enumerated().map { index, raw in
            let value = String(raw)
            if value.hasPrefix("@@") {
                let numbers = value.split(separator: " ").filter { $0.hasPrefix("-") || $0.hasPrefix("+") }
                old = numbers.first(where: { $0.hasPrefix("-") }).flatMap { Int($0.dropFirst().split(separator: ",").first ?? "") } ?? old
                new = numbers.first(where: { $0.hasPrefix("+") }).flatMap { Int($0.dropFirst().split(separator: ",").first ?? "") } ?? new
                return DiffLine(id: index, oldNumber: nil, newNumber: nil, left: value, right: value, marker: "hunk")
            }
            if value.hasPrefix("-") && !value.hasPrefix("---") {
                defer { old += 1 }
                return DiffLine(id: index, oldNumber: old, newNumber: nil, left: value, right: "", marker: "removed")
            }
            if value.hasPrefix("+") && !value.hasPrefix("+++") {
                defer { new += 1 }
                return DiffLine(id: index, oldNumber: nil, newNumber: new, left: "", right: value, marker: "added")
            }
            let oldNumber = value.hasPrefix(" ") ? old : nil
            let newNumber = value.hasPrefix(" ") ? new : nil
            if value.hasPrefix(" ") { old += 1; new += 1 }
            return DiffLine(id: index, oldNumber: oldNumber, newNumber: newNumber, left: value, right: value, marker: "context")
        }
    }

    private var hunks: [DiffHunk] {
        var groups: [[DiffLine]] = []
        var current: [DiffLine] = []
        for line in lines {
            if line.marker == "hunk", current.contains(where: { $0.marker == "hunk" }) {
                groups.append(current)
                current = []
            }
            current.append(line)
        }
        if !current.isEmpty { groups.append(current) }
        return groups.enumerated().map { DiffHunk(id: $0.offset, lines: $0.element) }
    }

    var body: some View {
        VStack(spacing: 0) {
            if hunks.count > 1 {
                HStack(spacing: 8) {
                    Button { selectedHunk = max(0, selectedHunk - 1) } label: { Image(systemName: "chevron.up") }
                        .disabled(selectedHunk == 0)
                    Text("Hunk \(selectedHunk + 1) of \(hunks.count)").font(.caption).foregroundStyle(.secondary)
                    Button { selectedHunk = min(hunks.count - 1, selectedHunk + 1) } label: { Image(systemName: "chevron.down") }
                        .disabled(selectedHunk == hunks.count - 1)
                    Spacer()
                }
                .buttonStyle(.borderless)
                .padding(.horizontal, 8).padding(.vertical, 5)
                Divider()
            }
            ScrollViewReader { proxy in
                ScrollView([.vertical, .horizontal]) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(hunks) { hunk in
                            VStack(alignment: .leading, spacing: 0) {
                                Button {
                                    if collapsedHunks.contains(hunk.id) { collapsedHunks.remove(hunk.id) }
                                    else { collapsedHunks.insert(hunk.id) }
                                } label: {
                                    HStack(spacing: 5) {
                                        Image(systemName: collapsedHunks.contains(hunk.id) ? "chevron.right" : "chevron.down")
                                        Text(hunk.header?.left ?? "Diff")
                                        Spacer()
                                    }
                                    .font(.system(.body, design: .monospaced))
                                    .foregroundStyle(.primary)
                                    .padding(.horizontal, 5).padding(.vertical, 3)
                                    .background(Color.accentColor.opacity(0.12))
                                }
                                .buttonStyle(.plain)
                                if !collapsedHunks.contains(hunk.id) {
                                    ForEach(hunk.lines.filter { $0.marker != "hunk" }) { line in
                                        lineView(line)
                                    }
                                }
                            }
                            .id(hunk.id)
                        }
                    }
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
                    .padding(8)
                }
                .onChange(of: selectedHunk) { index in
                    withAnimation { proxy.scrollTo(index, anchor: .top) }
                }
            }
        }
        .overlay {
            if text.isEmpty || text == "No changes for this selection." {
                ContentUnavailableView("No changes", systemImage: "checkmark.circle", description: Text("The working tree is clean."))
            }
        }
        .onChange(of: text) { _ in collapsedHunks.removeAll(); selectedHunk = 0 }
    }

    @ViewBuilder
    private func lineView(_ line: DiffLine) -> some View {
        if split {
            HStack(spacing: 0) {
                DiffCell(number: line.oldNumber, value: line.left, marker: line.marker).frame(minWidth: 330, alignment: .leading)
                Divider()
                DiffCell(number: line.newNumber, value: line.right, marker: line.marker).frame(minWidth: 330, alignment: .leading)
            }
        } else {
            DiffCell(number: line.newNumber ?? line.oldNumber,
                     value: line.marker == "added" ? line.right : line.left,
                     marker: line.marker)
        }
    }
}

private struct DiffCell: View {
    let number: Int?
    let value: String
    let marker: String

    var body: some View {
        HStack(spacing: 8) {
            Text(number.map { String($0) } ?? "")
                .foregroundStyle(.secondary).frame(width: 42, alignment: .trailing)
            Text(value).textSelection(.enabled)
        }
        .padding(.horizontal, 5).padding(.vertical, 2)
        .background(marker == "removed" ? Color.red.opacity(0.12) : marker == "added" ? Color.green.opacity(0.12) : marker == "hunk" ? Color.accentColor.opacity(0.12) : .clear)
    }
}

private struct StoragePanel: View {
    @ObservedObject var browser: Browser

    private func bytes(_ value: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(value), countStyle: .file)
    }

    var body: some View {
        VStack(spacing: 0) {
            PieChart(rows: browser.chart, freeBytes: browser.storageFree, totalBytes: browser.storageTotal,
                     hoveredPath: $browser.hoveredPath).padding()
            if !browser.chart.isEmpty {
                StorageTreemap(rows: browser.chart, hoveredPath: $browser.hoveredPath)
                    .padding(.horizontal)
            }
            if browser.storageTotal > 0 {
                Text(bytes(browser.storageFree) + " free of " + bytes(browser.storageTotal))
                    .font(.caption).foregroundStyle(.secondary).padding(.bottom, 8)
            }
            List(browser.storageEntries) { entry in
                HStack(spacing: 8) {
                    Image(systemName: entry.isDirectory ? "folder" : "doc")
                        .foregroundStyle(.tint)
                    Text(entry.name.isEmpty ? entry.path : entry.name).lineLimit(1)
                    Spacer()
                    Text(bytes(entry.bytes)).font(.caption).foregroundStyle(.secondary)
                }
                .contentShape(Rectangle())
                .onTapGesture { if entry.isDirectory { browser.navigate(entry.path) } }
            }
        }
        .onAppear { browser.refreshStorageComponent() }
    }
}

private struct StorageTreemap: View {
    let rows: [FileRow]
    @Binding var hoveredPath: String?

    private var total: Double { Double(max(1, rows.reduce(0) { $0 &+ $1.bytes })) }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Space by folder").font(.caption).foregroundStyle(.secondary)
            GeometryReader { proxy in
                Canvas { context, size in
                    var x = 0.0
                    for (index, row) in rows.enumerated() where row.bytes > 0 {
                        let width = max(2, size.width * Double(row.bytes) / total)
                        let rect = CGRect(x: x, y: 0, width: min(width, size.width - x), height: size.height)
                        guard rect.width > 0 else { continue }
                        let color = Color(hue: Double(index) / Double(max(rows.count, 1)), saturation: 0.48, brightness: 0.88)
                        let active = hoveredPath == row.path
                        context.fill(Path(rect), with: .color(color.opacity(hoveredPath == nil || active ? 1 : 0.30)))
                        if rect.width > 52 {
                            context.draw(Text(row.name).font(.caption2).foregroundColor(.black),
                                         at: CGPoint(x: rect.midX, y: rect.midY))
                        }
                        x += width + 1
                        if x >= size.width { break }
                    }
                }
                .contentShape(Rectangle())
                .onContinuousHover { phase in
                    switch phase {
                    case .active(let location): hoveredPath = row(at: location.x, width: proxy.size.width)
                    case .ended: hoveredPath = nil
                    }
                }
            }
            .frame(height: 72)
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
    }

    private func row(at x: CGFloat, width: CGFloat) -> String? {
        var position = 0.0
        for row in rows where row.bytes > 0 {
            let segment = max(2, width * Double(row.bytes) / total)
            if x >= position && x <= position + segment { return row.path }
            position += segment + 1
            if position >= Double(width) { break }
        }
        return nil
    }
}

private struct BatchPanel: View {
    @ObservedObject var browser: Browser

    var body: some View {
        Form {
            Section("Select items") {
                TextField("Name contains", text: $browser.selectionName)
                TextField("Extension (for example png)", text: $browser.selectionExtension)
                Picker("Type", selection: $browser.selectionKind) {
                    Text("Any").tag("all")
                    Text("Files").tag("files")
                    Text("Folders").tag("folders")
                }
                HStack {
                    Button("Select matching") { browser.selectByCriteria() }
                    Button("Clear") { browser.clearSelection() }
                    Spacer()
                    Text("\(browser.selectedPaths.count) selected").foregroundStyle(.secondary)
                }
            }
            Section("Batch action") {
                Picker("Operation", selection: $browser.batchAction) {
                    Text("Rename").tag("rename")
                    Text("Copy").tag("copy")
                    Text("Move").tag("move")
                }
                TextField("Destination folder", text: $browser.batchDestination)
                TextField("Find", text: $browser.batchFind)
                TextField("Replace", text: $browser.batchReplace)
                TextField("Prefix", text: $browser.batchPrefix)
                TextField("Suffix", text: $browser.batchSuffix)
                Text(browser.batchPaths.isEmpty ? "Uses the current selection when empty" : "One path per line")
                    .font(.caption).foregroundStyle(.secondary)
                TextEditor(text: $browser.batchPaths).frame(minHeight: 80)
                HStack {
                    Button("Preview") { browser.runBatch(preview: true) }
                    Button("Apply") { browser.runBatch(preview: false) }
                }
            }
            if !browser.batchPreview.isEmpty {
                Section("Preview") {
                    ForEach(browser.batchPreview) { item in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.from).lineLimit(1)
                            Text("→ " + item.to).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                }
            }
            if !browser.batchOutput.isEmpty {
                Text(browser.batchOutput).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
        }
        .formStyle(.grouped)
    }
}

private struct ArchivePanel: View {
    @ObservedObject var browser: Browser

    private var actionTitle: String {
        switch browser.archiveAction {
        case "compress": "Create archive"
        case "extract": "Extract archive"
        case "save": "Save archive"
        default: "Open archive"
        }
    }

    var body: some View {
        Form {
            Section("Archive") {
                Picker("Operation", selection: $browser.archiveAction) {
                    Text("Open").tag("open")
                    Text("Compress").tag("compress")
                    Text("Extract").tag("extract")
                    Text("Save").tag("save")
                }
                if browser.archiveAction != "compress" {
                    TextField("Archive path", text: $browser.archivePath)
                }
                if browser.archiveAction == "compress" {
                    Text("Uses the current selection when empty").font(.caption).foregroundStyle(.secondary)
                    TextEditor(text: $browser.archivePaths).frame(minHeight: 90)
                    TextField("New archive path", text: $browser.archiveDestination)
                } else if browser.archiveAction == "extract" {
                    TextField("Destination folder", text: $browser.archiveDestination)
                }
                Button(actionTitle) { browser.runArchive() }
            }
            if !browser.archiveOutput.isEmpty {
                Text(browser.archiveOutput).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Small views

private struct FileIcon: View {
    let path: String
    var body: some View {
        Image(nsImage: NSWorkspace.shared.icon(forFile: path))
            .resizable().scaledToFit()
    }
}

private struct QuickPreview: NSViewRepresentable {
    let path: String?
    func makeNSView(context: Context) -> QLPreviewView { QLPreviewView(frame: .zero, style: .normal) }
    func updateNSView(_ view: QLPreviewView, context: Context) {
        view.previewItem = path.map { URL(fileURLWithPath: $0) as NSURL }
    }
}

private struct PieChart: View {
    let rows: [FileRow]
    let freeBytes: UInt64
    let totalBytes: UInt64
    @Binding var hoveredPath: String?
    var total: Double {
        let indexed = rows.reduce(0) { $0 + $1.bytes }
        return Double(max(1, totalBytes > 0 ? totalBytes : indexed))
    }
    var otherUsedBytes: UInt64 {
        guard totalBytes > freeBytes else { return 0 }
        let used = totalBytes - freeBytes
        return used - min(used, rows.reduce(0) { $0 &+ $1.bytes })
    }

    var body: some View {
        GeometryReader { proxy in
            Canvas { context, size in
                let rect = CGRect(origin: .zero, size: size).insetBy(dx: 22, dy: 22)
                var start = -Double.pi / 2
                for (index, row) in rows.enumerated() where row.bytes > 0 {
                    let sweep = Double(row.bytes) / total * Double.pi * 2
                    var path = Path()
                    path.move(to: CGPoint(x: rect.midX, y: rect.midY))
                    path.addArc(center: CGPoint(x: rect.midX, y: rect.midY), radius: min(rect.width, rect.height) / 2,
                                startAngle: .radians(start), endAngle: .radians(start + sweep), clockwise: false)
                    path.closeSubpath()
                    let active = hoveredPath == row.path
                    let opacity = hoveredPath == nil || active ? 1.0 : 0.30
                    let color = Color(hue: Double(index) / Double(max(rows.count, 1)), saturation: 0.48, brightness: 0.88)
                    context.fill(path, with: .color(color.opacity(opacity)))
                    if active {
                        context.stroke(path, with: .color(.white.opacity(0.9)), lineWidth: 2)
                    }
                    if sweep > 0.22 {
                        let angle = start + sweep / 2
                        let radius = min(rect.width, rect.height) * 0.31
                        context.draw(Text(ByteCountFormatter.string(fromByteCount: Int64(row.bytes), countStyle: .file)).font(.caption2).foregroundColor(.black),
                                     at: CGPoint(x: rect.midX + cos(angle) * radius, y: rect.midY + sin(angle) * radius))
                    }
                    start += sweep
                }
                let otherSweep = min(1, Double(otherUsedBytes) / total) * Double.pi * 2
                if otherSweep > 0 {
                    var path = Path()
                    path.move(to: CGPoint(x: rect.midX, y: rect.midY))
                    path.addArc(center: CGPoint(x: rect.midX, y: rect.midY), radius: min(rect.width, rect.height) / 2,
                                startAngle: .radians(start), endAngle: .radians(start + otherSweep), clockwise: false)
                    path.closeSubpath()
                    context.fill(path, with: .color(.secondary.opacity(0.38)))
                    if otherSweep > 0.22 {
                        let angle = start + otherSweep / 2
                        let radius = min(rect.width, rect.height) * 0.31
                        context.draw(Text("Other used").font(.caption2).foregroundColor(.primary),
                                     at: CGPoint(x: rect.midX + cos(angle) * radius, y: rect.midY + sin(angle) * radius))
                    }
                    start += otherSweep
                }
                let freeSweep = min(1, Double(freeBytes) / total) * Double.pi * 2
                if freeSweep > 0 {
                    var path = Path()
                    path.move(to: CGPoint(x: rect.midX, y: rect.midY))
                    path.addArc(center: CGPoint(x: rect.midX, y: rect.midY), radius: min(rect.width, rect.height) / 2,
                                startAngle: .radians(start), endAngle: .radians(start + freeSweep), clockwise: false)
                    path.closeSubpath()
                    context.fill(path, with: .color(.secondary.opacity(0.22)))
                    if freeSweep > 0.22 {
                        let angle = start + freeSweep / 2
                        let radius = min(rect.width, rect.height) * 0.31
                        context.draw(Text("Free").font(.caption2).foregroundColor(.primary),
                                     at: CGPoint(x: rect.midX + cos(angle) * radius, y: rect.midY + sin(angle) * radius))
                    }
                }
            }
            .contentShape(Rectangle())
            .onContinuousHover { phase in
                switch phase {
                case .active(let location): hoveredPath = row(at: location, size: proxy.size)
                case .ended: hoveredPath = nil
                }
            }
        }
        .aspectRatio(1, contentMode: .fit)
    }

    private func row(at location: CGPoint, size: CGSize) -> String? {
        let rect = CGRect(origin: .zero, size: size).insetBy(dx: 22, dy: 22)
        let dx = Double(location.x - rect.midX)
        let dy = Double(location.y - rect.midY)
        guard hypot(dx, dy) <= Double(min(rect.width, rect.height) / 2) else { return nil }
        var angle = atan2(dy, dx) + Double.pi / 2
        if angle < 0 { angle += Double.pi * 2 }
        var start = 0.0
        for row in rows where row.bytes > 0 {
            let sweep = Double(row.bytes) / total * Double.pi * 2
            if angle >= start && angle < start + sweep { return row.path }
            start += sweep
        }
        return nil
    }
}

// MARK: - Result surfaces

private struct RowContextMenu: ViewModifier {
    @ObservedObject var browser: Browser
    let row: FileRow
    func body(content: Content) -> some View {
        content.contextMenu {
            Button("Open") { browser.activate(row) }
            Button(row.isDirectory ? "Open Enclosing Folder" : "Open Enclosing Folder") {
                NSWorkspace.shared.open(URL(fileURLWithPath: row.parent))
            }
            Button("Reveal in Finder") {
                NSWorkspace.shared.activateFileViewerSelecting([row.url])
            }
            Divider()
            Button("Copy Path") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(row.path, forType: .string)
            }
            Button("Rename…") { browser.rename(row) }
            Button("Duplicate") { browser.duplicate(row) }
            Button(browser.favorites.contains(row.path) ? "Remove from Favorites" : "Add to Favorites") {
                browser.toggleFavorite(row.path)
            }
            Divider()
            Button("Move to Trash", role: .destructive) {
                browser.selectedPaths = [row.path]
                browser.selectedPath = row.path
                browser.trashSelection()
            }
        }
    }
}

private struct RowInteraction: ViewModifier {
    @ObservedObject var browser: Browser
    let row: FileRow

    func body(content: Content) -> some View {
        let fill = browser.selectedPaths.contains(row.path)
            ? Color.accentColor.opacity(0.22)
            : browser.hoveredPath == row.path ? Color.accentColor.opacity(0.12) : Color.clear
        let highlighted = content
            .background(RoundedRectangle(cornerRadius: 8).fill(fill))
            .onHover { inside in
                if inside { browser.hoveredPath = row.path }
                else if browser.hoveredPath == row.path { browser.hoveredPath = nil }
            }
        if row.isDirectory {
            highlighted.onDrop(of: [UTType.fileURL], isTargeted: nil) { providers in
                browser.acceptDrop(providers, to: row.url)
            }
        } else {
            highlighted
        }
    }
}

private struct IconSurface: View {
    @ObservedObject var browser: Browser
    var body: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: browser.density), spacing: 8)], spacing: 8) {
                ForEach(browser.sortedRows) { row in
                    VStack(spacing: 6) {
                        FileIcon(path: row.path).frame(width: browser.density * 0.55, height: browser.density * 0.55)
                        Text(row.name).lineLimit(2).multilineTextAlignment(.center)
                        Text(row.sizeString).font(.caption).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity, minHeight: browser.density)
                    .padding(6)
                    .contentShape(Rectangle())
                    .onTapGesture { browser.select(row.path) }
                    .onTapGesture(count: 2) { browser.activate(row) }
                    .modifier(RowInteraction(browser: browser, row: row))
                    .modifier(RowContextMenu(browser: browser, row: row))
                    .onDrag { NSItemProvider(contentsOf: row.url) ?? NSItemProvider() }
                }
            }.padding(8)
        }
        .onDrop(of: [UTType.fileURL], isTargeted: nil) { providers in
            browser.acceptDrop(providers, to: URL(fileURLWithPath: browser.directory))
        }
    }
}

private struct ListSurface: View {
    @ObservedObject var browser: Browser
    var body: some View {
        ScrollView {
            LazyVStack(spacing: 1, pinnedViews: .sectionHeaders) {
                Section {
                    ForEach(browser.sortedRows) { row in
                        HStack {
                            FileIcon(path: row.path).frame(width: 28, height: 28)
                            Text(row.name).lineLimit(1).frame(minWidth: browser.nameColumnWidth, alignment: .leading)
                            if browser.showKindColumn {
                                Text(row.kind).font(.callout).foregroundStyle(.secondary)
                                    .frame(width: browser.kindColumnWidth, alignment: .leading)
                            }
                            if browser.showModifiedColumn {
                                Text(row.dateString).font(.callout).foregroundStyle(.secondary)
                                    .frame(width: browser.modifiedColumnWidth, alignment: .trailing)
                            }
                            if browser.showSizeColumn {
                                Text(row.sizeString).font(.callout).foregroundStyle(.secondary)
                                    .frame(width: browser.sizeColumnWidth, alignment: .trailing)
                            }
                            Spacer(minLength: 8)
                        }
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .contentShape(Rectangle())
                        .onTapGesture { browser.select(row.path) }
                        .onTapGesture(count: 2) { browser.activate(row) }
                        .modifier(RowInteraction(browser: browser, row: row))
                        .modifier(RowContextMenu(browser: browser, row: row))
                        .onDrag { NSItemProvider(contentsOf: row.url) ?? NSItemProvider() }
                    }
                } header: {
                    HStack {
                        sortHeader("Name", .name).frame(minWidth: browser.nameColumnWidth, alignment: .leading)
                        if browser.showKindColumn {
                            Text("Kind").frame(width: browser.kindColumnWidth, alignment: .leading)
                        }
                        if browser.showModifiedColumn {
                            sortHeader("Modified", browser.sortKey == .newest || browser.sortKey == .oldest ? browser.sortKey : .newest)
                                .frame(width: browser.modifiedColumnWidth, alignment: .trailing)
                        }
                        if browser.showSizeColumn {
                            sortHeader("Size", .size).frame(width: browser.sizeColumnWidth, alignment: .trailing)
                        }
                        Spacer(minLength: 8)
                    }
                    .font(.caption).foregroundStyle(.secondary)
                    .padding(.horizontal, 8).padding(.vertical, 4)
                    .background(.regularMaterial)
                }
            }.padding(.horizontal, 6)
        }
        .onDrop(of: [UTType.fileURL], isTargeted: nil) { providers in
            browser.acceptDrop(providers, to: URL(fileURLWithPath: browser.directory))
        }
    }

    private func sortHeader(_ title: String, _ key: SortKey) -> some View {
        Button {
            if key == .newest || key == .oldest {
                if browser.sortKey == .newest {
                    browser.sortKey = .oldest
                    browser.ascending = true
                } else if browser.sortKey == .oldest {
                    browser.sortKey = .newest
                    browser.ascending = false
                } else {
                    browser.sortKey = .newest
                    browser.ascending = false
                }
            } else if browser.sortKey == key { browser.ascending.toggle() }
            else {
                browser.sortKey = key
                browser.ascending = true
            }
        } label: {
            HStack(spacing: 2) {
                Text(title)
                if browser.sortKey == key {
                    Image(systemName: browser.ascending ? "chevron.up" : "chevron.down").font(.caption2)
                }
            }
        }.buttonStyle(.plain)
    }
}

private struct ColumnsSurface: View {
    @ObservedObject var browser: Browser
    var body: some View {
        Group {
            if browser.columns.isEmpty {
                ContentUnavailableView("No columns", systemImage: "rectangle.split.3x1",
                                       description: Text("Browse a folder to see its hierarchy."))
            } else {
                ScrollView(.horizontal) {
                    HStack(alignment: .top, spacing: 1) {
                        ForEach(browser.columns.indices, id: \.self) { index in
                            VStack(spacing: 1) {
                                ForEach(browser.columns[index]) { row in
                                    HStack {
                                        FileIcon(path: row.path).frame(width: 22, height: 22)
                                        Text(row.name).lineLimit(1)
                                        Spacer()
                                        if row.isDirectory { Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary) }
                                    }
                                    .padding(.horizontal, 8).padding(.vertical, 5)
                                    .contentShape(Rectangle())
                                    .onTapGesture { browser.select(row.path) }
                                    .onTapGesture(count: 2) { browser.activate(row) }
                                    .modifier(RowInteraction(browser: browser, row: row))
                                    .modifier(RowContextMenu(browser: browser, row: row))
                                    .onDrag { NSItemProvider(contentsOf: row.url) ?? NSItemProvider() }
                                }
                            }
                            .frame(width: 220)
                            .padding(4)
                            .qfindGlass()
                        }
                    }.padding(8)
                }
                .onDrop(of: [UTType.fileURL], isTargeted: nil) { providers in
                    browser.acceptDrop(providers, to: URL(fileURLWithPath: browser.directory))
                }
            }
        }
    }
}

private struct GallerySurface: View {
    @ObservedObject var browser: Browser
    var body: some View {
        VStack(spacing: 0) {
            Group {
                if let selected = browser.selectedRow {
                    QuickPreview(path: selected.path)
                } else if let first = browser.sortedRows.first {
                    QuickPreview(path: first.path)
                } else {
                    ContentUnavailableView("No selection", systemImage: "photo.stack",
                                           description: Text("Select a Hit to preview it."))
                }
            }.frame(maxHeight: .infinity)
            Divider()
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 8) {
                    ForEach(browser.sortedRows) { row in
                        VStack(spacing: 4) {
                            FileIcon(path: row.path).frame(width: 72, height: 72)
                            Text(row.name).font(.caption).lineLimit(1).frame(width: 88)
                        }
                        .padding(6)
                        .contentShape(Rectangle())
                        .onTapGesture { browser.select(row.path) }
                        .onTapGesture(count: 2) { browser.activate(row) }
                        .modifier(RowInteraction(browser: browser, row: row))
                        .modifier(RowContextMenu(browser: browser, row: row))
                        .onDrag { NSItemProvider(contentsOf: row.url) ?? NSItemProvider() }
                    }
                }.padding(8)
            }.frame(height: 128)
        }
        .onDrop(of: [UTType.fileURL], isTargeted: nil) { providers in
            browser.acceptDrop(providers, to: URL(fileURLWithPath: browser.directory))
        }
    }
}

// MARK: - Inspector / sidebar / chrome

private struct Inspector: View {
    @ObservedObject var browser: Browser
    var body: some View {
        VStack(spacing: 0) {
            if browser.showChart {
                StoragePanel(browser: browser)
            } else if let selected = browser.selectedRow {
                QuickPreview(path: selected.path).frame(minHeight: 220)
                Divider()
                Form {
                    Text(selected.name).font(.headline).lineLimit(2)
                    LabeledContent("Kind", value: selected.kind)
                    LabeledContent("Size", value: selected.sizeString)
                    LabeledContent("Modified", value: selected.dateString)
                    LabeledContent("Where", value: selected.parent)
                }.formStyle(.grouped).scrollContentBackground(.hidden)
                HStack {
                    Button("Open") { browser.activate(selected) }
                    Button("Reveal") { browser.revealSelection() }
                    Button("Trash", role: .destructive) { browser.trashSelection() }
                }.padding(.bottom, 8)
            } else {
                ContentUnavailableView("No selection", systemImage: "sidebar.right",
                                       description: Text("Select a Hit to see its Preview."))
            }
        }
    }
}

private struct Sidebar: View {
    @ObservedObject var browser: Browser
    private let home = FileManager.default.homeDirectoryForCurrentUser

    var body: some View {
        List {
            Section("Workspace") {
                Button { browser.workspace = .storage } label: {
                    Label(Workspace.storage.title, systemImage: Workspace.storage.symbol)
                }
                Button { browser.workspace = .projects; browser.refreshProjects(force: true) } label: {
                    Label(Workspace.projects.title, systemImage: Workspace.projects.symbol)
                }
            }
            if !browser.favorites.isEmpty {
                Section("Favorites") {
                    ForEach(browser.favorites, id: \.self) { path in
                        place(URL(fileURLWithPath: path).lastPathComponent, "star", path)
                            .contextMenu {
                                Button("Remove from Favorites") { browser.toggleFavorite(path) }
                            }
                    }
                }
            }
            Section("Places") {
                Button { browser.chooseFolder() } label: { Label("Choose Folder…", systemImage: "folder.badge.plus") }
                place("Home", "house", home.path)
                place("Desktop", "desktopcomputer", home.appendingPathComponent("Desktop").path)
                place("Documents", "doc", home.appendingPathComponent("Documents").path)
                place("Downloads", "arrow.down.circle", home.appendingPathComponent("Downloads").path)
                place("Pictures", "photo", home.appendingPathComponent("Pictures").path)
                place("Music", "music.note", home.appendingPathComponent("Music").path)
                place("Movies", "film", home.appendingPathComponent("Movies").path)
            }
            if !browser.recents.isEmpty {
                Section("Recents") {
                    ForEach(browser.recents, id: \.self) { path in
                        place(URL(fileURLWithPath: path).lastPathComponent, "clock", path)
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .navigationSplitViewColumnWidth(min: 160, ideal: 200)
    }

    private func place(_ title: String, _ icon: String, _ path: String) -> some View {
        Button { browser.navigate(path) } label: {
            HStack {
                Label(title, systemImage: icon)
                Spacer()
                if browser.directory == path {
                    Image(systemName: "checkmark").font(.caption2).foregroundStyle(.secondary)
                }
            }
        }
        .buttonStyle(.plain)
        .opacity(FileManager.default.fileExists(atPath: path) ? 1 : 0.45)
    }
}

private struct PathBar: View {
    @ObservedObject var browser: Browser
    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 2) {
                let parts = (browser.directory as NSString).pathComponents
                ForEach(parts.indices, id: \.self) { index in
                    let sub = NSString.path(withComponents: Array(parts.prefix(index + 1)))
                    if index > 0 {
                        Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                    }
                    Button(index == parts.count - 1 ? parts[index] : parts[index]) {
                        browser.navigate(sub)
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(index == parts.count - 1 ? Color.primary : Color.secondary)
                }
            }.padding(.horizontal, 10).padding(.vertical, 5)
        }
        .qfindGlass()
        .padding(.horizontal, 8)
    }
}

private struct StatusBar: View {
    @ObservedObject var browser: Browser
    var body: some View {
        HStack {
            Text("\(browser.sortedRows.count) items")
            let folders = browser.sortedRows.filter(\.isDirectory).count
            Text("\(folders) folders · \(browser.sortedRows.count - folders) files")
                .foregroundStyle(.secondary)
            if !browser.footerGitSummary.isEmpty {
                Text(browser.footerGitSummary)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            if let selected = browser.selectedRow {
                Text("\(selected.name) · \(selected.sizeString)").foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .font(.caption)
        .padding(.horizontal, 10).padding(.vertical, 4)
    }
}

// MARK: - Content

private struct ContentView: View {
    @StateObject private var browser: Browser

    init(browser: Browser) { _browser = StateObject(wrappedValue: browser) }

    var body: some View {
        NavigationSplitView {
            Sidebar(browser: browser)
        } detail: {
            Group {
                if browser.workspace == .projects {
                    ProjectsWorkspace(browser: browser)
                } else {
                    VStack(spacing: 0) {
                        if browser.showPathBar { PathBar(browser: browser).padding(.top, 6) }
                        HSplitView {
                            Group {
                                switch browser.viewMode {
                                case .icon: IconSurface(browser: browser)
                                case .list: ListSurface(browser: browser)
                                case .columns: ColumnsSurface(browser: browser)
                                case .gallery: GallerySurface(browser: browser)
                                }
                            }
                            .frame(minWidth: 420)
                            .overlay {
                                if browser.sortedRows.isEmpty {
                                    ContentUnavailableView.search(text: browser.query.isEmpty ? "Empty folder" : browser.query)
                                }
                            }
                            if browser.showBatch {
                                BatchPanel(browser: browser)
                                    .frame(minWidth: 320, idealWidth: 380)
                            } else if browser.showArchive {
                                ArchivePanel(browser: browser)
                                    .frame(minWidth: 320, idealWidth: 380)
                            } else if browser.showInspector || browser.showChart {
                                Inspector(browser: browser)
                                    .frame(minWidth: 280, idealWidth: 340)
                            }
                        }
                        if browser.showStatusBar {
                            Divider()
                            StatusBar(browser: browser)
                        }
                    }
                }
            }
            .navigationTitle(browser.workspace == .projects ? "Projects" : URL(fileURLWithPath: browser.directory).lastPathComponent)
            .searchable(text: $browser.query, placement: .toolbar, prompt: browser.globalSearch ? "Megaman search across indexed folders" : browser.recursive ? "Megaman search below this folder" : "Find in this folder")
            .onSubmit(of: .search) { browser.refreshNow() }
            .onChange(of: browser.query) { _ in browser.refresh() }
            .onChange(of: browser.recursive) { _ in browser.refreshNow() }
            .onChange(of: browser.globalSearch) { enabled in browser.setGlobalSearch(enabled) }
            .onChange(of: browser.sortKey) { _ in browser.applySort() }
            .onChange(of: browser.ascending) { _ in browser.applySort() }
            .onChange(of: browser.showKindColumn) { _ in browser.saveColumnLayout() }
            .onChange(of: browser.showModifiedColumn) { _ in browser.saveColumnLayout() }
            .onChange(of: browser.showSizeColumn) { _ in browser.saveColumnLayout() }
            .onChange(of: browser.nameColumnWidth) { _ in browser.saveColumnLayout() }
            .onChange(of: browser.kindColumnWidth) { _ in browser.saveColumnLayout() }
            .onChange(of: browser.modifiedColumnWidth) { _ in browser.saveColumnLayout() }
            .onChange(of: browser.sizeColumnWidth) { _ in browser.saveColumnLayout() }
            .onChange(of: browser.viewMode) { _ in browser.refreshNow() }
            .onChange(of: browser.workspace) { workspace in
                if workspace == .projects { browser.refreshProjects() }
            }
            .onChange(of: browser.showChart) { visible in
                if visible { browser.refreshChart(); browser.refreshStorageComponent() }
            }
            .alert("Megaman", isPresented: Binding(
                get: { browser.operationError != nil },
                set: { if !$0 { browser.operationError = nil } }
            )) {
                Button("OK") { browser.operationError = nil }
            } message: {
                Text(browser.operationError ?? "The operation failed.")
            }
            .toolbar {
                ToolbarItemGroup {
                    Button(action: browser.back) { Image(systemName: "chevron.left") }.help("Back")
                    Button(action: browser.forward) { Image(systemName: "chevron.right") }.help("Forward")
                    Button(action: browser.goParent) { Image(systemName: "chevron.up") }.help("Parent folder")
                    Picker("Mode", selection: $browser.recursive) {
                        Text("Classic").tag(false); Text("Qfind").tag(true)
                    }.pickerStyle(.segmented).help("Classic lists this folder; Megaman searches below it")
                    Picker("View", selection: $browser.viewMode) {
                        ForEach(ViewMode.allCases) { mode in
                            Image(systemName: mode.symbol).tag(mode).help(mode.title)
                        }
                    }.pickerStyle(.segmented)
                    Menu {
                        Toggle("Search all indexed folders", isOn: $browser.globalSearch)
                            .keyboardShortcut("g", modifiers: [.command])
                        Divider()
                        Picker("Sort", selection: $browser.sortKey) {
                            ForEach(SortKey.allCases) { key in Text(key.title).tag(key) }
                        }
                        Toggle("Ascending", isOn: $browser.ascending)
                        Toggle("Folders first", isOn: $browser.foldersFirst)
                        Divider()
                        Toggle("Kind column", isOn: $browser.showKindColumn)
                        Toggle("Modified column", isOn: $browser.showModifiedColumn)
                        Toggle("Size column", isOn: $browser.showSizeColumn)
                        Divider()
                        Text("Column widths")
                        Slider(value: $browser.nameColumnWidth, in: 140...520) { Text("Name") }
                        Slider(value: $browser.kindColumnWidth, in: 80...260) { Text("Kind") }
                        Slider(value: $browser.modifiedColumnWidth, in: 100...260) { Text("Modified") }
                        Slider(value: $browser.sizeColumnWidth, in: 72...220) { Text("Size") }
                    } label: { Image(systemName: "arrow.up.arrow.down") }.help("Sort")
                    if browser.viewMode == .icon {
                        Slider(value: $browser.density, in: 88...220).frame(width: 110).help("Icon size")
                    }
                    Button(action: browser.newFolder) { Image(systemName: "folder.badge.plus") }.help("New folder")
                    Button { browser.showBatch.toggle(); browser.showArchive = false } label: { Image(systemName: "rectangle.3.group") }.help("Batch actions")
                    Button { browser.showChart.toggle() }
                        label: { Image(systemName: browser.showChart ? "doc.richtext" : "chart.pie") }
                        .help("WeightMap")
                    Button { browser.showInspector.toggle() }
                        label: { Image(systemName: "sidebar.right") }
                        .help("Preview inspector")
                    ForEach(browser.components) { component in
                        if !component.commands.isEmpty {
                            Menu {
                                ForEach(component.commands) { command in
                                    Button(command.title) { browser.runComponent(component, command: command) }
                                }
                            } label: {
                                Image(systemName: nativeSymbol(component.icon))
                            }.help(component.title)
                        }
                    }
                    Button(action: browser.refreshNow) { Image(systemName: "arrow.clockwise") }.help("Refresh")
                }
            }
        }
    }
}

@main
struct MegamanMacApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView(browser: Browser())
        }
        .windowStyle(.titleBar)
        .windowToolbarStyle(.unifiedCompact)
        .defaultSize(width: 1180, height: 760)
    }
}
