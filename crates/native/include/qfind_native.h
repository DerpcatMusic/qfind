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
