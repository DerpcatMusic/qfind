// Thin Qt/Breeze adapter: Query via `qfind --json`. Catalog stays in Rust.
// Build: cmake -S . -B build && cmake --build build
// Looks native on KDE because it is a Qt Widgets app (Breeze).

#include <QApplication>
#include <QDrag>
#include <QKeyEvent>
#include <QLineEdit>
#include <QListWidget>
#include <QMimeData>
#include <QProcess>
#include <QUrl>
#include <QVBoxLayout>
#include <QWidget>
#include <QDesktopServices>
#include <QStatusBar>
#include <QMainWindow>
#include <QTimer>
#include <QJsonDocument>
#include <QJsonObject>

class HitList : public QListWidget {
public:
    using QListWidget::QListWidget;

protected:
    QMimeData *mimeData(const QList<QListWidgetItem *> &items) const override {
        auto *m = new QMimeData;
        QList<QUrl> urls;
        for (QListWidgetItem *item : items) {
            const QString path = item->data(Qt::UserRole).toString();
            if (!path.isEmpty()) {
                urls << QUrl::fromLocalFile(path);
            }
        }
        m->setUrls(urls);
        return m;
    }
};

class Window : public QMainWindow {
public:
    Window() {
        auto *central = new QWidget(this);
        auto *layout = new QVBoxLayout(central);
        search_ = new QLineEdit(this);
        search_->setPlaceholderText(QStringLiteral("Fuzzy search…  Space previews"));
        list_ = new HitList(this);
        list_->setDragEnabled(true);
        list_->setDefaultDropAction(Qt::CopyAction);
        layout->addWidget(search_);
        layout->addWidget(list_);
        setCentralWidget(central);
        statusBar()->showMessage(QStringLiteral("Qfind Qt — Breeze / KDE"));
        setWindowTitle(QStringLiteral("Qfind"));
        resize(900, 600);

        debounce_.setSingleShot(true);
        debounce_.setInterval(50);
        connect(search_, &QLineEdit::textChanged, this, [this] { debounce_.start(); });
        connect(&debounce_, &QTimer::timeout, this, &Window::runQuery);
        connect(list_, &QListWidget::itemActivated, this, [](QListWidgetItem *item) {
            QDesktopServices::openUrl(QUrl::fromLocalFile(item->data(Qt::UserRole).toString()));
        });
    }

protected:
    void keyPressEvent(QKeyEvent *e) override {
        if (e->key() == Qt::Key_Space && list_->hasFocus()) {
            if (auto *item = list_->currentItem()) {
                QDesktopServices::openUrl(QUrl::fromLocalFile(item->data(Qt::UserRole).toString()));
            }
            return;
        }
        if (e->key() == Qt::Key_Escape) {
            search_->clear();
            search_->setFocus();
            return;
        }
        QMainWindow::keyPressEvent(e);
    }

private:
    void runQuery() {
        const QString q = search_->text().trimmed();
        list_->clear();
        if (q.isEmpty()) {
            return;
        }
        QProcess p;
        p.start(QStringLiteral("qfind"), {QStringLiteral("--json"), QStringLiteral("--limit"),
                                          QStringLiteral("80"), q});
        if (!p.waitForFinished(4000)) {
            return;
        }
        const QList<QByteArray> lines = p.readAllStandardOutput().split('\n');
        for (const QByteArray &line : lines) {
            if (line.trimmed().isEmpty()) {
                continue;
            }
            const QJsonObject o = QJsonDocument::fromJson(line).object();
            const QString name = o.value(QStringLiteral("name")).toString();
            const QString path = o.value(QStringLiteral("path")).toString();
            auto *item = new QListWidgetItem(name + QStringLiteral("  —  ") + path);
            item->setData(Qt::UserRole, path);
            list_->addItem(item);
        }
        statusBar()->showMessage(QStringLiteral("%1 Hits").arg(list_->count()));
    }

    QLineEdit *search_{};
    HitList *list_{};
    QTimer debounce_;
};

int main(int argc, char **argv) {
    QApplication app(argc, argv);
    app.setApplicationName(QStringLiteral("Qfind"));
    app.setOrganizationName(QStringLiteral("qfind"));
    Window w;
    w.show();
    return app.exec();
}
