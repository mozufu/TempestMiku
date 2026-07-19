#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static unsigned long long parse_decimal(const char *text, const char *label) {
    if (text == NULL || *text == '\0') {
        fprintf(stderr, "%s must be a positive decimal integer\n", label);
        exit(2);
    }
    errno = 0;
    char *end = NULL;
    unsigned long long value = strtoull(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || value == 0) {
        fprintf(stderr, "%s must be a positive decimal integer\n", label);
        exit(2);
    }
    return value;
}

static void write_ready(const char *path) {
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd < 0) {
        perror("open ready marker");
        exit(1);
    }
    static const char ready[] = "ready\n";
    if (write(fd, ready, sizeof(ready) - 1) != (ssize_t)(sizeof(ready) - 1)
        || close(fd) != 0) {
        perror("write ready marker");
        exit(1);
    }
}

static void sleep_ms(unsigned long long milliseconds) {
    struct timespec remaining = {
        .tv_sec = (time_t)(milliseconds / 1000),
        .tv_nsec = (long)((milliseconds % 1000) * 1000000ULL),
    };
    while (nanosleep(&remaining, &remaining) != 0) {
        if (errno != EINTR) {
            perror("nanosleep");
            exit(1);
        }
    }
}

static int memory_probe(const char *bytes_text, const char *ready, const char *hold_text) {
    unsigned long long requested = parse_decimal(bytes_text, "memory bytes");
    unsigned long long hold_ms = parse_decimal(hold_text, "hold milliseconds");
    if (requested > SIZE_MAX) {
        fprintf(stderr, "memory bytes exceed size_t\n");
        return 2;
    }
    unsigned char *memory = malloc((size_t)requested);
    if (memory == NULL) {
        perror("malloc");
        return 1;
    }
    long page_size = sysconf(_SC_PAGESIZE);
    if (page_size <= 0) {
        fprintf(stderr, "could not determine page size\n");
        free(memory);
        return 1;
    }
    for (size_t offset = 0; offset < (size_t)requested; offset += (size_t)page_size) {
        memory[offset] = (unsigned char)(offset / (size_t)page_size);
    }
    memory[(size_t)requested - 1] = 1;
    write_ready(ready);
    sleep_ms(hold_ms);
    volatile unsigned char observed = memory[(size_t)requested - 1];
    free(memory);
    return observed == 1 ? 0 : 1;
}

static int pids_probe(const char *count_text, const char *ready, const char *hold_text) {
    unsigned long long requested = parse_decimal(count_text, "child count");
    unsigned long long hold_ms = parse_decimal(hold_text, "hold milliseconds");
    if (requested > 1024) {
        fprintf(stderr, "child count exceeds probe safety bound\n");
        return 2;
    }
    pid_t *children = calloc((size_t)requested, sizeof(*children));
    if (children == NULL) {
        perror("calloc");
        return 1;
    }
    size_t started = 0;
    for (; started < (size_t)requested; ++started) {
        pid_t child = fork();
        if (child < 0) {
            perror("fork");
            break;
        }
        if (child == 0) {
            sleep_ms(hold_ms + 2000);
            _exit(0);
        }
        children[started] = child;
    }
    if (started != (size_t)requested) {
        for (size_t index = 0; index < started; ++index) {
            kill(children[index], SIGKILL);
        }
    } else {
        write_ready(ready);
        sleep_ms(hold_ms);
        for (size_t index = 0; index < started; ++index) {
            kill(children[index], SIGTERM);
        }
    }
    int result = started == (size_t)requested ? 0 : 1;
    for (size_t index = 0; index < started; ++index) {
        int status = 0;
        if (waitpid(children[index], &status, 0) != children[index]
            || !WIFSIGNALED(status)) {
            result = 1;
        }
    }
    free(children);
    return result;
}

static unsigned long long monotonic_ns(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        perror("clock_gettime");
        exit(1);
    }
    return (unsigned long long)now.tv_sec * 1000000000ULL
        + (unsigned long long)now.tv_nsec;
}

static int cpu_probe(const char *hold_text, const char *ready) {
    unsigned long long hold_ms = parse_decimal(hold_text, "hold milliseconds");
    write_ready(ready);
    unsigned long long deadline = monotonic_ns() + hold_ms * 1000000ULL;
    volatile uint64_t state = 0x9e3779b97f4a7c15ULL;
    while (monotonic_ns() < deadline) {
        state ^= state << 7;
        state ^= state >> 9;
        state *= 0xbf58476d1ce4e5b9ULL;
    }
    return state == 0 ? 1 : 0;
}

int main(int argc, char **argv) {
    if (argc != 5 || strcmp(argv[1], "memory") != 0) {
        if (argc == 5 && strcmp(argv[1], "pids") == 0) {
            return pids_probe(argv[2], argv[3], argv[4]);
        }
        if (argc == 4 && strcmp(argv[1], "cpu") == 0) {
            return cpu_probe(argv[2], argv[3]);
        }
        fprintf(stderr,
            "usage: resource-probe memory BYTES READY HOLD_MS | "
            "pids COUNT READY HOLD_MS | cpu HOLD_MS READY\n");
        return 2;
    }
    return memory_probe(argv[2], argv[3], argv[4]);
}
