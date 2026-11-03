#!/usr/bin/env python3
"""Run a command; report wall seconds, CPU seconds and peak RSS (MiB) of that child only."""
import json, os, subprocess, sys, time

def run(cmd, cwd=None, env=None, stdout=None, stdin=None):
    t0 = time.perf_counter()
    p = subprocess.Popen(cmd, cwd=cwd, env=env, stdin=stdin,
                         stdout=stdout if stdout is not None else subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
    _, status, ru = os.wait4(p.pid, 0)
    wall = time.perf_counter() - t0
    return {"rc": os.waitstatus_to_exitcode(status), "wall": wall,
            "cpu": ru.ru_utime + ru.ru_stime, "rss_mib": ru.ru_maxrss / 1024.0}

if __name__ == "__main__":
    print(json.dumps(run(sys.argv[1:])))
