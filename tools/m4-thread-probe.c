#include <pthread.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static void *thread_main(void *unused) {
    (void)unused;
    static const char output[] = "thread-ok\n";
    return write(STDOUT_FILENO, output, sizeof(output) - 1) == (ssize_t)(sizeof(output) - 1)
        ? NULL
        : (void *)1;
}

int main(void) {
    pthread_t thread;
    void *thread_result = NULL;
    if (pthread_create(&thread, NULL, thread_main, NULL) != 0
        || pthread_join(thread, &thread_result) != 0
        || thread_result != NULL) {
        return 1;
    }
    pid_t child = fork();
    if (child < 0) {
        return 1;
    }
    if (child == 0) {
        static const char output[] = "child-ok\n";
        _exit(write(STDOUT_FILENO, output, sizeof(output) - 1)
            == (ssize_t)(sizeof(output) - 1) ? 0 : 1);
    }
    int status = 0;
    return waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0
        ? 0
        : 1;
}
