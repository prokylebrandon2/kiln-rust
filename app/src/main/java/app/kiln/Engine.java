package app.kiln;

import android.content.Context;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.TimeUnit;

/**
 * Talks to the Rust engine. One bridge process (libkiln_bridge.so, launched from the native
 * library folder) runs per user operation, never per file: the request goes in as one JSON line,
 * progress comes back as JSON lines, and the last line is the result. Writing "cancel" to the
 * process asks the engine to stop cleanly; it is killed only if it has not exited after 3 seconds.
 *
 * Java never reads archive data: all of that happens inside the engine.
 */
final class Engine {
    private static final long KILL_AFTER_MS = 3000;

    private static volatile String exe = "";
    /** Where the engine keeps scratch files (TMPDIR): inside the app cache, so Settings > Cache covers it. */
    private static volatile File tmp;
    /** Parallel jobs: 0 = automatic (the engine uses every core). */
    private static volatile int jobs = 0;
    /** The search that is running now (a newer search or cancelSearch stops it). */
    private static volatile Run searchRun;

    private Engine() { }

    /** Remember where the bridge binary is. Call once at start. */
    static void init(Context c) {
        exe = c.getApplicationInfo().nativeLibraryDir + "/libkiln_bridge.so";
        File t = new File(c.getCacheDir(), "engine-tmp");
        //noinspection ResultOfMethodCallIgnored
        t.mkdirs();
        tmp = t;
    }

    /** Settings > parallel jobs. 0 (automatic) lets the engine use all cores. */
    static void setJobs(int n) {
        jobs = Math.max(0, n);
    }

    static boolean ready() {
        return !exe.isEmpty() && new File(exe).canExecute();
    }

    /**
     * Runs one engine function and returns its result object. A {@link MainActivity.Progress}
     * among the arguments is not sent: it receives the engine's progress events and, when it is
     * cancelled, tells the engine to stop.
     */
    static JSONObject call(String fn, Object... args) throws Exception {
        MainActivity.Progress progress = null;
        JSONArray list = new JSONArray();
        for (Object a : args) {
            if (a instanceof MainActivity.Progress) {
                progress = (MainActivity.Progress) a;
            } else if (a == null) {
                list.put(JSONObject.NULL);
            } else if (a instanceof File) {
                list.put(((File) a).getPath());
            } else {
                list.put(a);
            }
        }
        JSONObject req = new JSONObject();
        req.put("fn", fn);
        req.put("args", list);
        req.put("jobs", jobs);
        return run(fn, req.toString(), progress);
    }

    /** Stops the running search, if any. */
    static void cancelSearch() {
        Run r = searchRun;
        if (r != null) r.cancel();
    }

    // ------------------------------------------------------------------ one process

    private static JSONObject run(String fn, String request, MainActivity.Progress progress)
            throws Exception {
        if (!ready()) {
            return failure("the Kiln engine is missing from this install", false);
        }
        final boolean isSearch = fn.equals("search");
        if (isSearch) cancelSearch();                    // a newer search replaces the old one

        ProcessBuilder pb = new ProcessBuilder(exe);
        pb.redirectErrorStream(false);
        File t = tmp;
        if (t != null) pb.environment().put("TMPDIR", t.getPath());   // Android has no usable default
        final Run r = new Run(pb.start());
        if (isSearch) searchRun = r;
        if (progress != null) {
            progress.setOnCancel(r::cancel);
            if (progress.cancelled()) r.cancel();        // cancelled before we attached
        }
        try {
            r.send(request);
            return r.read(progress);
        } finally {
            if (isSearch && searchRun == r) searchRun = null;
            if (progress != null) progress.setOnCancel(null);
            r.finish();
        }
    }

    private static JSONObject failure(String msg, boolean cancelled) {
        JSONObject o = new JSONObject();
        try {
            o.put("ok", false);
            if (cancelled) o.put("cancelled", true);
            o.put("error", msg);
            o.put("message", msg);
        } catch (JSONException ignored) {
        }
        return o;
    }

    /** A running bridge process and what is needed to talk to it. */
    private static final class Run {
        final Process proc;
        final OutputStream in;
        final StringBuilder err = new StringBuilder();
        volatile boolean cancelled;

        Run(Process p) {
            proc = p;
            in = p.getOutputStream();
            final InputStream es = p.getErrorStream();
            Thread t = new Thread(() -> {                // keep stderr from filling up its pipe
                byte[] buf = new byte[1024];
                try {
                    int n;
                    while ((n = es.read(buf)) > 0) {
                        synchronized (err) {
                            if (err.length() < 2048) {
                                err.append(new String(buf, 0, n, StandardCharsets.UTF_8));
                            }
                        }
                    }
                } catch (IOException ignored) {
                }
            }, "kiln-engine-stderr");
            t.setDaemon(true);
            t.start();
        }

        void send(String line) throws IOException {
            in.write((line + "\n").getBytes(StandardCharsets.UTF_8));
            in.flush();                                  // stdin stays open: that is how "cancel" gets in
        }

        /** Asks the engine to stop; kills it if it is still there 3 seconds later. */
        void cancel() {
            if (cancelled) return;
            cancelled = true;
            Thread t = new Thread(() -> {
                try {
                    in.write("cancel\n".getBytes(StandardCharsets.UTF_8));
                    in.flush();
                } catch (IOException ignored) {
                }
                try {
                    if (!proc.waitFor(KILL_AFTER_MS, TimeUnit.MILLISECONDS)) proc.destroyForcibly();
                } catch (InterruptedException e) {
                    proc.destroyForcibly();
                }
            }, "kiln-engine-cancel");
            t.setDaemon(true);
            t.start();
        }

        /** Reads progress lines until the result line. */
        JSONObject read(MainActivity.Progress progress) throws IOException {
            BufferedReader br = new BufferedReader(
                    new InputStreamReader(proc.getInputStream(), StandardCharsets.UTF_8), 1 << 16);
            String line;
            while ((line = br.readLine()) != null) {
                if (line.isEmpty()) continue;
                JSONObject o;
                try {
                    o = new JSONObject(line);
                } catch (JSONException e) {
                    continue;                            // not ours: ignore stray output
                }
                JSONObject res = o.optJSONObject("result");
                if (res != null) return res;
                JSONObject pr = o.optJSONObject("progress");
                if (pr != null && progress != null) progress.event(pr.toString());
            }
            if (cancelled) return failure("Cancelled", true);
            String tail;
            synchronized (err) {
                tail = err.toString().trim();
            }
            return failure(tail.isEmpty() ? "the Kiln engine stopped unexpectedly"
                    : "the Kiln engine stopped unexpectedly: " + tail, false);
        }

        /** Closes the pipes and makes sure the process is gone. */
        void finish() {
            try {
                in.close();
            } catch (IOException ignored) {
            }
            try {
                if (!proc.waitFor(KILL_AFTER_MS, TimeUnit.MILLISECONDS)) proc.destroyForcibly();
            } catch (InterruptedException e) {
                proc.destroyForcibly();
            }
        }
    }
}
