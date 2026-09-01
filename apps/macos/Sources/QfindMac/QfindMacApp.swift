import AppKit
import CQfind
import QuickLookUI
import SwiftUI

private struct FileRow: Identifiable, Hashable {
    let id: UInt32
    let name: String
    let path: String
    let bytes: UInt64
    let entries: UInt64
    let isDirectory: Bool
}

private final class RowsBox {
    var rows: [FileRow] = []
}

private let collectRow: QfindRowCallback = { context, pointer in
    guard let context, let row = pointer?.pointee else { return }
    Unmanaged<RowsBox>.fromOpaque(context).takeUnretainedValue().rows.append(FileRow(
        id: row.id,
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

@MainActor
private final class Browser: ObservableObject {
    @Published var rows: [FileRow] = []
    @Published var chart: [FileRow] = []
    @Published var selected: FileRow?
    @Published var query = ""
    @Published var directory: String
    @Published var recursive = false
    @Published var grid = true
    @Published var showChart = false
    @Published var globalChart = false
    @Published var density = 128.0

    private let manager: OpaquePointer

    init?() {
        let initialDirectory = FileManager.default.homeDirectoryForCurrentUser.path
        directory = initialDirectory
        guard let handle = initialDirectory.withCString({ qfind_manager_open($0) }) else { return nil }
        manager = handle
        refresh()
    }

    deinit { qfind_manager_free(manager) }

    func refresh() {
        let box = RowsBox()
        let context = Unmanaged.passUnretained(box).toOpaque()
        query.withCString { _ = qfind_manager_rows(manager, $0, recursive ? 1 : 0, 5_000, collectRow, context) }
        rows = box.rows
        if selected.map({ !rows.contains($0) }) == true { selected = nil }
        refreshChart()
    }

    func refreshChart() {
        let box = RowsBox()
        let context = Unmanaged.passUnretained(box).toOpaque()
        _ = qfind_manager_chart(manager, globalChart ? 1 : 0, 24, collectRow, context)
        chart = box.rows
    }

    func navigate(_ path: String) {
        guard path.withCString({ qfind_manager_navigate(manager, $0) }) == 0 else { return }
        directory = path
        query = ""
        selected = nil
        refresh()
    }

    func back() { move(qfind_manager_back(manager)) }
    func forward() { move(qfind_manager_forward(manager)) }

    private func move(_ status: Int32) {
        guard status == 0 else { return }
        let box = TextBox()
        _ = qfind_manager_directory(manager, collectText, Unmanaged.passUnretained(box).toOpaque())
        directory = box.value
        selected = nil
        refresh()
    }

    func activate(_ row: FileRow) {
        if row.isDirectory { navigate(row.path) }
        else { NSWorkspace.shared.open(URL(fileURLWithPath: row.path)) }
    }
}

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
    var total: Double { Double(max(1, rows.reduce(0) { $0 + $1.bytes })) }

    var body: some View {
        Canvas { context, size in
            let rect = CGRect(origin: .zero, size: size).insetBy(dx: 22, dy: 22)
            var start = -Double.pi / 2
            for (index, row) in rows.enumerated() {
                let sweep = Double(row.bytes) / total * Double.pi * 2
                var path = Path()
                path.move(to: CGPoint(x: rect.midX, y: rect.midY))
                path.addArc(center: CGPoint(x: rect.midX, y: rect.midY), radius: min(rect.width, rect.height) / 2,
                            startAngle: .radians(start), endAngle: .radians(start + sweep), clockwise: false)
                path.closeSubpath()
                context.fill(path, with: .color(Color(hue: Double(index) / Double(max(rows.count, 1)), saturation: 0.48, brightness: 0.88)))
                if sweep > 0.22 {
                    let angle = start + sweep / 2
                    let radius = min(rect.width, rect.height) * 0.31
                    context.draw(Text(ByteCountFormatter.string(fromByteCount: Int64(row.bytes), countStyle: .file)).font(.caption2).foregroundColor(.black),
                                 at: CGPoint(x: rect.midX + cos(angle) * radius, y: rect.midY + sin(angle) * radius))
                }
                start += sweep
            }
        }
        .aspectRatio(1, contentMode: .fit)
    }
}

private struct Results: View {
    @ObservedObject var browser: Browser

    var body: some View {
        ScrollView {
            if browser.grid {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: browser.density), spacing: 8)], spacing: 8) {
                    ForEach(browser.rows) { row in tile(row) }
                }.padding(8)
            } else {
                LazyVStack(spacing: 1) { ForEach(browser.rows) { row in line(row) } }
                    .padding(.horizontal, 6)
            }
        }
    }

    private func tile(_ row: FileRow) -> some View {
        VStack(spacing: 6) {
            FileIcon(path: row.path).frame(width: browser.density * 0.55, height: browser.density * 0.55)
            Text(row.name).lineLimit(2).multilineTextAlignment(.center)
            Text(ByteCountFormatter.string(fromByteCount: Int64(row.bytes), countStyle: .file)).font(.caption).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, minHeight: browser.density)
        .padding(6)
        .background(browser.selected == row ? Color.accentColor.opacity(0.22) : Color.clear, in: RoundedRectangle(cornerRadius: 8))
        .contentShape(Rectangle()).onTapGesture { browser.selected = row }
        .onTapGesture(count: 2) { browser.activate(row) }
    }

    private func line(_ row: FileRow) -> some View {
        HStack {
            FileIcon(path: row.path).frame(width: 28, height: 28)
            Text(row.name); Spacer()
            Text(ByteCountFormatter.string(fromByteCount: Int64(row.bytes), countStyle: .file)).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 8).padding(.vertical, 4)
        .background(browser.selected == row ? Color.accentColor.opacity(0.22) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle()).onTapGesture { browser.selected = row }
        .onTapGesture(count: 2) { browser.activate(row) }
    }
}

private struct ContentView: View {
    @StateObject private var browser: Browser
    private let home = FileManager.default.homeDirectoryForCurrentUser

    init(browser: Browser) { _browser = StateObject(wrappedValue: browser) }

    var body: some View {
        NavigationSplitView {
            List {
                Section("Places") {
                    place("Home", "house", home)
                    place("Desktop", "desktopcomputer", home.appendingPathComponent("Desktop"))
                    place("Documents", "doc", home.appendingPathComponent("Documents"))
                    place("Downloads", "arrow.down.circle", home.appendingPathComponent("Downloads"))
                    place("Pictures", "photo", home.appendingPathComponent("Pictures"))
                    place("Music", "music.note", home.appendingPathComponent("Music"))
                    place("Movies", "film", home.appendingPathComponent("Movies"))
                }
            }.navigationSplitViewColumnWidth(min: 160, ideal: 190)
        } detail: {
            HSplitView {
                Results(browser: browser).frame(minWidth: 420)
                Group {
                    if browser.showChart {
                        VStack {
                            Picker("Scope", selection: $browser.globalChart) {
                                Text("Directory").tag(false); Text("Global").tag(true)
                            }.pickerStyle(.segmented).padding()
                            PieChart(rows: browser.chart).padding()
                            List(browser.chart) { row in
                                HStack { Text(row.name); Spacer(); Text(ByteCountFormatter.string(fromByteCount: Int64(row.bytes), countStyle: .file)) }
                                    .contentShape(Rectangle()).onTapGesture { if row.isDirectory { browser.navigate(row.path) } }
                            }
                        }
                    } else {
                        QuickPreview(path: browser.selected?.path)
                    }
                }.frame(minWidth: 280, idealWidth: 360)
            }
            .navigationTitle(URL(fileURLWithPath: browser.directory).lastPathComponent)
            .searchable(text: $browser.query, placement: .toolbar, prompt: "Find in this folder")
            .onSubmit(of: .search) { browser.refresh() }
            .onChange(of: browser.query) { _ in browser.refresh() }
            .onChange(of: browser.recursive) { _ in browser.refresh() }
            .onChange(of: browser.globalChart) { _ in browser.refreshChart() }
            .toolbar {
                ToolbarItemGroup {
                    Button(action: browser.back) { Image(systemName: "chevron.left") }
                    Button(action: browser.forward) { Image(systemName: "chevron.right") }
                    Picker("Mode", selection: $browser.recursive) { Text("Classic").tag(false); Text("Qfind").tag(true) }.pickerStyle(.segmented)
                    Picker("View", selection: $browser.grid) { Image(systemName: "list.bullet").tag(false); Image(systemName: "square.grid.2x2").tag(true) }.pickerStyle(.segmented)
                    Slider(value: $browser.density, in: 88...220).frame(width: 110)
                    Button { browser.showChart.toggle(); if browser.showChart { browser.refreshChart() } } label: { Image(systemName: browser.showChart ? "doc.richtext" : "chart.pie") }
                }
            }
        }
    }

    private func place(_ title: String, _ icon: String, _ url: URL) -> some View {
        Button { browser.navigate(url.path) } label: { Label(title, systemImage: icon) }
            .buttonStyle(.plain)
    }
}

@main
struct QfindMacApp: App {
    var body: some Scene {
        WindowGroup {
            if let browser = Browser() { ContentView(browser: browser) }
            else { VStack { Image(systemName: "magnifyingglass"); Text("Run qfind index, then reopen Qfind.") }.padding() }
        }.windowStyle(.titleBar)
    }
}
