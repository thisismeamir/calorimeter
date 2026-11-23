/* Header-only C/C++ wrapper for the Calorimeter CALO_CHANNEL protocol (spec §9.4). */
#ifndef CALO_CHANNEL_H
#define CALO_CHANNEL_H

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

static inline int calo_channel_fd(void) {
    static int cached_fd = -2;
    if (cached_fd == -2) {
        const char *path = getenv("CALO_CHANNEL");
        if (path != NULL && path[0] != '\0') {
            cached_fd = open(path, O_WRONLY | O_CLOEXEC);
        } else {
            cached_fd = -1;
        }
    }
    return cached_fd;
}

static inline long calo_gettid(void) {
#ifdef SYS_gettid
    return syscall(SYS_gettid);
#else
    return 1L;
#endif
}

static inline void calo_write_line(const char *buf, int len) {
    int fd = calo_channel_fd();
    if (fd >= 0 && len > 0) {
        ssize_t ignored = write(fd, buf, (size_t)len);
        (void)ignored;
    }
}

static inline void calo_phase_begin(const char *name) {
    if (calo_channel_fd() < 0) return;
    char buf[512];
    int n = snprintf(buf, sizeof(buf), "phase %s start\n", name);
    calo_write_line(buf, n);
}

static inline void calo_phase_end(const char *name) {
    if (calo_channel_fd() < 0) return;
    char buf[512];
    int n = snprintf(buf, sizeof(buf), "phase %s end\n", name);
    calo_write_line(buf, n);
}

static inline void calo_region_begin(const char *name) {
    if (calo_channel_fd() < 0) return;
    char buf[512];
    int n = snprintf(buf, sizeof(buf), "tid=%ld region %s start\n", calo_gettid(), name);
    calo_write_line(buf, n);
}

static inline void calo_region_end(const char *name) {
    if (calo_channel_fd() < 0) return;
    char buf[512];
    int n = snprintf(buf, sizeof(buf), "tid=%ld region %s end\n", calo_gettid(), name);
    calo_write_line(buf, n);
}

static inline void calo_metric(const char *key, double value, const char *unit) {
    if (calo_channel_fd() < 0) return;
    char buf[512];
    int n;
    if (unit != NULL && unit[0] != '\0') {
        n = snprintf(buf, sizeof(buf), "metric %s=%g %s\n", key, value, unit);
    } else {
        n = snprintf(buf, sizeof(buf), "metric %s=%g\n", key, value);
    }
    calo_write_line(buf, n);
}

static inline void calo_tag(const char *key, const char *value) {
    if (calo_channel_fd() < 0) return;
    char buf[512];
    int n = snprintf(buf, sizeof(buf), "tag %s=%s\n", key, value);
    calo_write_line(buf, n);
}

#define CALO_PHASE_BEGIN(name) calo_phase_begin(name)
#define CALO_PHASE_END(name)   calo_phase_end(name)
#define CALO_REGION_BEGIN(name) calo_region_begin(name)
#define CALO_REGION_END(name)   calo_region_end(name)

#ifdef __cplusplus
namespace calo {
class Region {
    const char *name_;
public:
    explicit Region(const char *name) : name_(name) { calo_region_begin(name_); }
    ~Region() { calo_region_end(name_); }
    Region(const Region &) = delete;
    Region &operator=(const Region &) = delete;
};

class Phase {
    const char *name_;
public:
    explicit Phase(const char *name) : name_(name) { calo_phase_begin(name_); }
    ~Phase() { calo_phase_end(name_); }
    Phase(const Phase &) = delete;
    Phase &operator=(const Phase &) = delete;
};
} // namespace calo
#endif

#endif /* CALO_CHANNEL_H */
