#include "qfind_native.h"

#include <QApplication>
#include <QAbstractSlider>
#include <QAction>
#include <QComboBox>
#include <QColor>
#include <QCursor>
#include <QDesktopServices>
#include <QDir>
#include <QDropEvent>
#include <QElapsedTimer>
#include <QEvent>
#include <QFile>
#include <QFileDialog>
#include <QFileIconProvider>
#include <QFileInfo>
#include <QFutureWatcher>
#include <QFileSystemWatcher>
#include <QImage>
#include <QImageReader>
#include <QHash>
#include <QHeaderView>
#include <QCheckBox>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QInputDialog>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonValue>
#include <QKeySequence>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMouseEvent>
#include <QMenu>
#include <QMainWindow>
#include <QMessageBox>
#include <QMimeData>
#include <QPlainTextEdit>
#include <QPainter>
#include <QPixmap>
#include <QPushButton>
#include <QRegularExpression>
#include <QScrollBar>
#include <QSpinBox>
#include <QStyle>
#include <QSplitter>
#include <QStackedWidget>
#include <QStandardPaths>
#include <QStatusBar>
#include <QSettings>
#include <QSet>
#include <QSignalBlocker>
#include <QToolBar>
#include <QThreadPool>
#include <QTableWidget>
#include <QTabWidget>
#include <QTextCursor>
#include <QToolTip>
#include <QTreeWidget>
#include <QUrl>
#include <QVariant>
#include <QVBoxLayout>
#include <QWidget>
#include <QTimer>
#include <QtConcurrent/QtConcurrentRun>

#include <cstdint>
#include <algorithm>
#include <cmath>
#include <iterator>
#include <functional>
#include <mutex>
#include <memory>
#include <utility>

extern "C" std::uint64_t qfind_folder_sizes_revision();

namespace {

struct Row {
    QString name;
    QString path;
    std::uint64_t bytes = 0;
    std::uint64_t entries = 0;
    bool directory = false;
};

struct Rows {
    QList<Row> values;
};

struct Text {
    QString value;
};

enum class NativeAction {
    Refresh,
    Navigate,
    Back,
    Forward,
};

struct NativeHandle {
    explicit NativeHandle(QfindManager *value)
        : manager(value)
    {
    }

    ~NativeHandle()
    {
        qfind_manager_free(manager);
    }

    QfindManager *manager = nullptr;
};

struct NativeResult {
    NativeAction action = NativeAction::Refresh;
    QfindManager *manager = nullptr;
    std::shared_ptr<NativeHandle> handle;
    QString operation;
    QString directory;
    QString error;
    QList<Row> rows;
    int status = 0;
};

struct FileResult {
    QString operation;
    QStringList failures;
    int completed = 0;
};

struct PreviewResult {
    QString path;
    QString title;
    QString text;
};

struct ThumbnailResult {
    QString path;
    QImage image;
};

struct DiffHunk {
    QString header;
    QStringList old_lines;
    QStringList new_lines;
};

struct ComponentResult {
    QString component;
    QString action;
    QString response;
    QString error;
    int status = 0;
};

extern "C" void collectRow(void *context, const QfindRow *row)
{
    if (!context || !row) {
        return;
    }
    auto *rows = static_cast<Rows *>(context);
    Row value;
    value.name = QString::fromUtf8(row->name ? row->name : "");
    value.path = QString::fromUtf8(row->path ? row->path : "");
    value.bytes = row->bytes;
    value.entries = row->entries;
    value.directory = row->is_dir != 0;
    rows->values.push_back(std::move(value));
}

extern "C" void collectText(void *context, const char *text)
{
    if (context) {
        static_cast<Text *>(context)->value = QString::fromUtf8(text ? text : "");
    }
}

QString formatBytes(std::uint64_t bytes)
{
    static const char *units[] = {"B", "KB", "MB", "GB", "TB"};
    double value = static_cast<double>(bytes);
    std::size_t unit = 0;
    while (value >= 1024.0 && unit + 1 < std::size(units)) {
        value /= 1024.0;
        ++unit;
    }
    if (unit == 0) {
        return QStringLiteral("%1 B").arg(bytes);
    }
    return QStringLiteral("%1 %2").arg(value, 0, 'f', value >= 10.0 ? 1 : 2).arg(units[unit]);
}

PreviewResult loadPreview(const QString &path)
{
    PreviewResult result;
    result.path = path;
    const QFileInfo info(path);
    result.title = QStringLiteral("%1\n%2\n%3")
                       .arg(info.fileName(), info.isDir() ? QStringLiteral("Folder") : formatBytes(info.size()), path);
    if (info.isDir() || !info.isFile()) {
        return result;
    }
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) {
        result.text = QStringLiteral("Cannot read this file.");
        return result;
    }
    const QByteArray bytes = file.read(64 * 1024);
    if (bytes.contains('\0')) {
        result.text = QStringLiteral("Binary file\n\nUse Open to launch it with the native desktop application.");
    } else {
        result.text = QString::fromUtf8(bytes);
    }
    return result;
}

ThumbnailResult loadThumbnail(const QString &path)
{
    ThumbnailResult result;
    result.path = path;
    QImageReader reader(path);
    reader.setAutoTransform(true);
    const QSize source_size = reader.size();
    const QSize target = source_size.isValid() ? source_size.scaled(QSize(96, 96), Qt::KeepAspectRatio)
                                               : QSize(96, 96);
    reader.setScaledSize(target);
    result.image = reader.read();
    return result;
}

bool looksLikeImage(const QString &path)
{
    static const QSet<QString> extensions = {
        QStringLiteral("avif"), QStringLiteral("bmp"), QStringLiteral("gif"), QStringLiteral("heic"),
        QStringLiteral("heif"), QStringLiteral("ico"), QStringLiteral("jpeg"), QStringLiteral("jpg"),
        QStringLiteral("png"), QStringLiteral("tif"), QStringLiteral("tiff"), QStringLiteral("webp")};
    return extensions.contains(QFileInfo(path).suffix().toLower());
}

QString nativeError(QfindManager *manager, int status)
{
    Text text;
    if (manager && qfind_manager_error(manager, collectText, &text) == 0 && !text.value.isEmpty()) {
        return text.value;
    }
    return QStringLiteral("Native operation failed (%1)").arg(status);
}

NativeResult loadNativeLocked(QfindManager *manager, NativeAction action, const QString &path,
                              bool global, bool recursive, unsigned sort, const QString &query)
{
    NativeResult result;
    result.action = action;
    if (!manager) {
        result.operation = QStringLiteral("Open manager");
        result.error = QStringLiteral("The native Qfind manager is unavailable.");
        result.status = -1;
        return result;
    }

    auto failed = [&](const QString &operation, int status) {
        result.operation = operation;
        result.status = status;
        result.error = nativeError(manager, status);
        return result;
    };
    int status = 0;
    if (action == NativeAction::Navigate) {
        const QByteArray utf8 = path.toUtf8();
        status = qfind_manager_navigate(manager, utf8.constData());
        if (status != 0) {
            return failed(QStringLiteral("Open folder"), status);
        }
    } else if (action == NativeAction::Back) {
        status = qfind_manager_back(manager);
        if (status != 0) {
            return failed(QStringLiteral("Back"), status);
        }
    } else if (action == NativeAction::Forward) {
        status = qfind_manager_forward(manager);
        if (status != 0) {
            return failed(QStringLiteral("Forward"), status);
        }
    }

    status = qfind_manager_search_scope(manager, global ? 1 : 0);
    if (status != 0) {
        return failed(QStringLiteral("Set search scope"), status);
    }
    status = qfind_manager_sort(manager, sort);
    if (status != 0) {
        return failed(QStringLiteral("Set sort"), status);
    }

    Text directory;
    status = qfind_manager_directory(manager, collectText, &directory);
    if (status != 0) {
        return failed(QStringLiteral("Read location"), status);
    }
    result.directory = directory.value;

    const QByteArray query_utf8 = query.toUtf8();
    Rows rows;
    status = qfind_manager_rows(manager, query_utf8.constData(), recursive ? 1 : 0, 5000,
                                collectRow, &rows);
    if (status != 0) {
        return failed(QStringLiteral("Load files"), status);
    }
    result.rows = std::move(rows.values);
    return result;
}

NativeResult loadNative(QfindManager *manager, std::mutex &mutex, NativeAction action,
                        const QString &path, bool global, bool recursive, unsigned sort,
                        const QString &query)
{
    std::lock_guard lock(mutex);
    return loadNativeLocked(manager, action, path, global, recursive, sort, query);
}

NativeResult initializeNative(std::mutex &mutex, const QString &initial)
{
    std::lock_guard lock(mutex);
    NativeResult result;
    result.operation = QStringLiteral("Open manager");
    const QByteArray path = initial.toUtf8();
    result.manager = qfind_manager_open(path.constData());
    if (!result.manager) {
        result.status = -1;
        result.error = QStringLiteral("Unable to open the native Qfind manager.");
    } else {
        result.handle = std::make_shared<NativeHandle>(result.manager);
    }
    return result;
}

ComponentResult loadComponent(QfindManager *manager, const QString &component, const QJsonObject &request)
{
    ComponentResult result;
    result.component = component;
    result.action = request.value(QStringLiteral("action")).toString();
    const QByteArray component_utf8 = component.toUtf8();
    const QByteArray request_utf8 = QJsonDocument(request).toJson(QJsonDocument::Compact);
    Text response;
    const int status = qfind_manager_component(manager, component_utf8.constData(), request_utf8.constData(),
                                               collectText, &response);
    result.status = status;
    if (status == 0) {
        result.response = response.value;
    } else {
        result.error = response.value.isEmpty() ? nativeError(manager, status) : response.value;
    }
    return result;
}

class FileTree final : public QTreeWidget {
public:
    using QTreeWidget::QTreeWidget;

    void setDropHandler(std::function<void(const QStringList &, Qt::DropAction)> handler)
    {
        drop_handler_ = std::move(handler);
    }

protected:
    QMimeData *mimeData(const QList<QTreeWidgetItem *> &items) const override
    {
        auto *mime = new QMimeData;
        QList<QUrl> urls;
        for (auto *item : items) {
            const QString path = item->data(0, Qt::UserRole).toString();
            if (!path.isEmpty()) {
                urls.push_back(QUrl::fromLocalFile(path));
            }
        }
        mime->setUrls(urls);
        return mime;
    }

protected:
    void dropEvent(QDropEvent *event) override
    {
        QStringList paths;
        for (const QUrl &url : event->mimeData()->urls()) {
            if (url.isLocalFile()) {
                paths.push_back(url.toLocalFile());
            }
        }
        if (paths.isEmpty() || !drop_handler_) {
            event->ignore();
            return;
        }
        const Qt::DropAction action = event->dropAction() == Qt::MoveAction ? Qt::MoveAction : Qt::CopyAction;
        drop_handler_(paths, action);
        event->setDropAction(action);
        event->accept();
    }

private:
    std::function<void(const QStringList &, Qt::DropAction)> drop_handler_;
};

class FileGrid final : public QListWidget {
public:
    using QListWidget::QListWidget;

    void setDropHandler(std::function<void(const QStringList &, Qt::DropAction)> handler)
    {
        drop_handler_ = std::move(handler);
    }

protected:
    QMimeData *mimeData(const QList<QListWidgetItem *> &items) const override
    {
        auto *mime = new QMimeData;
        QList<QUrl> urls;
        for (auto *item : items) {
            const QString path = item->data(Qt::UserRole).toString();
            if (!path.isEmpty()) {
                urls.push_back(QUrl::fromLocalFile(path));
            }
        }
        mime->setUrls(urls);
        return mime;
    }

    void dropEvent(QDropEvent *event) override
    {
        QStringList paths;
        for (const QUrl &url : event->mimeData()->urls()) {
            if (url.isLocalFile()) {
                paths.push_back(url.toLocalFile());
            }
        }
        if (paths.isEmpty() || !drop_handler_) {
            event->ignore();
            return;
        }
        const Qt::DropAction action = event->dropAction() == Qt::MoveAction ? Qt::MoveAction : Qt::CopyAction;
        drop_handler_(paths, action);
        event->setDropAction(action);
        event->accept();
    }

private:
    std::function<void(const QStringList &, Qt::DropAction)> drop_handler_;
};

struct StorageSlice {
    QString name;
    QString path;
    std::uint64_t bytes = 0;
    QColor color;
};

class StorageMapWidget final : public QWidget {
public:
    using HoverHandler = std::function<void(const QString &)>;

    explicit StorageMapWidget(QWidget *parent = nullptr)
        : QWidget(parent)
    {
        setMouseTracking(true);
        setMinimumHeight(420);
    }

    void setHoverHandler(HoverHandler handler)
    {
        hover_handler_ = std::move(handler);
    }

    void setData(QList<StorageSlice> slices, std::uint64_t free_bytes, std::uint64_t total_bytes)
    {
        slices_ = std::move(slices);
        std::sort(slices_.begin(), slices_.end(), [](const StorageSlice &left, const StorageSlice &right) {
            return left.bytes > right.bytes;
        });
        segments_.clear();
        std::uint64_t entry_bytes = 0;
        std::uint64_t omitted_bytes = 0;
        int color_index = 0;
        for (int index = 0; index < slices_.size(); ++index) {
            StorageSlice &slice = slices_[index];
            if (!slice.bytes) {
                continue;
            }
            entry_bytes = entry_bytes > UINT64_MAX - slice.bytes ? UINT64_MAX : entry_bytes + slice.bytes;
            if (color_index < 12) {
                slice.color = QColor::fromHsv((color_index++ * 47) % 360, 165, 215);
                segments_.push_back(slice);
            } else {
                omitted_bytes = omitted_bytes > UINT64_MAX - slice.bytes ? UINT64_MAX : omitted_bytes + slice.bytes;
            }
        }
        free_bytes_ = free_bytes;
        total_bytes_ = total_bytes;
        const std::uint64_t used_plus_free = entry_bytes > UINT64_MAX - free_bytes_ ? UINT64_MAX : entry_bytes + free_bytes_;
        other_bytes_ = total_bytes_ > used_plus_free ? total_bytes_ - used_plus_free : 0;
        if (!total_bytes_) {
            total_bytes_ = entry_bytes + free_bytes_ + other_bytes_;
        }
        if (omitted_bytes) {
            segments_.push_back({QStringLiteral("Other entries"), QString(), omitted_bytes, QColor(170, 145, 90)});
        }
        if (other_bytes_) {
            segments_.push_back({QStringLiteral("Other used"), QString(), other_bytes_, QColor(120, 125, 135)});
        }
        if (free_bytes_) {
            segments_.push_back({QStringLiteral("Free"), QString(), free_bytes_, QColor(80, 180, 115)});
        }
        hovered_segment_ = -1;
        update();
    }

protected:
    void paintEvent(QPaintEvent *) override
    {
        QPainter painter(this);
        painter.setRenderHint(QPainter::Antialiasing, true);
        painter.fillRect(rect(), palette().base());
        const qreal pie_size = qMin(qreal(220), qMax(qreal(100), qMin(width() * 0.42, height() * 0.42)));
        pie_rect_ = QRectF(14, 14, pie_size, pie_size);
        const qreal chart_bottom = qMax(pie_rect_.bottom(), pie_rect_.top() + segments_.size() * 24.0);
        treemap_rect_ = QRectF(14, chart_bottom + 42, qMax(qreal(1), qreal(width() - 28)),
                               qMax(qreal(1), height() - chart_bottom - 56));
        painter.setPen(palette().text().color());
        painter.drawText(QRectF(14, treemap_rect_.top() - 30, width() - 28, 24), Qt::AlignLeft,
                         QStringLiteral("Largest entries in this location"));

        if (!total_bytes_ || segments_.isEmpty()) {
            painter.drawText(pie_rect_, Qt::AlignCenter, QStringLiteral("No per-entry size data"));
            painter.drawText(treemap_rect_, Qt::AlignCenter,
                             QStringLiteral("Build an index to calculate folder sizes."));
            return;
        }

        pie_segments_.clear();
        treemap_rects_.clear();
        treemap_indices_.clear();
        qreal angle = 0;
        const qreal total = static_cast<qreal>(total_bytes_);
        for (int index = 0; index < segments_.size(); ++index) {
            const StorageSlice &segment = segments_.at(index);
            const qreal span = static_cast<qreal>(segment.bytes) / total * 360.0;
            painter.setBrush(segment.color);
            painter.setPen(palette().base().color());
            painter.drawPie(pie_rect_, qRound(angle * 16.0), qRound(span * 16.0));
            pie_segments_.push_back({angle, span});
            angle += span;
        }

        painter.setPen(palette().text().color());
        qreal legend_y = pie_rect_.top();
        for (int index = 0; index < segments_.size(); ++index) {
            const StorageSlice &segment = segments_.at(index);
            const QRectF swatch( pie_rect_.right() + 22, legend_y + 3, 12, 12);
            painter.fillRect(swatch, segment.color);
            painter.drawText(QRectF(swatch.right() + 8, legend_y, width() - swatch.right() - 20, 20),
                             Qt::AlignLeft | Qt::AlignVCenter,
                             QStringLiteral("%1  %2").arg(segment.name, formatBytes(segment.bytes)));
            legend_y += 24;
        }

        const QList<StorageSlice> entries = entrySegments();
        std::uint64_t known = 0;
        for (const StorageSlice &entry : entries) {
            known = known > UINT64_MAX - entry.bytes ? UINT64_MAX : known + entry.bytes;
        }
        if (!known) {
            painter.drawText(treemap_rect_, Qt::AlignCenter, QStringLiteral("No per-entry size data"));
            return;
        }
        qreal x = treemap_rect_.left();
        const qreal width_scale = treemap_rect_.width() / static_cast<qreal>(known);
        for (const StorageSlice &entry : entries) {
            const qreal box_width = qMax(qreal(2), static_cast<qreal>(entry.bytes) * width_scale);
            const QRectF box(x, treemap_rect_.top(), qMin(box_width, treemap_rect_.right() - x), treemap_rect_.height());
            treemap_rects_.push_back(box);
            int segment_index = -1;
            for (int index = 0; index < segments_.size(); ++index) {
                if (segments_.at(index).path == entry.path) {
                    segment_index = index;
                    break;
                }
            }
            treemap_indices_.push_back(segment_index);
            painter.setBrush(entry.color);
            painter.setPen(palette().base().color());
            painter.drawRect(box);
            if (box.width() > 46) {
                painter.setPen(Qt::white);
                painter.drawText(box.adjusted(6, 6, -6, -6), Qt::AlignLeft | Qt::AlignTop | Qt::TextWordWrap,
                                 QStringLiteral("%1\n%2").arg(entry.name, formatBytes(entry.bytes)));
            }
            x += box_width;
            if (x >= treemap_rect_.right()) {
                break;
            }
        }
    }

    void mouseMoveEvent(QMouseEvent *event) override
    {
        const int segment = hitSegment(event->pos());
        if (segment == hovered_segment_) {
            return;
        }
        hovered_segment_ = segment;
        if (segment < 0 || segment >= segments_.size()) {
            setToolTip(QString());
            if (hover_handler_) {
                hover_handler_(QString());
            }
            return;
        }
        const StorageSlice &slice = segments_.at(segment);
        const QString tip = QStringLiteral("%1\n%2").arg(slice.name, formatBytes(slice.bytes));
        setToolTip(tip);
        QToolTip::showText(QCursor::pos(), tip, this);
        if (hover_handler_) {
            hover_handler_(slice.path);
        }
    }

    void leaveEvent(QEvent *event) override
    {
        hovered_segment_ = -1;
        QWidget::leaveEvent(event);
    }

private:
    QList<StorageSlice> entrySegments() const
    {
        QList<StorageSlice> entries;
        for (const StorageSlice &slice : segments_) {
            if (!slice.path.isEmpty()) {
                entries.push_back(slice);
            }
        }
        return entries;
    }

    int hitSegment(const QPoint &point) const
    {
        if (pie_rect_.contains(point)) {
            const qreal dx = point.x() - pie_rect_.center().x();
            const qreal dy = point.y() - pie_rect_.center().y();
            if (std::hypot(dx, dy) <= pie_rect_.width() / 2.0) {
                qreal angle = std::atan2(-dy, dx) * 180.0 / 3.14159265358979323846;
                if (angle < 0) {
                    angle += 360.0;
                }
                qreal start = 0;
                for (int index = 0; index < pie_segments_.size(); ++index) {
                    const qreal span = pie_segments_.at(index).second;
                    if (angle >= start && angle < start + span) {
                        return index;
                    }
                    start += span;
                }
            }
        }
        for (int index = 0; index < treemap_rects_.size(); ++index) {
            if (treemap_rects_.at(index).contains(point)) {
                return treemap_indices_.at(index);
            }
        }
        return -1;
    }

    QList<StorageSlice> slices_;
    QList<StorageSlice> segments_;
    QList<QPair<qreal, qreal>> pie_segments_;
    QList<QRectF> treemap_rects_;
    QList<int> treemap_indices_;
    QRectF pie_rect_;
    QRectF treemap_rect_;
    HoverHandler hover_handler_;
    std::uint64_t free_bytes_ = 0;
    std::uint64_t other_bytes_ = 0;
    std::uint64_t total_bytes_ = 0;
    int hovered_segment_ = -1;
};

struct AsyncPools {
    QThreadPool native;
    QThreadPool component;
    QThreadPool file;
    QThreadPool preview;
    QThreadPool thumbnail;

    AsyncPools()
    {
        native.setMaxThreadCount(1);
        component.setMaxThreadCount(2);
        file.setMaxThreadCount(1);
        preview.setMaxThreadCount(1);
        thumbnail.setMaxThreadCount(1);
    }
};

AsyncPools &asyncPools()
{
    static auto *pools = new AsyncPools;
    return *pools;
}

class Window final : public QMainWindow {
public:
    explicit Window(const QString &initial_directory)
        : initial_directory_(initial_directory),
          native_pool_(asyncPools().native),
          component_pool_(asyncPools().component),
          file_pool_(asyncPools().file),
          preview_pool_(asyncPools().preview),
          thumbnail_pool_(asyncPools().thumbnail)
    {
        setWindowTitle(QStringLiteral("Megaman"));
        resize(1200, 760);

        makeToolbar();
        makePlaces();
        makeContent();
        git_footer_ = new QLabel(QStringLiteral("Git: checking…"), this);
        git_footer_->setToolTip(QStringLiteral("Git status for the current folder"));
        statusBar()->addPermanentWidget(git_footer_);
        folder_size_timer_.setInterval(1000);
        folder_size_timer_.setSingleShot(false);
        connect(&folder_size_timer_, &QTimer::timeout, this, [this] { pollFolderSizes(); });
        folder_size_timer_.start();
        directory_watcher_ = new QFileSystemWatcher(this);
        directory_change_debounce_.setSingleShot(true);
        directory_change_debounce_.setInterval(180);
        connect(directory_watcher_, &QFileSystemWatcher::directoryChanged, this, [this](const QString &path) {
            if (path == directory_) {
                pending_selection_paths_ = selectedPaths();
                directory_change_debounce_.start();
            }
        });
        connect(&directory_change_debounce_, &QTimer::timeout, this, [this] {
            if (!manager_ || directory_.isEmpty()) {
                return;
            }
            pending_selection_paths_ = selectedPaths();
            requestRefresh();
            requestStorage();
        });
        thumbnail_debounce_.setSingleShot(true);
        thumbnail_debounce_.setInterval(150);
        connect(&thumbnail_debounce_, &QTimer::timeout, this, [this] { scheduleGridThumbnails(); });
        requestRefresh();
    }

    ~Window() override
    {
        saveSettings();
        // Futures keep their captured NativeHandle alive after this window closes. The
        // process-wide pools outlive Window, so closing never waits for a build/archive.
        // ponytail: process-exit teardown is the deliberate lifetime ceiling for these pools.
        manager_ = nullptr;
        native_handle_.reset();
    }

private:
    void makeToolbar()
    {
        auto *toolbar = addToolBar(QStringLiteral("Navigation"));
        toolbar->setMovable(false);
        toolbar->setIconSize(QSize(18, 18));
        toolbar->setToolButtonStyle(Qt::ToolButtonIconOnly);
        const auto add_icon_action = [this, toolbar](QStyle::StandardPixmap icon, const QString &name) {
            auto *action = toolbar->addAction(style()->standardIcon(icon), name);
            action->setToolTip(name);
            action->setStatusTip(name);
            return action;
        };

        auto *back = add_icon_action(QStyle::SP_ArrowBack, QStringLiteral("Back"));
        auto *forward = add_icon_action(QStyle::SP_ArrowForward, QStringLiteral("Forward"));
        auto *up = add_icon_action(QStyle::SP_ArrowUp, QStringLiteral("Up"));
        toolbar->addSeparator();
        auto *choose = add_icon_action(QStyle::SP_DirOpenIcon, QStringLiteral("Choose folder"));
        auto *open = add_icon_action(QStyle::SP_DialogOpenButton, QStringLiteral("Open"));
        auto *reveal = add_icon_action(QStyle::SP_FileDialogContentsView, QStringLiteral("Reveal"));
        auto *copy = add_icon_action(QStyle::SP_FileIcon, QStringLiteral("Copy"));
        auto *rename = add_icon_action(QStyle::SP_FileDialogInfoView, QStringLiteral("Rename"));
        auto *trash = add_icon_action(QStyle::SP_TrashIcon, QStringLiteral("Trash"));
        auto *selection_menu = new QMenu(QStringLiteral("Selection"), this);
        auto *select_all = selection_menu->addAction(QStringLiteral("Select all"));
        select_all->setShortcut(QKeySequence::SelectAll);
        select_all->setShortcutContext(Qt::WindowShortcut);
        auto *clear_selection = selection_menu->addAction(QStringLiteral("Clear selection"));
        auto *selection = selection_menu->menuAction();
        selection->setIcon(style()->standardIcon(QStyle::SP_FileDialogDetailedView));
        toolbar->addAction(selection);
        selection->setToolTip(QStringLiteral("Selection actions"));
        selection->setStatusTip(QStringLiteral("Selection actions"));
        toolbar->addSeparator();

        search_ = new QLineEdit(this);
        search_->setClearButtonEnabled(true);
        search_->setPlaceholderText(QStringLiteral("Search files in this folder"));
        search_->setMinimumWidth(220);
        toolbar->addWidget(search_);

        location_ = new QLineEdit(this);
        location_->setPlaceholderText(QStringLiteral("Location"));
        location_->setClearButtonEnabled(false);
        location_->setMinimumWidth(220);
        toolbar->addWidget(location_);
        connect(location_, &QLineEdit::returnPressed, this, [this] { navigate(location_->text()); });

        scope_ = new QComboBox(this);
        scope_->addItem(QStringLiteral("Folder (live)"), 0u);
        scope_->addItem(QStringLiteral("Folder (indexed)"), 1u);
        scope_->addItem(QStringLiteral("Everywhere"), 2u);
        scope_->setToolTip(QStringLiteral("Search scope"));
        scope_->setMinimumWidth(105);
        scope_->setMaximumWidth(140);
        toolbar->addWidget(scope_);

        sort_ = new QComboBox(this);
        sort_->addItem(QStringLiteral("Relevance"), 0u);
        sort_->addItem(QStringLiteral("Name"), 1u);
        sort_->addItem(QStringLiteral("Name, descending"), 2u);
        sort_->addItem(QStringLiteral("Newest"), 3u);
        sort_->addItem(QStringLiteral("Oldest"), 4u);
        sort_->addItem(QStringLiteral("Largest"), 5u);
        sort_->addItem(QStringLiteral("Smallest"), 6u);
        sort_->setToolTip(QStringLiteral("Sort results"));
        sort_->setMinimumWidth(95);
        sort_->setMaximumWidth(125);
        toolbar->addWidget(sort_);

        auto *preview = add_icon_action(QStyle::SP_FileDialogInfoView, QStringLiteral("Preview"));
        preview->setCheckable(true);
        preview->setChecked(true);
        grid_view_action_ = add_icon_action(QStyle::SP_FileDialogContentsView, QStringLiteral("Grid view"));
        grid_view_action_->setCheckable(true);
        grid_view_action_->setToolTip(QStringLiteral("Show files as an icon grid"));
        auto *global = add_icon_action(QStyle::SP_FileDialogContentsView, QStringLiteral("Global search"));
        global->setShortcut(QKeySequence(QStringLiteral("Ctrl+G")));
        global->setShortcutContext(Qt::WindowShortcut);

        connect(back, &QAction::triggered, this, [this] { moveHistory(false); });
        connect(forward, &QAction::triggered, this, [this] { moveHistory(true); });
        connect(up, &QAction::triggered, this, [this] {
            if (!directory_.isEmpty()) {
                navigate(QFileInfo(directory_).dir().absolutePath());
            }
        });
        connect(choose, &QAction::triggered, this, [this] {
            const QString path = QFileDialog::getExistingDirectory(this, QStringLiteral("Choose folder"), directory_);
            if (!path.isEmpty()) {
                navigate(path);
            }
        });
        connect(open, &QAction::triggered, this, [this] { openSelected(); });
        connect(reveal, &QAction::triggered, this, [this] { revealSelected(); });
        connect(copy, &QAction::triggered, this, [this] { copySelected(); });
        connect(rename, &QAction::triggered, this, [this] { renameSelected(); });
        connect(trash, &QAction::triggered, this, [this] { trashSelected(); });
        connect(select_all, &QAction::triggered, this, [this] {
            if (results_) {
                results_->selectAll();
            }
            if (grid_results_) {
                grid_results_->selectAll();
            }
        });
        connect(clear_selection, &QAction::triggered, this, [this] {
            if (results_) {
                results_->clearSelection();
            }
            if (grid_results_) {
                grid_results_->clearSelection();
            }
        });
        connect(preview, &QAction::toggled, this, [this](bool visible) {
            if (preview_pane_) {
                preview_pane_->setVisible(visible);
            }
        });
        connect(grid_view_action_, &QAction::toggled, this, [this](bool grid) {
            if (file_views_) {
                file_views_->setCurrentIndex(grid ? 1 : 0);
                showSelection();
                if (grid) {
                    thumbnail_debounce_.start();
                }
            }
        });
        connect(global, &QAction::triggered, this, [this] {
            scope_->setCurrentIndex(2);
            search_->setFocus();
            search_->selectAll();
        });
        connect(search_, &QLineEdit::textChanged, this, [this] { debounce_.start(); });
        connect(scope_, qOverload<int>(&QComboBox::currentIndexChanged), this, [this] { requestRefresh(); });
        connect(sort_, qOverload<int>(&QComboBox::currentIndexChanged), this, [this] { requestRefresh(); });
        connect(search_, &QLineEdit::returnPressed, this, [this] { requestRefresh(); });
        debounce_.setSingleShot(true);
        debounce_.setInterval(120);
        connect(&debounce_, &QTimer::timeout, this, [this] { requestRefresh(); });
    }

    void makePlaces()
    {
        places_ = new QListWidget(this);
        places_->setMinimumWidth(165);
        places_->setMaximumWidth(260);
        places_->setIconSize(QSize(18, 18));
        addPlace(QStringLiteral("Home"), QDir::homePath(), QStyle::SP_DirHomeIcon);
        addPlace(QStringLiteral("Desktop"), QStandardPaths::writableLocation(QStandardPaths::DesktopLocation));
        addPlace(QStringLiteral("Documents"), QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation));
        addPlace(QStringLiteral("Downloads"), QStandardPaths::writableLocation(QStandardPaths::DownloadLocation));
        addPlace(QStringLiteral("Pictures"), QStandardPaths::writableLocation(QStandardPaths::PicturesLocation));
        addPlace(QStringLiteral("Music"), QStandardPaths::writableLocation(QStandardPaths::MusicLocation));
        addPlace(QStringLiteral("Videos"), QStandardPaths::writableLocation(QStandardPaths::MoviesLocation));
        connect(places_, &QListWidget::itemClicked, this, [this](QListWidgetItem *item) {
            navigate(item->data(Qt::UserRole).toString());
        });
    }

    void addPlace(const QString &name, const QString &path, QStyle::StandardPixmap icon = QStyle::SP_DirIcon)
    {
        if (path.isEmpty() || !QFileInfo(path).isDir()) {
            return;
        }
        auto *item = new QListWidgetItem(style()->standardIcon(icon), name, places_);
        item->setData(Qt::UserRole, path);
        item->setToolTip(path);
    }

    void makeContent()
    {
        auto *content = new QSplitter(Qt::Horizontal, this);
        content->addWidget(places_);

        file_views_ = new QStackedWidget(content);
        results_ = new FileTree(file_views_);
        results_->setHeaderLabels({QStringLiteral("Name"), QStringLiteral("Kind"), QStringLiteral("Size"), QStringLiteral("Location")});
        results_->setSelectionMode(QAbstractItemView::ExtendedSelection);
        results_->setSelectionBehavior(QAbstractItemView::SelectRows);
        results_->setDragEnabled(true);
        results_->setAcceptDrops(true);
        results_->setDragDropMode(QAbstractItemView::DragDrop);
        results_->setDefaultDropAction(Qt::CopyAction);
        results_->setAlternatingRowColors(true);
        results_->setUniformRowHeights(true);
        results_->header()->setStretchLastSection(true);
        results_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
        results_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
        results_->header()->setSectionResizeMode(2, QHeaderView::ResizeToContents);
        results_->header()->setSectionsMovable(true);
        results_->header()->setContextMenuPolicy(Qt::CustomContextMenu);
        connect(results_->header(), &QWidget::customContextMenuRequested, this, [this](const QPoint &point) {
            QMenu menu(this);
            const QStringList labels = {QStringLiteral("Name"), QStringLiteral("Kind"), QStringLiteral("Size"),
                                        QStringLiteral("Location")};
            for (int column = 0; column < labels.size(); ++column) {
                auto *action = menu.addAction(labels.at(column));
                action->setCheckable(true);
                action->setChecked(!results_->isColumnHidden(column));
                connect(action, &QAction::toggled, this, [this, column](bool visible) {
                    results_->setColumnHidden(column, !visible);
                });
            }
            menu.exec(results_->header()->mapToGlobal(point));
        });
        {
            QSettings settings;
            const QByteArray header_state = settings.value(QStringLiteral("files/headerState")).toByteArray();
            if (!header_state.isEmpty()) {
                results_->header()->restoreState(header_state);
            }
        }
        results_->setDropHandler([this](const QStringList &paths, Qt::DropAction action) {
            importDropped(paths, action);
        });
        connect(results_, &QTreeWidget::itemDoubleClicked, this,
                [this](QTreeWidgetItem *item, int) { activate(item); });
        connect(results_, &QTreeWidget::itemSelectionChanged, this, [this] { showSelection(); });
        file_views_->addWidget(results_);

        grid_results_ = new FileGrid(file_views_);
        grid_results_->setViewMode(QListView::IconMode);
        grid_results_->setMovement(QListView::Static);
        grid_results_->setResizeMode(QListView::Adjust);
        grid_results_->setIconSize(QSize(48, 48));
        grid_results_->setGridSize(QSize(112, 86));
        grid_results_->setSelectionMode(QAbstractItemView::ExtendedSelection);
        grid_results_->setSelectionBehavior(QAbstractItemView::SelectItems);
        grid_results_->setDragEnabled(true);
        grid_results_->setAcceptDrops(true);
        grid_results_->setDragDropMode(QAbstractItemView::DragDrop);
        grid_results_->setDefaultDropAction(Qt::CopyAction);
        grid_results_->setSpacing(6);
        grid_results_->setDropHandler([this](const QStringList &paths, Qt::DropAction action) {
            importDropped(paths, action);
        });
        connect(grid_results_->verticalScrollBar(), &QAbstractSlider::valueChanged, this, [this] {
            thumbnail_debounce_.start();
        });
        connect(grid_results_, &QListWidget::itemDoubleClicked, this,
                [this](QListWidgetItem *item) { activateGrid(item); });
        connect(grid_results_, &QListWidget::itemSelectionChanged, this, [this] { showSelection(); });
        file_views_->addWidget(grid_results_);
        {
            QSettings settings;
            grid_view_action_->setChecked(settings.value(QStringLiteral("files/gridView"), false).toBool());
        }
        content->addWidget(file_views_);

        preview_pane_ = new QWidget(content);
        auto *preview_layout = new QVBoxLayout(preview_pane_);
        preview_title_ = new QLabel(preview_pane_);
        preview_title_->setWordWrap(true);
        preview_title_->setTextInteractionFlags(Qt::TextSelectableByMouse);
        preview_text_ = new QPlainTextEdit(preview_pane_);
        preview_text_->setReadOnly(true);
        preview_text_->setLineWrapMode(QPlainTextEdit::WidgetWidth);
        preview_layout->addWidget(preview_title_);
        preview_layout->addWidget(preview_text_, 1);
        content->addWidget(preview_pane_);
        content->setStretchFactor(1, 1);
        content->setStretchFactor(2, 0);
        content->setSizes({180, 700, 320});
        file_splitter_ = content;
        {
            QSettings settings;
            const QByteArray splitter_state = settings.value(QStringLiteral("files/splitterState")).toByteArray();
            if (!splitter_state.isEmpty()) {
                content->restoreState(splitter_state);
            }
        }
        browser_page_ = content;
        workspaces_ = new QTabWidget(this);
        workspaces_->addTab(browser_page_, QStringLiteral("Files"));
        setCentralWidget(workspaces_);
    }

    static QPushButton *button(const QString &title, QWidget *parent)
    {
        return new QPushButton(title, parent);
    }

    void makeComponentPage(const QJsonObject &definition)
    {
        const QString id = definition.value(QStringLiteral("id")).toString();
        if (id.isEmpty() || id == QStringLiteral("shell") || component_pages_.contains(id)) {
            return;
        }
        auto *page = new QWidget(workspaces_);
        auto *layout = new QVBoxLayout(page);
        auto *commands = new QHBoxLayout;
        component_commands_.insert(id, commands);
        layout->addLayout(commands);
        component_pages_.insert(id, page);
        workspaces_->addTab(page, definition.value(QStringLiteral("title")).toString(id));

        const QJsonArray command_values = definition.value(QStringLiteral("commands")).toArray();
        for (const QJsonValue &value : command_values) {
            const QJsonObject command = value.toObject();
            const QString command_id = command.value(QStringLiteral("id")).toString();
            if (command_id.isEmpty()) {
                continue;
            }
            auto *control = button(command.value(QStringLiteral("title")).toString(command_id), page);
            control->setToolTip(command.value(QStringLiteral("mutating")).toBool()
                                    ? QStringLiteral("Changes files or project state")
                                    : QStringLiteral("Read from the native component"));
            commands->addWidget(control);
            connect(control, &QPushButton::clicked, this, [this, id, command_id] {
                dispatchComponentCommand(id, command_id);
            });
        }

        if (id == QStringLiteral("projects")) {
            makeProjectsPage(layout, page);
        } else if (id == QStringLiteral("git")) {
            makeGitPage(layout, page);
        } else if (id == QStringLiteral("tasks")) {
            makeTasksPage(layout, page);
        } else if (id == QStringLiteral("batch")) {
            makeBatchPage(layout, page);
        } else if (id == QStringLiteral("storage")) {
            makeStoragePage(layout, page);
        } else if (id == QStringLiteral("archives")) {
            makeArchivePage(layout, page);
        } else {
            layout->addWidget(new QLabel(QStringLiteral("Native component: %1").arg(id), page));
            layout->addStretch();
        }
    }

    void makeProjectsPage(QVBoxLayout *layout, QWidget *page)
    {
        auto *row = new QHBoxLayout;
        row->addWidget(new QLabel(QStringLiteral("Repositories and worktrees"), page));
        row->addStretch();
        layout->addLayout(row);
        project_context_ = new QLabel(QStringLiteral("Select a checkout"), page);
        project_context_->setTextInteractionFlags(Qt::TextSelectableByMouse);
        layout->addWidget(project_context_);
        projects_ = new QListWidget(page);
        layout->addWidget(projects_, 1);
        connect(projects_, &QListWidget::itemClicked, this, [this](QListWidgetItem *item) {
            active_project_ = item->data(Qt::UserRole).toString();
            updateProjectContext();
        });
        connect(projects_, &QListWidget::itemDoubleClicked, this, [this](QListWidgetItem *item) {
            const QString path = item->data(Qt::UserRole).toString();
            if (path.isEmpty()) {
                return;
            }
            auto *window = new Window(path);
            window->setAttribute(Qt::WA_DeleteOnClose);
            window->show();
        });
    }

    void makeGitPage(QVBoxLayout *layout, QWidget *page)
    {
        git_context_ = new QLabel(QStringLiteral("Select a checkout in Projects"), page);
        layout->addWidget(git_context_);
        auto *filters = new QHBoxLayout;
        git_file_ = new QLineEdit(page);
        git_file_->setPlaceholderText(QStringLiteral("Optional relative file"));
        git_staged_ = new QCheckBox(QStringLiteral("Staged"), page);
        filters->addWidget(git_file_, 1);
        filters->addWidget(git_staged_);
        layout->addLayout(filters);
        git_files_ = new QListWidget(page);
        git_files_->setMaximumHeight(130);
        git_files_->setSelectionMode(QAbstractItemView::SingleSelection);
        git_files_->setToolTip(QStringLiteral("Changed files returned by the native Git component"));
        layout->addWidget(git_files_);
        connect(git_files_, &QListWidget::itemClicked, this, [this](QListWidgetItem *item) {
            git_file_->setText(item->data(Qt::UserRole).toString());
            requestGit(QStringLiteral("diff"));
        });
        auto *hunk_row = new QHBoxLayout;
        git_hunks_ = new QListWidget(page);
        git_hunks_->setMaximumHeight(90);
        git_hunks_->setSelectionMode(QAbstractItemView::SingleSelection);
        git_hunks_->setToolTip(QStringLiteral("Select a hunk to navigate its split diff"));
        git_toggle_hunk_ = button(QStringLiteral("Collapse / expand hunk"), page);
        hunk_row->addWidget(git_hunks_, 1);
        hunk_row->addWidget(git_toggle_hunk_);
        layout->addLayout(hunk_row);
        connect(git_hunks_, &QListWidget::itemClicked, this, [this](QListWidgetItem *item) {
            const int index = git_hunks_->row(item);
            if (index < 0 || index >= git_hunk_positions_.size()) {
                return;
            }
            centerDiffAt(git_hunk_positions_.at(index));
        });
        connect(git_toggle_hunk_, &QPushButton::clicked, this, [this] { toggleSelectedHunk(); });
        auto *split = new QSplitter(Qt::Horizontal, page);
        git_left_ = new QPlainTextEdit(split);
        git_right_ = new QPlainTextEdit(split);
        git_status_ = new QPlainTextEdit(page);
        git_left_->setReadOnly(true);
        git_right_->setReadOnly(true);
        git_status_->setReadOnly(true);
        git_left_->setPlaceholderText(QStringLiteral("Old / removed lines"));
        git_right_->setPlaceholderText(QStringLiteral("New / added lines"));
        split->addWidget(git_left_);
        split->addWidget(git_right_);
        layout->addWidget(split, 1);
        layout->addWidget(git_status_);
    }

    void makeArchivePage(QVBoxLayout *layout, QWidget *page)
    {
        archive_context_ = new QLabel(QStringLiteral("Select an archive in Files, or enter its path"), page);
        archive_context_->setTextInteractionFlags(Qt::TextSelectableByMouse);
        layout->addWidget(archive_context_);
        auto *path_row = new QHBoxLayout;
        archive_path_ = new QLineEdit(page);
        archive_path_->setPlaceholderText(QStringLiteral("Archive or extracted workspace path"));
        auto *use_selected = button(QStringLiteral("Use selected"), page);
        path_row->addWidget(archive_path_, 1);
        path_row->addWidget(use_selected);
        layout->addLayout(path_row);
        auto *destination_row = new QHBoxLayout;
        archive_destination_ = new QLineEdit(page);
        archive_destination_->setPlaceholderText(QStringLiteral("Destination (optional; dialogs fill it)"));
        auto *choose_destination = button(QStringLiteral("Choose destination"), page);
        destination_row->addWidget(archive_destination_, 1);
        destination_row->addWidget(choose_destination);
        layout->addLayout(destination_row);
        archive_status_ = new QLabel(page);
        archive_status_->setWordWrap(true);
        layout->addWidget(archive_status_);
        connect(use_selected, &QPushButton::clicked, this, [this] {
            const QString path = selectedPath();
            if (!path.isEmpty()) {
                archive_path_->setText(path);
            }
        });
        connect(choose_destination, &QPushButton::clicked, this, [this] {
            const QString path = QFileDialog::getSaveFileName(this, QStringLiteral("Choose archive destination"),
                                                               directory_, QStringLiteral("Archives (*.tar *.tar.gz *.tgz *.zip *.7z *.tar.bz2 *.tar.xz *.tar.zst)"));
            if (!path.isEmpty()) {
                archive_destination_->setText(path);
            }
        });
        layout->addStretch();
    }

    void makeTasksPage(QVBoxLayout *layout, QWidget *page)
    {
        tasks_context_ = new QLabel(QStringLiteral("Select a checkout in Projects"), page);
        layout->addWidget(tasks_context_);
        tasks_ = new QListWidget(page);
        task_output_ = new QPlainTextEdit(page);
        task_output_->setReadOnly(true);
        layout->addWidget(tasks_, 1);
        layout->addWidget(task_output_);
    }

    void makeBatchPage(QVBoxLayout *layout, QWidget *page)
    {
        auto *form = new QFormLayout;
        batch_paths_ = new QLineEdit(page);
        batch_paths_->setPlaceholderText(QStringLiteral("Absolute paths, separated by ;"));
        batch_destination_ = new QLineEdit(page);
        batch_find_ = new QLineEdit(page);
        batch_replace_ = new QLineEdit(page);
        batch_prefix_ = new QLineEdit(page);
        batch_suffix_ = new QLineEdit(page);
        batch_start_ = new QSpinBox(page);
        batch_start_->setRange(1, 1000000);
        batch_start_->setValue(1);
        batch_action_ = new QComboBox(page);
        batch_action_->addItems({QStringLiteral("rename"), QStringLiteral("copy"), QStringLiteral("move")});
        form->addRow(QStringLiteral("Paths"), batch_paths_);
        form->addRow(QStringLiteral("Destination"), batch_destination_);
        form->addRow(QStringLiteral("Find"), batch_find_);
        form->addRow(QStringLiteral("Replace"), batch_replace_);
        form->addRow(QStringLiteral("Prefix"), batch_prefix_);
        form->addRow(QStringLiteral("Suffix"), batch_suffix_);
        form->addRow(QStringLiteral("Start"), batch_start_);
        form->addRow(QStringLiteral("Apply action"), batch_action_);
        layout->addLayout(form);
        auto *actions = new QHBoxLayout;
        auto *use_selected = button(QStringLiteral("Use selected files"), page);
        actions->addWidget(use_selected);
        actions->addStretch();
        layout->addLayout(actions);
        batch_preview_ = new QTableWidget(0, 3, page);
        batch_preview_->setHorizontalHeaderLabels({QStringLiteral("Apply"), QStringLiteral("From"), QStringLiteral("To")});
        batch_preview_->horizontalHeader()->setStretchLastSection(true);
        batch_preview_->setSelectionBehavior(QAbstractItemView::SelectRows);
        layout->addWidget(batch_preview_, 1);
        batch_status_ = new QLabel(page);
        layout->addWidget(batch_status_);
        connect(use_selected, &QPushButton::clicked, this, [this] {
            const QStringList paths = selectedPaths();
            batch_paths_->setText(paths.join(QStringLiteral(";")));
        });
    }

    void makeStoragePage(QVBoxLayout *layout, QWidget *page)
    {
        auto *row = new QHBoxLayout;
        storage_status_ = new QLabel(QStringLiteral("No storage map loaded"), page);
        row->addWidget(storage_status_, 1);
        layout->addLayout(row);
        storage_map_ = new StorageMapWidget(page);
        storage_map_->setHoverHandler([this](const QString &path) { highlightStoragePath(path); });
        layout->addWidget(storage_map_, 2);
        storage_entries_ = new QTreeWidget(page);
        storage_entries_->setHeaderLabels({QStringLiteral("Name"), QStringLiteral("Bytes"), QStringLiteral("Path")});
        storage_entries_->header()->setStretchLastSection(true);
        storage_entries_->setToolTip(QStringLiteral("Storage entries are scoped to the selected location; free/total describe its filesystem."));
        connect(storage_entries_, &QTreeWidget::itemDoubleClicked, this, [this](QTreeWidgetItem *item, int) {
            if (item->data(0, Qt::UserRole).toBool()) {
                navigate(item->text(2));
            }
        });
        layout->addWidget(storage_entries_, 1);
    }

    void highlightStoragePath(const QString &path)
    {
        if (path.isEmpty() || !results_) {
            return;
        }
        if (file_views_ && file_views_->currentWidget() == grid_results_) {
            for (int index = 0; index < grid_results_->count(); ++index) {
                auto *item = grid_results_->item(index);
                if (item->data(Qt::UserRole).toString() == path) {
                    results_->clearSelection();
                    grid_results_->clearSelection();
                    grid_results_->setCurrentItem(item);
                    item->setSelected(true);
                    grid_results_->scrollToItem(item);
                    return;
                }
            }
        }
        for (int index = 0; index < results_->topLevelItemCount(); ++index) {
            auto *item = results_->topLevelItem(index);
            if (item->data(0, Qt::UserRole).toString() == path) {
                results_->clearSelection();
                grid_results_->clearSelection();
                item->setSelected(true);
                results_->scrollToItem(item);
                return;
            }
        }
        for (int index = 0; index < grid_results_->count(); ++index) {
            auto *item = grid_results_->item(index);
            if (item->data(Qt::UserRole).toString() == path) {
                results_->clearSelection();
                grid_results_->clearSelection();
                grid_results_->setCurrentItem(item);
                item->setSelected(true);
                grid_results_->scrollToItem(item);
                return;
            }
        }
    }

    void requestShell()
    {
        if (!manager_ || shell_scheduled_) {
            return;
        }
        shell_scheduled_ = true;
        requestComponent(QStringLiteral("shell"), QJsonObject());
    }

    void renderShell(const QString &response)
    {
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &parse_error);
        if (parse_error.error != QJsonParseError::NoError || !document.isObject()) {
            statusBar()->showMessage(QStringLiteral("Shell registry unavailable: %1").arg(parse_error.errorString()));
            return;
        }
        shell_registry_ = document.object();
        for (const QJsonValue &value : shell_registry_.value(QStringLiteral("components")).toArray()) {
            makeComponentPage(value.toObject());
        }
        if (component_pages_.contains(QStringLiteral("projects"))) {
            QJsonObject request;
            request.insert(QStringLiteral("action"), QStringLiteral("list"));
            requestComponent(QStringLiteral("projects"), request);
        }
        if (component_pages_.contains(QStringLiteral("storage"))) {
            requestStorage();
        }
    }

    void requestComponent(const QString &component, const QJsonObject &request)
    {
        if (!manager_) {
            return;
        }
        auto *watcher = new QFutureWatcher<ComponentResult>(this);
        component_pending_.insert(watcher);
        connect(watcher, &QFutureWatcher<ComponentResult>::finished, this, [this, watcher] {
            const ComponentResult result = watcher->result();
            component_pending_.remove(watcher);
            watcher->deleteLater();
            applyComponent(result);
        });
        const auto handle = native_handle_;
        watcher->setFuture(QtConcurrent::run(&component_pool_, [handle, component, request] {
            return loadComponent(handle ? handle->manager : nullptr, component, request);
        }));
    }

    void dispatchComponentCommand(const QString &component, const QString &command)
    {
        QJsonObject request;
        if (component == QStringLiteral("projects")) {
            request.insert(QStringLiteral("action"), command == QStringLiteral("list") ? QStringLiteral("refresh") : command);
        } else if (component == QStringLiteral("git")) {
            request.insert(QStringLiteral("action"), command);
            if (!active_project_.isEmpty()) {
                request.insert(QStringLiteral("path"), active_project_);
            }
            if (git_file_ && !git_file_->text().trimmed().isEmpty()) {
                request.insert(QStringLiteral("file"), git_file_->text().trimmed());
            }
            request.insert(QStringLiteral("staged"), git_staged_ && git_staged_->isChecked());
        } else if (component == QStringLiteral("tasks")) {
            if (command == QStringLiteral("run")) {
                runSelectedTask();
                return;
            }
            request.insert(QStringLiteral("action"), command);
            if (!active_project_.isEmpty()) {
                request.insert(QStringLiteral("path"), active_project_);
            }
        } else if (component == QStringLiteral("batch")) {
            const bool mutating = command == QStringLiteral("rename") || command == QStringLiteral("copy") || command == QStringLiteral("move");
            requestBatch(command, mutating);
            return;
        } else if (component == QStringLiteral("storage")) {
            request.insert(QStringLiteral("action"), command == QStringLiteral("refresh") ? QStringLiteral("map") : command);
            if (!directory_.isEmpty()) {
                request.insert(QStringLiteral("path"), directory_);
            }
        } else if (component == QStringLiteral("archives")) {
            dispatchArchiveCommand(command);
            return;
        } else {
            request.insert(QStringLiteral("action"), command);
        }
        requestComponent(component, request);
    }

    QString archiveInputPath() const
    {
        if (archive_path_ && !archive_path_->text().trimmed().isEmpty()) {
            return archive_path_->text().trimmed();
        }
        return selectedPath();
    }

    void dispatchArchiveCommand(const QString &command)
    {
        QJsonObject request;
        request.insert(QStringLiteral("action"), command);
        if (command == QStringLiteral("compress")) {
            const QStringList selected = selectedPaths();
            if (selected.isEmpty()) {
                QMessageBox::information(this, QStringLiteral("Compress"), QStringLiteral("Select files or folders first."));
                return;
            }
            QJsonArray paths;
            for (const QString &path : selected) {
                paths.append(path);
            }
            QString destination = archive_destination_ ? archive_destination_->text().trimmed() : QString();
            if (destination.isEmpty()) {
                destination = QFileDialog::getSaveFileName(
                    this, QStringLiteral("Create archive"), QDir(directory_).filePath(QStringLiteral("archive.tar")),
                    QStringLiteral("Archives (*.tar *.tar.gz *.tgz *.zip *.7z *.tar.bz2 *.tar.xz *.tar.zst)"));
            }
            if (destination.isEmpty()) {
                return;
            }
            request.insert(QStringLiteral("paths"), paths);
            request.insert(QStringLiteral("destination"), destination);
        } else if (command == QStringLiteral("extract")) {
            const QString path = archiveInputPath();
            if (path.isEmpty()) {
                QMessageBox::information(this, QStringLiteral("Extract"), QStringLiteral("Select an archive first."));
                return;
            }
            QString destination = archive_destination_ ? archive_destination_->text().trimmed() : QString();
            if (destination.isEmpty()) {
                destination = QFileDialog::getSaveFileName(
                    this, QStringLiteral("Extract into a new folder"), QDir(directory_).filePath(QStringLiteral("extracted")),
                    QStringLiteral("New folder (*)"));
            }
            if (destination.isEmpty()) {
                return;
            }
            request.insert(QStringLiteral("path"), path);
            request.insert(QStringLiteral("destination"), destination);
        } else if (command == QStringLiteral("open")) {
            const QString path = archiveInputPath();
            if (path.isEmpty()) {
                QMessageBox::information(this, QStringLiteral("Open archive"), QStringLiteral("Select an archive first."));
                return;
            }
            request.insert(QStringLiteral("path"), path);
        } else if (command == QStringLiteral("save")) {
            const QString path = archive_workspace_.isEmpty() ? archiveInputPath() : archive_workspace_;
            if (path.isEmpty()) {
                QMessageBox::information(this, QStringLiteral("Save archive"), QStringLiteral("Open an archive workspace first."));
                return;
            }
            request.insert(QStringLiteral("path"), path);
        }
        requestComponent(QStringLiteral("archives"), request);
    }

    void applyComponent(const ComponentResult &result)
    {
        if (result.component == QStringLiteral("shell")) {
            shell_scheduled_ = false;
        }
        if (result.status != 0) {
            showError(result.component, result.status, result.error);
            return;
        }
        if (result.component == QStringLiteral("shell")) {
            renderShell(result.response);
        } else if (result.component == QStringLiteral("projects")) {
            applyProjects(result.response);
        } else if (result.component == QStringLiteral("git")) {
            applyGit(result.action, result.response);
        } else if (result.component == QStringLiteral("tasks")) {
            applyTasks(result.action, result.response);
        } else if (result.component == QStringLiteral("batch")) {
            applyBatch(result.action, result.response);
        } else if (result.component == QStringLiteral("storage")) {
            applyStorage(result.response);
        } else if (result.component == QStringLiteral("archives")) {
            applyArchives(result.action, result.response);
        }
    }

    void updateProjectContext()
    {
        const QString label = active_project_.isEmpty()
                                  ? QStringLiteral("Select a checkout in Projects")
                                  : QStringLiteral("Checkout: %1").arg(active_project_);
        if (project_context_) project_context_->setText(label);
        if (git_context_) git_context_->setText(label);
        if (tasks_context_) tasks_context_->setText(label);
        if (active_project_.isEmpty()) {
            return;
        }
        requestGit(QStringLiteral("status"));
        requestTasks(QStringLiteral("list"));
    }

    void applyProjects(const QString &response)
    {
        if (!projects_) {
            return;
        }
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &parse_error);
        if (!document.isObject()) {
            showError(QStringLiteral("Projects"), -2, parse_error.errorString());
            return;
        }
        projects_->clear();
        for (const QJsonValue &value : document.object().value(QStringLiteral("projects")).toArray()) {
            const QJsonObject project = value.toObject();
            const QString path = project.value(QStringLiteral("path")).toString();
            if (path.isEmpty()) {
                continue;
            }
            const QString branch = project.value(QStringLiteral("branch")).toString();
            const QString repository = project.value(QStringLiteral("repository")).toString();
            const QString title = repository.isEmpty() ? path : repository + QStringLiteral("  ") + branch;
            auto *item = new QListWidgetItem(title, projects_);
            item->setData(Qt::UserRole, path);
            item->setToolTip(path);
        }
        if (!active_project_.isEmpty()) {
            for (int i = 0; i < projects_->count(); ++i) {
                if (projects_->item(i)->data(Qt::UserRole).toString() == active_project_) {
                    projects_->setCurrentRow(i);
                    break;
                }
            }
        }
        if (!active_project_.isEmpty() && projects_->currentItem()) {
            updateProjectContext();
        }
    }

    void requestGit(const QString &action)
    {
        if (active_project_.isEmpty() || !component_pages_.contains(QStringLiteral("git"))) {
            return;
        }
        QJsonObject request;
        request.insert(QStringLiteral("action"), action);
        request.insert(QStringLiteral("path"), active_project_);
        if (git_file_ && !git_file_->text().trimmed().isEmpty()) {
            request.insert(QStringLiteral("file"), git_file_->text().trimmed());
        }
        request.insert(QStringLiteral("staged"), git_staged_ && git_staged_->isChecked());
        requestComponent(QStringLiteral("git"), request);
    }

    void requestGitSummary(bool force)
    {
        if (!manager_ || directory_.isEmpty() || !git_footer_) {
            return;
        }
        if (!force && git_footer_clock_.isValid() && git_footer_clock_.elapsed() < 5000) {
            return;
        }
        const QString path = directory_;
        const std::uint64_t request_generation = ++git_footer_generation_;
        git_footer_request_started_ = true;
        git_footer_clock_.restart();
        git_footer_->setText(QStringLiteral("Git: checking…"));
        QJsonObject request;
        request.insert(QStringLiteral("action"), QStringLiteral("status"));
        request.insert(QStringLiteral("path"), path);
        request.insert(QStringLiteral("staged"), false);
        auto *watcher = new QFutureWatcher<ComponentResult>(this);
        component_pending_.insert(watcher);
        connect(watcher, &QFutureWatcher<ComponentResult>::finished, this,
                [this, watcher, path, request_generation] {
                    const ComponentResult result = watcher->result();
                    component_pending_.remove(watcher);
                    watcher->deleteLater();
                    if (request_generation != git_footer_generation_ || path != directory_) {
                        return;
                    }
                    applyGitSummary(result, path);
                });
        const auto handle = native_handle_;
        watcher->setFuture(QtConcurrent::run(&component_pool_, [handle, request] {
            return loadComponent(handle ? handle->manager : nullptr, QStringLiteral("git"), request);
        }));
    }

    void applyGitSummary(const ComponentResult &result, const QString &path)
    {
        if (!git_footer_ || result.status != 0) {
            if (git_footer_) {
                git_footer_->setText(QStringLiteral("Git: unavailable"));
                git_footer_->setToolTip(result.error.isEmpty() ? QStringLiteral("Not a Git checkout") : result.error);
            }
            return;
        }
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(result.response.toUtf8(), &parse_error);
        if (parse_error.error != QJsonParseError::NoError || !document.isObject()) {
            git_footer_->setText(QStringLiteral("Git: unavailable"));
            git_footer_->setToolTip(QStringLiteral("Git status response was invalid"));
            return;
        }
        const QJsonObject object = document.object();
        QString branch = object.value(QStringLiteral("status")).toString().split(QLatin1Char('\n')).value(0).trimmed();
        if (branch.startsWith(QStringLiteral("## "))) {
            branch = branch.mid(3).trimmed();
        }
        if (branch.isEmpty()) {
            branch = QStringLiteral("detached");
        }
        const int changes = object.value(QStringLiteral("files")).toArray().size();
        const QString change_label = changes == 0
                                         ? QStringLiteral("clean")
                                         : QStringLiteral("%1 %2").arg(changes).arg(changes == 1 ? QStringLiteral("change")
                                                                                                    : QStringLiteral("changes"));
        git_footer_->setText(QStringLiteral("%1 · %2").arg(branch, change_label));
        git_footer_->setToolTip(QStringLiteral("%1 · Open Git changes").arg(path));
    }

    void applyGit(const QString &action, const QString &response)
    {
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &parse_error);
        if (!document.isObject()) {
            showError(QStringLiteral("Git"), -2, parse_error.errorString());
            return;
        }
        const QString text = document.object().value(QStringLiteral("text")).toString();
        if (git_files_) {
            const QJsonArray files = document.object().value(QStringLiteral("files")).toArray();
            const QString selected = git_file_ ? git_file_->text().trimmed() : QString();
            QSignalBlocker blocker(git_files_);
            git_files_->clear();
            int number = 1;
            for (const QJsonValue &value : files) {
                const QString file = value.toString();
                if (file.isEmpty()) {
                    continue;
                }
                auto *item = new QListWidgetItem(QStringLiteral("%1  %2").arg(number++).arg(file), git_files_);
                item->setData(Qt::UserRole, file);
                item->setToolTip(file);
                if (file == selected) {
                    git_files_->setCurrentItem(item);
                }
            }
        }
        if (action == QStringLiteral("diff")) {
            showSplitDiff(text);
        } else {
            git_status_->setPlainText(text);
            git_hunks_data_.clear();
            git_hunk_positions_.clear();
            git_collapsed_hunks_.clear();
            if (git_hunks_) {
                git_hunks_->clear();
            }
            git_left_->clear();
            git_right_->clear();
        }
        if (action == QStringLiteral("stage") || action == QStringLiteral("unstage")) {
            requestGit(QStringLiteral("status"));
        }
    }

    void showSplitDiff(const QString &text)
    {
        git_diff_prefix_.clear();
        git_hunks_data_.clear();
        git_collapsed_hunks_.clear();
        int current_hunk = -1;
        int old_number = 1;
        int new_number = 1;
        for (const QString &line : text.split(QLatin1Char('\n'))) {
            if (line.startsWith(QStringLiteral("@@"))) {
                const auto match = QRegularExpression(QStringLiteral("^@@ -(\\d+)(?:,\\d+)? \\+(\\d+)")).match(line);
                if (match.hasMatch()) {
                    old_number = match.captured(1).toInt();
                    new_number = match.captured(2).toInt();
                }
                git_hunks_data_.push_back({line, {}, {}});
                current_hunk = git_hunks_data_.size() - 1;
                continue;
            }
            if (current_hunk < 0) {
                if (!line.isEmpty()) {
                    git_diff_prefix_.push_back(line);
                }
                continue;
            }
            DiffHunk &hunk = git_hunks_data_[current_hunk];
            if (line.startsWith(QStringLiteral("+")) && !line.startsWith(QStringLiteral("+++"))) {
                hunk.new_lines << QStringLiteral("%1 | %2").arg(new_number++).arg(line.mid(1));
            } else if (line.startsWith(QStringLiteral("-")) && !line.startsWith(QStringLiteral("---"))) {
                hunk.old_lines << QStringLiteral("%1 | %2").arg(old_number++).arg(line.mid(1));
            } else {
                hunk.old_lines << QStringLiteral("%1 | %2").arg(old_number++).arg(line);
                hunk.new_lines << QStringLiteral("%1 | %2").arg(new_number++).arg(line);
            }
        }
        {
            QSignalBlocker blocker(git_hunks_);
            git_hunks_->clear();
            for (const DiffHunk &hunk : git_hunks_data_) {
                git_hunks_->addItem(hunk.header);
            }
        }
        renderSplitDiff();
    }

    void renderSplitDiff()
    {
        QStringList old_lines = git_diff_prefix_;
        QStringList new_lines = git_diff_prefix_;
        git_hunk_positions_.clear();
        for (int index = 0; index < git_hunks_data_.size(); ++index) {
            const DiffHunk &hunk = git_hunks_data_.at(index);
            git_hunk_positions_.push_back({old_lines.size(), new_lines.size()});
            old_lines << hunk.header;
            new_lines << hunk.header;
            if (git_collapsed_hunks_.contains(index)) {
                old_lines << QStringLiteral("… hunk collapsed …");
                new_lines << QStringLiteral("… hunk collapsed …");
            } else {
                old_lines.append(hunk.old_lines);
                new_lines.append(hunk.new_lines);
            }
            if (git_hunks_ && index < git_hunks_->count()) {
                git_hunks_->item(index)->setText(
                    QStringLiteral("%1 %2").arg(git_collapsed_hunks_.contains(index) ? QStringLiteral("▶") : QStringLiteral("▼"),
                                                hunk.header));
            }
        }
        if (old_lines.isEmpty() && new_lines.isEmpty()) {
            old_lines << QStringLiteral("No changes for this selection.");
            new_lines << QStringLiteral("No changes for this selection.");
        }
        git_left_->setPlainText(old_lines.join(QLatin1Char('\n')));
        git_right_->setPlainText(new_lines.join(QLatin1Char('\n')));
    }

    void centerDiffAt(const QPair<int, int> &position)
    {
        const auto center = [](QPlainTextEdit *edit, int line) {
            QTextCursor cursor = edit->textCursor();
            cursor.movePosition(QTextCursor::Start);
            cursor.movePosition(QTextCursor::Down, QTextCursor::MoveAnchor, line);
            edit->setTextCursor(cursor);
            edit->ensureCursorVisible();
        };
        center(git_left_, position.first);
        center(git_right_, position.second);
    }

    void toggleSelectedHunk()
    {
        const int index = git_hunks_ ? git_hunks_->currentRow() : -1;
        if (index < 0 || index >= git_hunks_data_.size()) {
            return;
        }
        if (git_collapsed_hunks_.contains(index)) {
            git_collapsed_hunks_.remove(index);
        } else {
            git_collapsed_hunks_.insert(index);
        }
        renderSplitDiff();
        git_hunks_->setCurrentRow(index);
    }

    void requestTasks(const QString &action)
    {
        if (active_project_.isEmpty() || !component_pages_.contains(QStringLiteral("tasks"))) {
            return;
        }
        QJsonObject request;
        request.insert(QStringLiteral("action"), action);
        request.insert(QStringLiteral("path"), active_project_);
        if (action == QStringLiteral("run") && tasks_ && tasks_->currentItem()) {
            request.insert(QStringLiteral("command"), tasks_->currentItem()->data(Qt::UserRole).toString());
        }
        requestComponent(QStringLiteral("tasks"), request);
    }

    void runSelectedTask()
    {
        if (!tasks_ || !tasks_->currentItem()) {
            return;
        }
        requestTasks(QStringLiteral("run"));
    }

    void applyTasks(const QString &action, const QString &response)
    {
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &parse_error);
        if (!document.isObject()) {
            showError(QStringLiteral("Tasks"), -2, parse_error.errorString());
            return;
        }
        const QJsonObject object = document.object();
        if (action == QStringLiteral("run")) {
            task_output_->setPlainText(object.value(QStringLiteral("text")).toString());
            return;
        }
        tasks_->clear();
        for (const QJsonValue &value : object.value(QStringLiteral("commands")).toArray()) {
            const QJsonObject command = value.toObject();
            const QString id = command.value(QStringLiteral("id")).toString();
            auto *item = new QListWidgetItem(command.value(QStringLiteral("title")).toString(id), tasks_);
            item->setData(Qt::UserRole, id);
        }
    }

    QStringList batchPaths() const
    {
        return batch_paths_->text().split(QRegularExpression(QStringLiteral("[;\\n]")), Qt::SkipEmptyParts);
    }

    QJsonObject batchRequest(const QString &action, bool selected_only)
    {
        QStringList paths;
        if (selected_only && batch_preview_) {
            for (int row = 0; row < batch_preview_->rowCount(); ++row) {
                auto *check = batch_preview_->item(row, 0);
                if (check && check->checkState() == Qt::Checked) {
                    paths.push_back(batch_preview_->item(row, 1)->text());
                }
            }
        }
        if (!selected_only) {
            paths = batchPaths();
        }
        QJsonArray json_paths;
        for (const QString &path : paths) {
            json_paths.append(path.trimmed());
        }
        QJsonObject request;
        request.insert(QStringLiteral("action"), action);
        request.insert(QStringLiteral("paths"), json_paths);
        request.insert(QStringLiteral("destination"), batch_destination_->text().trimmed());
        request.insert(QStringLiteral("find"), batch_find_->text());
        request.insert(QStringLiteral("replace"), batch_replace_->text());
        request.insert(QStringLiteral("prefix"), batch_prefix_->text());
        request.insert(QStringLiteral("suffix"), batch_suffix_->text());
        request.insert(QStringLiteral("start"), batch_start_->value());
        return request;
    }

    void requestBatch(const QString &action, bool mutating)
    {
        if (!component_pages_.contains(QStringLiteral("batch"))) {
            return;
        }
        const QJsonObject request = batchRequest(action, mutating && batch_preview_ && batch_preview_->rowCount() > 0);
        requestComponent(QStringLiteral("batch"), request);
    }

    void applyBatch(const QString &action, const QString &response)
    {
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &parse_error);
        if (!document.isObject()) {
            showError(QStringLiteral("Batch"), -2, parse_error.errorString());
            return;
        }
        const QJsonObject object = document.object();
        const QJsonArray items = object.value(QStringLiteral("items")).toArray();
        if (!items.isEmpty()) {
            batch_preview_->setRowCount(0);
            for (const QJsonValue &value : items) {
                const QJsonObject item = value.toObject();
                const int row = batch_preview_->rowCount();
                batch_preview_->insertRow(row);
                auto *check = new QTableWidgetItem;
                check->setCheckState(Qt::Checked);
                batch_preview_->setItem(row, 0, check);
                batch_preview_->setItem(row, 1, new QTableWidgetItem(item.value(QStringLiteral("from")).toString()));
                batch_preview_->setItem(row, 2, new QTableWidgetItem(item.value(QStringLiteral("to")).toString()));
            }
        }
        if (batch_status_) {
            batch_status_->setText(object.value(QStringLiteral("text")).toString().isEmpty()
                                       ? QStringLiteral("%1 batch items").arg(items.size())
                                       : object.value(QStringLiteral("text")).toString());
        }
        if (action != QStringLiteral("rename_preview") && items.isEmpty()) {
            requestBatch(QStringLiteral("rename_preview"), false);
        }
    }

    void requestStorage()
    {
        if (!component_pages_.contains(QStringLiteral("storage"))) {
            return;
        }
        QJsonObject request;
        request.insert(QStringLiteral("action"), QStringLiteral("map"));
        if (!directory_.isEmpty()) {
            request.insert(QStringLiteral("path"), directory_);
        }
        requestComponent(QStringLiteral("storage"), request);
    }

    void applyStorage(const QString &response)
    {
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &parse_error);
        if (!document.isObject()) {
            showError(QStringLiteral("Storage"), -2, parse_error.errorString());
            return;
        }
        const QJsonObject object = document.object();
        storage_entries_->clear();
        QList<StorageSlice> slices;
        for (const QJsonValue &value : object.value(QStringLiteral("entries")).toArray()) {
            const QJsonObject entry = value.toObject();
            const QString name = entry.value(QStringLiteral("name")).toString();
            const QString path = entry.value(QStringLiteral("path")).toString();
            const std::uint64_t bytes = entry.value(QStringLiteral("bytes")).toVariant().toULongLong();
            auto *item = new QTreeWidgetItem(storage_entries_);
            item->setText(0, name);
            item->setText(1, bytes ? formatBytes(bytes) : QStringLiteral("—"));
            item->setText(2, path);
            item->setData(0, Qt::UserRole, entry.value(QStringLiteral("is_dir")).toBool());
            item->setToolTip(0, path);
            item->setToolTip(1, bytes ? QStringLiteral("%1 bytes").arg(bytes) : QStringLiteral("Size is unavailable"));
            if (bytes) {
                slices.push_back({name, path, bytes, QColor()});
            }
        }
        const std::uint64_t free_bytes = object.value(QStringLiteral("free")).toVariant().toULongLong();
        const std::uint64_t total_bytes = object.value(QStringLiteral("total")).toVariant().toULongLong();
        const QString map_path = object.value(QStringLiteral("path")).toString();
        storage_map_->setData(std::move(slices), free_bytes, total_bytes);
        storage_status_->setText(QStringLiteral("Free %1 / %2 total on the filesystem containing %3; entries below cover %4")
                                      .arg(formatBytes(free_bytes))
                                      .arg(formatBytes(total_bytes))
                                      .arg(map_path.isEmpty() ? directory_ : map_path)
                                      .arg(map_path.isEmpty() ? directory_ : map_path));
    }

    void applyArchives(const QString &action, const QString &response)
    {
        QJsonParseError parse_error;
        const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &parse_error);
        if (!document.isObject()) {
            showError(QStringLiteral("Archives"), -2, parse_error.errorString());
            return;
        }
        const QJsonObject object = document.object();
        const QString path = object.value(QStringLiteral("path")).toString();
        const QString text = object.value(QStringLiteral("text")).toString();
        if (!path.isEmpty()) {
            if (action == QStringLiteral("open")) {
                archive_workspace_ = path;
                archive_path_->setText(path);
                archive_context_->setText(QStringLiteral("Archive workspace: %1").arg(path));
            } else if (action == QStringLiteral("extract")) {
                archive_destination_->setText(path);
            } else if (action == QStringLiteral("compress")) {
                archive_path_->setText(path);
            }
            if (action == QStringLiteral("open") || action == QStringLiteral("extract")) {
                navigate(path);
            }
        }
        if (archive_status_) {
            archive_status_->setText(text.isEmpty() ? QStringLiteral("Archive operation completed") : text);
        }
    }

    void requestRefresh()
    {
        if (!results_) {
            return;
        }
        if (!manager_) {
            if (!initialization_scheduled_) {
                initialization_scheduled_ = true;
                schedule(NativeAction::Refresh, initial_directory_);
            }
            return;
        }
        schedule(NativeAction::Refresh, QString());
    }

    void pollFolderSizes()
    {
        if (!manager_) {
            return;
        }
        const std::uint64_t revision = qfind_folder_sizes_revision();
        if (!folder_size_revision_seen_) {
            folder_size_revision_ = revision;
            folder_size_revision_seen_ = true;
            return;
        }
        if (revision == folder_size_revision_) {
            return;
        }
        folder_size_revision_ = revision;
        pending_selection_paths_ = selectedPaths();
        requestRefresh();
        requestStorage();
    }

    void schedule(NativeAction action, const QString &path)
    {
        const auto generation = ++generation_;
        const unsigned scope = scope_->currentData().toUInt();
        const bool global = scope == 2;
        const bool recursive = scope != 0;
        const unsigned sort = sort_->currentData().toUInt();
        const QString query = action == NativeAction::Navigate ? QString() : search_->text().trimmed();
        auto *watcher = new QFutureWatcher<NativeResult>(this);
        pending_.insert(watcher);
        connect(watcher, &QFutureWatcher<NativeResult>::finished, this, [this, watcher, generation] {
            const NativeResult result = watcher->result();
            pending_.remove(watcher);
            watcher->deleteLater();
            if (initialization_scheduled_) {
                initialization_scheduled_ = false;
                native_handle_ = result.handle;
                manager_ = result.manager;
                if (result.status != 0) {
                    showError(result.operation, result.status, result.error);
                } else {
                    requestShell();
                    requestRefresh();
                }
                return;
            }
            if (generation != generation_) {
                return;
            }
            apply(result);
        });
        const auto handle = native_handle_;
        const auto native_mutex = native_mutex_;
        watcher->setFuture(QtConcurrent::run(&native_pool_, [handle, native_mutex, action, path, global, recursive, sort, query] {
            if (!handle) {
                return initializeNative(*native_mutex, path);
            }
            return loadNative(handle->manager, *native_mutex, action, path, global, recursive, sort, query);
        }));
    }

    void apply(const NativeResult &result)
    {
        if (thumbnail_generation_ != generation_) {
            thumbnail_requests_.clear();
            thumbnail_generation_ = generation_;
        }
        if (result.status != 0) {
            showError(result.operation, result.status, result.error);
            return;
        }
        if (!result.directory.isEmpty()) {
            directory_ = result.directory;
            location_->setText(directory_);
            rebindDirectoryWatcher();
        }
        requestGitSummary(result.action != NativeAction::Refresh || !git_footer_request_started_);
        if (result.action == NativeAction::Navigate) {
            QSignalBlocker blocker(search_);
            search_->clear();
            requestStorage();
        }

        results_->setUpdatesEnabled(false);
        results_->clear();
        grid_results_->setUpdatesEnabled(false);
        grid_results_->clear();
        const QFileIconProvider icons;
        for (const Row &row : result.rows) {
            auto *item = new QTreeWidgetItem(results_);
            const QIcon icon = icons.icon(row.directory ? QFileIconProvider::Folder : QFileIconProvider::File);
            item->setIcon(0, icon);
            item->setText(0, row.name);
            item->setText(1, row.directory ? QStringLiteral("Folder") : QStringLiteral("File"));
            item->setText(2, row.bytes ? formatBytes(row.bytes) : QStringLiteral("—"));
            item->setText(3, row.path);
            item->setData(0, Qt::UserRole, row.path);
            item->setData(0, Qt::UserRole + 1, row.directory);
            auto *grid_item = new QListWidgetItem(icon, row.name, grid_results_);
            grid_item->setToolTip(row.path);
            grid_item->setData(Qt::UserRole, row.path);
            grid_item->setData(Qt::UserRole + 1, row.directory);
        }
        results_->setUpdatesEnabled(true);
        grid_results_->setUpdatesEnabled(true);
        restoreSelection();
        statusBar()->showMessage(QStringLiteral("%1 items").arg(result.rows.size()));
        showSelection();
        if (file_views_ && file_views_->currentWidget() == grid_results_) {
            thumbnail_debounce_.start();
        }
    }

    void restoreSelection()
    {
        if (pending_selection_paths_.isEmpty()) {
            return;
        }
        QSet<QString> paths;
        for (const QString &path : pending_selection_paths_) {
            paths.insert(path);
        }
        if (file_views_ && file_views_->currentWidget() == grid_results_) {
            QSignalBlocker blocker(grid_results_);
            for (int index = 0; index < grid_results_->count(); ++index) {
                auto *item = grid_results_->item(index);
                item->setSelected(paths.contains(item->data(Qt::UserRole).toString()));
            }
        } else {
            QSignalBlocker blocker(results_);
            for (int index = 0; index < results_->topLevelItemCount(); ++index) {
                auto *item = results_->topLevelItem(index);
                item->setSelected(paths.contains(item->data(0, Qt::UserRole).toString()));
            }
        }
        pending_selection_paths_.clear();
    }

    void rebindDirectoryWatcher()
    {
        if (!directory_watcher_) {
            return;
        }
        const QStringList watched = directory_watcher_->directories();
        if (!watched.isEmpty()) {
            directory_watcher_->removePaths(watched);
        }
        if (!directory_.isEmpty()) {
            directory_watcher_->addPath(directory_);
        }
    }

    void navigate(const QString &path)
    {
        if (!manager_ || path.trimmed().isEmpty()) {
            return;
        }
        schedule(NativeAction::Navigate, QDir::cleanPath(path.trimmed()));
    }

    void moveHistory(bool forward)
    {
        if (!manager_) {
            return;
        }
        schedule(forward ? NativeAction::Forward : NativeAction::Back, QString());
    }

    void activate(QTreeWidgetItem *item)
    {
        const QString path = item->data(0, Qt::UserRole).toString();
        if (item->data(0, Qt::UserRole + 1).toBool()) {
            navigate(path);
        } else {
            QDesktopServices::openUrl(QUrl::fromLocalFile(path));
        }
    }

    void activateGrid(QListWidgetItem *item)
    {
        const QString path = item->data(Qt::UserRole).toString();
        if (item->data(Qt::UserRole + 1).toBool()) {
            navigate(path);
        } else {
            QDesktopServices::openUrl(QUrl::fromLocalFile(path));
        }
    }

    QString selectedPath() const
    {
        const QStringList paths = selectedPaths();
        return paths.isEmpty() ? QString() : paths.front();
    }

    QStringList selectedPaths() const
    {
        QStringList paths;
        if (file_views_ && file_views_->currentWidget() == grid_results_) {
            for (auto *item : grid_results_->selectedItems()) {
                paths.push_back(item->data(Qt::UserRole).toString());
            }
        } else {
            for (auto *item : results_->selectedItems()) {
                paths.push_back(item->data(0, Qt::UserRole).toString());
            }
        }
        paths.removeAll(QString());
        return paths;
    }

    void openSelected()
    {
        const QString path = selectedPath();
        if (!path.isEmpty()) {
            QDesktopServices::openUrl(QUrl::fromLocalFile(path));
        }
    }

    void revealSelected()
    {
        const QString path = selectedPath();
        if (path.isEmpty()) {
            return;
        }
        const QFileInfo info(path);
        QDesktopServices::openUrl(QUrl::fromLocalFile(info.isDir() ? info.absoluteFilePath() : info.absolutePath()));
    }

    void copySelected()
    {
        const QStringList paths = selectedPaths();
        if (paths.isEmpty()) {
            return;
        }
        const QString destination = QFileDialog::getExistingDirectory(this, QStringLiteral("Copy files to"), directory_);
        if (destination.isEmpty()) {
            return;
        }
        requestBatchTransfer(paths, Qt::CopyAction, destination);
    }

    void importDropped(const QStringList &paths, Qt::DropAction action)
    {
        requestBatchTransfer(paths, action, directory_);
    }

    void requestBatchTransfer(const QStringList &paths, Qt::DropAction action, const QString &destination)
    {
        if (paths.isEmpty() || destination.trimmed().isEmpty()) {
            return;
        }
        QJsonArray json_paths;
        for (const QString &path : paths) {
            json_paths.append(QDir::cleanPath(path));
        }
        QJsonObject request;
        request.insert(QStringLiteral("action"), action == Qt::MoveAction ? QStringLiteral("move") : QStringLiteral("copy"));
        request.insert(QStringLiteral("paths"), json_paths);
        request.insert(QStringLiteral("destination"), QDir::cleanPath(destination.trimmed()));
        requestComponent(QStringLiteral("batch"), request);
    }

    void renameSelected()
    {
        const QStringList paths = selectedPaths();
        if (paths.size() != 1) {
            QMessageBox::information(this, QStringLiteral("Rename"), QStringLiteral("Select one file or folder to rename."));
            return;
        }
        const QString path = paths.front();
        const QFileInfo info(path);
        bool accepted = false;
        const QString name = QInputDialog::getText(this, QStringLiteral("Rename"), QStringLiteral("Name"),
                                                   QLineEdit::Normal, info.fileName(), &accepted).trimmed();
        if (!accepted || name.isEmpty() || name == info.fileName()) {
            return;
        }
        if (name == QStringLiteral(".") || name == QStringLiteral("..") ||
            name.contains(QLatin1Char('/')) || name.contains(QLatin1Char('\\')) ||
            name.contains(QChar::Null)) {
            QMessageBox::warning(this, QStringLiteral("Rename"), QStringLiteral("Enter a single valid file name."));
            return;
        }
        const QString destination = info.dir().filePath(name);
        scheduleFileOperation([path, destination] {
            FileResult result;
            result.operation = QStringLiteral("Rename");
            if (QFile::rename(path, destination)) {
                result.completed = 1;
            } else {
                result.failures.push_back(path);
            }
            return result;
        });
    }

    void trashSelected()
    {
        const QStringList paths = selectedPaths();
        if (paths.isEmpty()) {
            return;
        }
        scheduleFileOperation([paths] {
            FileResult result;
            result.operation = QStringLiteral("Move to Trash");
            for (const QString &path : paths) {
                if (QFile::moveToTrash(path)) {
                    ++result.completed;
                } else {
                    result.failures.push_back(path);
                }
            }
            return result;
        });
    }

    void scheduleFileOperation(std::function<FileResult()> operation)
    {
        auto *watcher = new QFutureWatcher<FileResult>(this);
        file_pending_.insert(watcher);
        connect(watcher, &QFutureWatcher<FileResult>::finished, this, [this, watcher] {
            const FileResult result = watcher->result();
            file_pending_.remove(watcher);
            watcher->deleteLater();
            if (!result.failures.isEmpty()) {
                QMessageBox::warning(this, result.operation,
                                     QStringLiteral("Completed %1. Failed:\n%2")
                                         .arg(result.completed)
                                         .arg(result.failures.join(QStringLiteral("\n"))));
            } else {
                statusBar()->showMessage(QStringLiteral("%1 completed (%2)").arg(result.operation).arg(result.completed));
            }
            requestRefresh();
        });
        watcher->setFuture(QtConcurrent::run(&file_pool_, [operation = std::move(operation)] {
            return operation();
        }));
    }

    void scheduleGridThumbnails()
    {
        if (!grid_results_ || !file_views_ || file_views_->currentWidget() != grid_results_) {
            return;
        }
        constexpr int max_thumbnails = 48;
        int scheduled = 0;
        for (int index = 0; index < grid_results_->count() && scheduled < max_thumbnails; ++index) {
            auto *item = grid_results_->item(index);
            if (!item || item->data(Qt::UserRole + 2).toBool()) {
                continue;
            }
            if (!grid_results_->visualItemRect(item).intersects(grid_results_->viewport()->rect())) {
                continue;
            }
            const QString path = item->data(Qt::UserRole).toString();
            if (!looksLikeImage(path)) {
                continue;
            }
            const std::uint64_t generation = generation_;
            if (thumbnail_requests_.value(path, 0) == generation) {
                continue;
            }
            thumbnail_requests_.insert(path, generation);
            ++scheduled;
            auto *watcher = new QFutureWatcher<ThumbnailResult>(this);
            thumbnail_pending_.insert(watcher);
            connect(watcher, &QFutureWatcher<ThumbnailResult>::finished, this, [this, watcher, generation] {
                const ThumbnailResult result = watcher->result();
                thumbnail_pending_.remove(watcher);
                watcher->deleteLater();
                if (thumbnail_requests_.value(result.path, 0) != generation || generation != generation_ ||
                    result.image.isNull()) {
                    return;
                }
                for (int index = 0; index < grid_results_->count(); ++index) {
                    auto *item = grid_results_->item(index);
                    if (item->data(Qt::UserRole).toString() == result.path) {
                        item->setIcon(QIcon(QPixmap::fromImage(result.image)));
                        item->setData(Qt::UserRole + 2, true);
                        break;
                    }
                }
            });
            watcher->setFuture(QtConcurrent::run(&thumbnail_pool_, [path] { return loadThumbnail(path); }));
        }
    }

    void showSelection()
    {
        if (!preview_title_ || !preview_text_) {
            return;
        }
        const QString path = selectedPath();
        if (path.isEmpty()) {
            preview_title_->setText(QStringLiteral("Select a file to preview it."));
            preview_text_->clear();
            return;
        }
        preview_title_->setText(QStringLiteral("Loading preview…"));
        preview_text_->clear();
        auto *watcher = new QFutureWatcher<PreviewResult>(this);
        preview_pending_.insert(watcher);
        connect(watcher, &QFutureWatcher<PreviewResult>::finished, this, [this, watcher] {
            const PreviewResult result = watcher->result();
            preview_pending_.remove(watcher);
            watcher->deleteLater();
            if (result.path != selectedPath()) {
                return;
            }
            preview_title_->setText(result.title);
            preview_text_->setPlainText(result.text);
        });
        watcher->setFuture(QtConcurrent::run(&preview_pool_, [path] { return loadPreview(path); }));
    }

    void showError(const QString &operation, int status, const QString &error = QString())
    {
        const QString detail = error.isEmpty() ? nativeError(manager_, status) : error;
        statusBar()->showMessage(operation + QStringLiteral(": ") + detail);
        QMessageBox::warning(this, operation, detail);
    }

    void saveSettings() const
    {
        if (!results_ || !file_splitter_) {
            return;
        }
        QSettings settings;
        settings.setValue(QStringLiteral("files/headerState"), results_->header()->saveState());
        settings.setValue(QStringLiteral("files/splitterState"), file_splitter_->saveState());
        settings.setValue(QStringLiteral("files/gridView"), grid_view_action_ && grid_view_action_->isChecked());
    }

    QfindManager *manager_ = nullptr;
    std::shared_ptr<NativeHandle> native_handle_;
    QString initial_directory_;
    QString directory_;
    QListWidget *places_ = nullptr;
    FileTree *results_ = nullptr;
    FileGrid *grid_results_ = nullptr;
    QWidget *browser_page_ = nullptr;
    QSplitter *file_splitter_ = nullptr;
    QStackedWidget *file_views_ = nullptr;
    QTabWidget *workspaces_ = nullptr;
    QWidget *preview_pane_ = nullptr;
    QLabel *preview_title_ = nullptr;
    QPlainTextEdit *preview_text_ = nullptr;
    QLineEdit *search_ = nullptr;
    QLineEdit *location_ = nullptr;
    QComboBox *scope_ = nullptr;
    QComboBox *sort_ = nullptr;
    QTimer debounce_;
    QTimer folder_size_timer_;
    QTimer directory_change_debounce_;
    QTimer thumbnail_debounce_;
    QFileSystemWatcher *directory_watcher_ = nullptr;
    std::uint64_t folder_size_revision_ = 0;
    bool folder_size_revision_seen_ = false;
    QStringList pending_selection_paths_;
    QAction *grid_view_action_ = nullptr;
    std::shared_ptr<std::mutex> native_mutex_ = std::make_shared<std::mutex>();
    QThreadPool &native_pool_;
    QThreadPool &component_pool_;
    QThreadPool &file_pool_;
    QThreadPool &preview_pool_;
    QThreadPool &thumbnail_pool_;
    std::uint64_t generation_ = 0;
    bool initialization_scheduled_ = false;
    QSet<QFutureWatcher<NativeResult> *> pending_;
    QSet<QFutureWatcher<FileResult> *> file_pending_;
    QSet<QFutureWatcher<ComponentResult> *> component_pending_;
    QSet<QFutureWatcher<PreviewResult> *> preview_pending_;
    QSet<QFutureWatcher<ThumbnailResult> *> thumbnail_pending_;
    QHash<QString, std::uint64_t> thumbnail_requests_;
    std::uint64_t thumbnail_generation_ = 0;
    QHash<QString, QWidget *> component_pages_;
    QHash<QString, QHBoxLayout *> component_commands_;
    QJsonObject shell_registry_;
    bool shell_scheduled_ = false;
    QListWidget *projects_ = nullptr;
    QLabel *project_context_ = nullptr;
    QString active_project_;
    QLineEdit *git_file_ = nullptr;
    QLabel *git_context_ = nullptr;
    QCheckBox *git_staged_ = nullptr;
    QPlainTextEdit *git_left_ = nullptr;
    QPlainTextEdit *git_right_ = nullptr;
    QPlainTextEdit *git_status_ = nullptr;
    QListWidget *git_files_ = nullptr;
    QListWidget *git_hunks_ = nullptr;
    QPushButton *git_toggle_hunk_ = nullptr;
    QStringList git_diff_prefix_;
    QList<DiffHunk> git_hunks_data_;
    QList<QPair<int, int>> git_hunk_positions_;
    QSet<int> git_collapsed_hunks_;
    QListWidget *tasks_ = nullptr;
    QLabel *tasks_context_ = nullptr;
    QPlainTextEdit *task_output_ = nullptr;
    QLineEdit *batch_paths_ = nullptr;
    QLineEdit *batch_destination_ = nullptr;
    QLineEdit *batch_find_ = nullptr;
    QLineEdit *batch_replace_ = nullptr;
    QLineEdit *batch_prefix_ = nullptr;
    QLineEdit *batch_suffix_ = nullptr;
    QSpinBox *batch_start_ = nullptr;
    QComboBox *batch_action_ = nullptr;
    QTableWidget *batch_preview_ = nullptr;
    QLabel *batch_status_ = nullptr;
    QTreeWidget *storage_entries_ = nullptr;
    QLabel *storage_status_ = nullptr;
    StorageMapWidget *storage_map_ = nullptr;
    QLabel *archive_context_ = nullptr;
    QLineEdit *archive_path_ = nullptr;
    QLineEdit *archive_destination_ = nullptr;
    QLabel *archive_status_ = nullptr;
    QString archive_workspace_;
    QLabel *git_footer_ = nullptr;
    QElapsedTimer git_footer_clock_;
    std::uint64_t git_footer_generation_ = 0;
    bool git_footer_request_started_ = false;
};

} // namespace

QString initialDirectory(int argc, char **argv)
{
    QString path = qEnvironmentVariable("QFIND_ROOT");
    for (int i = 1; i + 1 < argc; ++i) {
        if (QString::fromLocal8Bit(argv[i]) == QStringLiteral("--here")) {
            path = QString::fromLocal8Bit(argv[i + 1]);
            break;
        }
    }
    return !path.isEmpty() && QFileInfo(path).isDir() ? QDir(path).absolutePath() : QDir::homePath();
}

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    app.setApplicationName(QStringLiteral("Megaman"));
    app.setOrganizationName(QStringLiteral("qfind"));
    Window window(initialDirectory(argc, argv));
    window.show();
    return app.exec();
}
