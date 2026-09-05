#ifndef QFIND_NATIVE_H
#define QFIND_NATIVE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct QfindManager QfindManager;

typedef struct QfindRow {
    const char *name;
    const char *path;
    uint64_t bytes;
    uint64_t entries;
    uint32_t id;
    uint8_t is_dir;
} QfindRow;

typedef void (*QfindRowCallback)(void *context, const QfindRow *row);
typedef void (*QfindTextCallback)(void *context, const char *text);

/* UTF-8 strings; callback data is borrowed only for the duration of callback.
 * Handles must not be freed during calls. Calls may block: use a worker queue.
 * Callbacks must not throw or reenter this handle. Status: 0 success,
 * -1 invalid argument, -2 operation failure (read error), -3 unwind panic.
 * Existing entry points and QfindRow layout remain ABI-compatible.
 */
int32_t qfind_manager_search_scope(QfindManager *manager, uint8_t global);
/* 0 relevance, 1 name, 2 name descending, 3 newest, 4 oldest, 5 largest, 6 smallest */
int32_t qfind_manager_sort(QfindManager *manager, uint32_t sort);
int32_t qfind_manager_error(QfindManager *manager, QfindTextCallback callback, void *context);

/* Shared component JSON request (maximum 4 MiB), response borrowed by callback.
 * Component IDs: shell, projects, git, tasks, storage, batch, archives.
 * Callback receives JSON on success, plain UTF-8 error text on operation failure.
 * Prefer this per-call error to qfind_manager_error during concurrent operations. */
int32_t qfind_manager_component(QfindManager *manager, const char *component, const char *request_json, QfindTextCallback callback, void *context);

uint64_t qfind_folder_sizes_revision(void);

QfindManager *qfind_manager_open(const char *initial_directory);
void qfind_manager_free(QfindManager *manager);
int32_t qfind_manager_navigate(QfindManager *manager, const char *path);
int32_t qfind_manager_back(QfindManager *manager);
int32_t qfind_manager_forward(QfindManager *manager);
int32_t qfind_manager_directory(QfindManager *manager, QfindTextCallback callback, void *context);
int32_t qfind_manager_rows(
    QfindManager *manager,
    const char *query,
    uint8_t recursive,
    uint32_t limit,
    QfindRowCallback callback,
    void *context
);
int32_t qfind_manager_chart(
    QfindManager *manager,
    uint8_t global,
    uint32_t limit,
    QfindRowCallback callback,
    void *context
);

#ifdef __cplusplus
}
#endif

#endif
