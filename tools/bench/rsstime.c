/* rsstime OUT.json CMD ARGS...  -- run CMD (stdin/stdout -> /dev/null, stderr passed through) and
 * write {"rc","wall","cpu","rss_mib"} measured with wait4(). A tiny parent process matters: Linux
 * folds the parent's peak RSS into a child at exec, so measuring from a large Python process
 * puts a floor under every number. */
#include <fcntl.h>
#include <stdio.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc < 3) return 2;
    struct timespec a, b; clock_gettime(CLOCK_MONOTONIC, &a);
    pid_t p = fork();
    if (p == 0) { int n = open("/dev/null", O_RDWR); dup2(n, 0); dup2(n, 1); execvp(argv[2], argv + 2); _exit(127); }
    int st; struct rusage ru; wait4(p, &st, 0, &ru); clock_gettime(CLOCK_MONOTONIC, &b);
    double wall = (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;
    double cpu = ru.ru_utime.tv_sec + ru.ru_utime.tv_usec / 1e6 + ru.ru_stime.tv_sec + ru.ru_stime.tv_usec / 1e6;
    FILE *f = fopen(argv[1], "w");
    fprintf(f, "{\"rc\":%d,\"wall\":%.6f,\"cpu\":%.6f,\"rss_mib\":%.3f}\n", WIFEXITED(st) ? WEXITSTATUS(st) : 128 + WTERMSIG(st), wall, cpu, ru.ru_maxrss / 1024.0);
    fclose(f); return 0;
}
