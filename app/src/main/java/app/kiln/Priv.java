package app.kiln;

import android.content.ComponentName;
import android.content.Context;
import android.content.ServiceConnection;
import android.content.pm.PackageManager;
import android.os.Environment;
import android.os.IBinder;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;

import rikka.shizuku.Shizuku;

/**
 * Access to folders an ordinary app can't read (the whole file system from "/", other apps' data,
 * Android/data), through root (su) or Shizuku.
 *
 * Everything here is a shell command run with more rights than Kiln has. Kiln's own screens keep
 * using plain Java files; this class steps in only for paths that {@link #wants} says are out of
 * reach. Files that must be opened by a normal API (the text editor, an archive, another app) are
 * first copied to Kiln's cache ("staged") and written back afterwards.
 *
 * All methods that run a command block: call them from a background thread, except the tiny ones
 * used for a name check while a dialog is open.
 */
final class Priv {
    private Priv() { }

    static final int OFF = 0, ROOT = 1, SHIZUKU = 2;
    private static final int REQ_SHIZUKU = 4101;

    private static Context app;
    private static volatile IPrivService svc;              // the Shizuku service, once bound
    private static volatile boolean binding;
    private static volatile Boolean rootOk;                // null = not tried yet
    private static volatile String rootNote = "";
    private static boolean listening;
    private static String storage = "/storage/emulated/0";
    private static String ownDir = "";                     // Kiln's own folder under Android/data
    private static final AtomicInteger seq = new AtomicInteger();

    /** Staged copy (path) -> the real path it was copied from. */
    private static final Map<String, String> origins = new ConcurrentHashMap<>();
    /** Staged copies handed to another app for editing: path -> size|mtime when handed over. */
    private static final Map<String, String> watched = new ConcurrentHashMap<>();

    interface Listener { void changed(); }
    private static volatile Listener listener;

    static void setListener(Listener l) { listener = l; }

    private static void fire() {
        Listener l = listener;
        if (l != null) l.changed();
    }

    // ------------------------------------------------------------------ setup and state

    static synchronized void init(Context c) {
        if (app != null) return;
        app = c.getApplicationContext();
        try {
            storage = Environment.getExternalStorageDirectory().getPath();
        } catch (Throwable ignored) {
        }
        ownDir = storage + "/Android/data/" + app.getPackageName();
        if (!listening) {
            listening = true;
            try {
                Shizuku.addBinderReceivedListenerSticky(() -> connect());
                Shizuku.addBinderDeadListener(() -> {
                    svc = null;
                    binding = false;
                    fire();
                });
            } catch (Throwable ignored) {
            }
        }
        if (mode() == SHIZUKU) connect();
    }

    static int mode() { return app == null ? OFF : Defaults.privMode(app); }

    static int mode(Context c) { return Defaults.privMode(c); }

    static void setMode(int m) {
        if (app == null) return;
        Defaults.setPrivMode(app, m);
        if (m == SHIZUKU) connect();
        fire();
    }

    /** Whether commands can run right now. */
    static boolean ready() {
        switch (mode()) {
            case ROOT: return Boolean.TRUE.equals(rootOk);
            case SHIZUKU:
                IPrivService s = svc;
                return s != null && s.asBinder().pingBinder();
            default: return false;
        }
    }

    /** True when the chosen way in is switched on and working. */
    static boolean active() { return mode() != OFF && ready(); }

    // ---- Shizuku

    static boolean shizukuRunning() {
        try {
            return Shizuku.pingBinder();
        } catch (Throwable t) {
            return false;
        }
    }

    static boolean shizukuInstalled() {
        if (app == null) return false;
        try {
            app.getPackageManager().getPackageInfo("moe.shizuku.privileged.api", 0);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    static boolean shizukuGranted() {
        try {
            return !Shizuku.isPreV11() && Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED;
        } catch (Throwable t) {
            return false;
        }
    }

    /** Asks Shizuku for permission; cb gets true if granted. */
    static void requestShizuku(final java.util.function.Consumer<Boolean> cb) {
        try {
            if (!shizukuRunning()) {
                cb.accept(false);
                return;
            }
            if (shizukuGranted()) {
                connect();
                cb.accept(true);
                return;
            }
            Shizuku.addRequestPermissionResultListener(new Shizuku.OnRequestPermissionResultListener() {
                @Override
                public void onRequestPermissionResult(int requestCode, int grantResult) {
                    if (requestCode != REQ_SHIZUKU) return;
                    Shizuku.removeRequestPermissionResultListener(this);
                    boolean ok = grantResult == PackageManager.PERMISSION_GRANTED;
                    if (ok) connect();
                    cb.accept(ok);
                }
            });
            Shizuku.requestPermission(REQ_SHIZUKU);
        } catch (Throwable t) {
            cb.accept(false);
        }
    }

    private static final ServiceConnection CONN = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            svc = binder != null && binder.pingBinder() ? IPrivService.Stub.asInterface(binder) : null;
            binding = false;
            fire();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            svc = null;
            binding = false;
            fire();
        }
    };

    /** Starts the Shizuku service if Shizuku is running and has allowed Kiln. */
    static synchronized void connect() {
        if (app == null || svc != null || binding) return;
        if (!shizukuRunning() || !shizukuGranted()) return;
        try {
            binding = true;
            Shizuku.UserServiceArgs args = new Shizuku.UserServiceArgs(
                    new ComponentName(app.getPackageName(), PrivService.class.getName()))
                    .daemon(false)
                    .processNameSuffix("priv")
                    .version(1);
            Shizuku.bindUserService(args, CONN);
        } catch (Throwable t) {
            binding = false;
        }
    }

    /** Waits (up to ms) for the Shizuku service to come up. */
    private static void awaitShizuku(int ms) {
        connect();
        long end = System.currentTimeMillis() + ms;
        while (svc == null && System.currentTimeMillis() < end) {
            try {
                Thread.sleep(50);
            } catch (InterruptedException e) {
                return;
            }
        }
    }

    // ---- root

    /** Asks for root (the superuser app shows its prompt). Blocks. */
    static boolean probeRoot() {
        try {
            Res r = rootExec("id");
            boolean ok = r.code == 0 && r.out.contains("uid=0");
            rootOk = ok;
            rootNote = ok ? "" : (r.err.isEmpty() ? "Root access was refused or isn't available." : r.err.trim());
        } catch (Throwable t) {
            rootOk = false;
            rootNote = "Root isn't available on this device.";
        }
        fire();
        return Boolean.TRUE.equals(rootOk);
    }

    static String rootNote() { return rootNote; }

    static boolean rootTried() { return rootOk != null; }

    /** One line saying how access stands, for Settings and error messages. */
    static String status() {
        switch (mode()) {
            case ROOT:
                if (rootOk == null) return "Not checked yet";
                return rootOk ? "Working: root" : (rootNote.isEmpty() ? "Root not available" : rootNote);
            case SHIZUKU:
                if (!shizukuInstalled() && !shizukuRunning()) return "Shizuku isn't installed";
                if (!shizukuRunning()) return "Shizuku isn't running";
                if (!shizukuGranted()) return "Permission not given yet";
                IPrivService s = svc;
                if (s == null) return binding ? "Connecting..." : "Not connected";
                String who = "";
                try {
                    who = s.uid() == 0 ? " as root" : " as shell";
                } catch (Throwable ignored) {
                }
                return "Working: Shizuku" + who;
            default:
                return "Off";
        }
    }

    // ------------------------------------------------------------------ running commands

    static final class Res {
        int code;
        String out = "", err = "";
    }

    private static String q(String s) {
        return "'" + s.replace("'", "'\\''") + "'";
    }

    private static final int OUT_CAP = 24 * 1024 * 1024;

    private static Res rootExec(String cmd) throws IOException, InterruptedException {
        Process p = new ProcessBuilder("su", "-c", cmd).start();
        try {
            p.getOutputStream().close();
        } catch (IOException ignored) {
        }
        final ByteArrayOutputStream o = new ByteArrayOutputStream(), e = new ByteArrayOutputStream();
        Thread to = pump(p.getInputStream(), o), te = pump(p.getErrorStream(), e);
        int code = p.waitFor();
        to.join(5000);
        te.join(5000);
        Res r = new Res();
        r.code = code;
        r.out = new String(o.toByteArray(), "UTF-8");
        r.err = new String(e.toByteArray(), "UTF-8");
        return r;
    }

    private static Thread pump(final InputStream in, final ByteArrayOutputStream to) {
        Thread t = new Thread(() -> {
            byte[] buf = new byte[16384];
            try {
                int n;
                while ((n = in.read(buf)) > 0) {
                    if (to.size() < OUT_CAP) to.write(buf, 0, n);
                }
            } catch (IOException ignored) {
            }
        }, "kiln-priv-pump");
        t.setDaemon(true);
        t.start();
        return t;
    }

    private static Res shizukuExec(String cmd) throws IOException {
        awaitShizuku(4000);
        IPrivService s = svc;
        if (s == null) throw new IOException("Shizuku isn't connected");
        File dir = new File(app.getExternalCacheDir(), "priv/io");
        dir.mkdirs();
        File out = new File(dir, "o" + seq.incrementAndGet() + ".txt");
        File err = new File(dir, "e" + seq.get() + ".txt");
        try {
            Res r = new Res();
            r.code = s.exec(cmd, out.getPath(), err.getPath());
            r.out = slurp(out);
            r.err = slurp(err);
            return r;
        } catch (android.os.RemoteException e) {
            svc = null;
            throw new IOException("Lost the connection to Shizuku");
        } finally {
            out.delete();
            err.delete();
        }
    }

    private static String slurp(File f) {
        try (FileInputStream in = new FileInputStream(f)) {
            ByteArrayOutputStream b = new ByteArrayOutputStream();
            byte[] buf = new byte[16384];
            int n;
            while ((n = in.read(buf)) > 0) {
                if (b.size() < OUT_CAP) b.write(buf, 0, n);
            }
            return b.toString("UTF-8");
        } catch (IOException e) {
            return "";
        }
    }

    /** Runs a shell script with elevated rights. Throws if no way in is switched on. */
    static Res run(String script) throws IOException {
        try {
            switch (mode()) {
                case ROOT:
                    if (rootOk == null) probeRoot();
                    if (!Boolean.TRUE.equals(rootOk)) throw new IOException(status());
                    return rootExec(script);
                case SHIZUKU:
                    return shizukuExec(script);
                default:
                    throw new IOException("Root or Shizuku access is off (Settings > Access)");
            }
        } catch (InterruptedException e) {
            throw new IOException("Interrupted");
        }
    }

    private static String errOf(Res r) {
        String e = r.err.trim();
        if (e.isEmpty()) e = r.out.trim();
        if (e.length() > 300) e = e.substring(0, 300);
        return e.isEmpty() ? "failed (code " + r.code + ")" : e;
    }

    // ------------------------------------------------------------------ which paths need it

    /** Paths an ordinary app can't read: everything outside the storages, and Android/data and obb. */
    static boolean restricted(String path) {
        if (path == null || !path.startsWith("/")) return false;
        String p = path.length() > 1 && path.endsWith("/") ? path.substring(0, path.length() - 1) : path;
        if (p.equals("/sdcard") || p.startsWith("/sdcard/")) p = storage + p.substring(7);
        if (p.equals(ownDir) || p.startsWith(ownDir + "/")) return false;
        if (app != null) {
            String pkg = app.getPackageName();
            if (p.startsWith("/data/user/0/" + pkg) || p.startsWith("/data/data/" + pkg)) return false;
        }
        if (p.equals(storage) || p.startsWith(storage + "/")) {
            String rest = p.length() > storage.length() ? p.substring(storage.length() + 1) : "";
            return rest.equals("Android/data") || rest.startsWith("Android/data/")
                    || rest.equals("Android/obb") || rest.startsWith("Android/obb/");
        }
        if (p.startsWith("/storage/")) {
            String[] seg = p.substring(9).split("/");           // after "/storage/"
            int skip = seg[0].equals("emulated") || seg[0].equals("self") ? 2 : 1;
            if (seg.length < skip) return true;                // "/storage/emulated" on its own
            String r1 = seg.length > skip ? seg[skip] : "", r2 = seg.length > skip + 1 ? seg[skip + 1] : "";
            return r1.equals("Android") && (r2.equals("data") || r2.equals("obb"));
        }
        return true;                                           // "/", /data, /system, /storage itself...
    }

    /** True when `path` needs the elevated route and it is ready. */
    static boolean wants(String path) { return active() && restricted(path); }

    /** True when any of the paths in a JSON array (or `dest`) needs it. */
    static boolean involves(String jsonPaths, String dest) {
        if (!active()) return false;
        if (dest != null && !dest.isEmpty() && restricted(dest)) return true;
        try {
            JSONArray a = new JSONArray(jsonPaths);
            for (int i = 0; i < a.length(); i++) if (restricted(a.optString(i))) return true;
        } catch (Exception ignored) {
        }
        return false;
    }

    /** Why a restricted folder can't be shown (null when it can). */
    static String denyReason(String path) {
        if (!restricted(path)) return null;
        if (mode() == OFF) return "Turn on Root or Shizuku in Settings > Access to open this folder.";
        if (!ready()) return "Root/Shizuku access isn't ready: " + status() + ".";
        return null;
    }

    // ------------------------------------------------------------------ listing and searching

    private static final String TAB = "\t";
    private static final String FMT = "%F" + TAB + "%s" + TAB + "%Y" + TAB + "%n";

    private static JSONObject err(String m) {
        JSONObject o = new JSONObject();
        try {
            o.put("ok", false).put("error", m);
        } catch (Exception ignored) {
        }
        return o;
    }

    /** One folder in the same JSON shape the Python scan returns. Every plain file is included. */
    static JSONObject scan(String dir, boolean hidden, byte[] sig, int minSize, boolean builtin,
                           int encMask, String indexName) {
        try {
            String cmd = "cd -- " + q(dir) + " || exit 2\n"
                    + "[ -f " + q("./" + indexName) + " ] && echo '#set'\n"
                    + "stat -L -c '" + FMT + "' -- ./* ./.[!.]* ./..?* 2>/dev/null\n";
            Res r = run(cmd);
            if (r.out.isEmpty() && r.code == 2) return err("can't open " + dir + ": " + errOf(r));
            boolean isSet = false;
            List<JSONObject> dirs = new ArrayList<>(), files = new ArrayList<>();
            List<String> candidates = new ArrayList<>();
            for (String line : r.out.split("\n")) {
                if (line.equals("#set")) {
                    isSet = true;
                    continue;
                }
                String[] f = line.split(TAB, 4);
                if (f.length < 4) continue;
                String name = f[3].startsWith("./") ? f[3].substring(2) : f[3];
                if (name.isEmpty() || (!hidden && name.startsWith("."))) continue;
                long size = parse(f[1]), mtime = parse(f[2]);
                JSONObject o = new JSONObject();
                if (f[0].startsWith("directory")) {
                    o.put("n", name).put("k", "dir").put("s", 0).put("m", mtime).put("c", -1);
                    dirs.add(o);
                } else if (f[0].startsWith("regular")) {
                    o.put("n", name).put("k", "file").put("s", size).put("m", mtime).put("e", false);
                    files.add(o);
                    if (sig != null && size >= minSize) candidates.add(name);
                }
            }
            if (!candidates.isEmpty() && candidates.size() <= 80) sniff(dir, files, candidates, sig, builtin, encMask);
            java.util.Comparator<JSONObject> byName =
                    (x, y) -> x.optString("n").compareToIgnoreCase(y.optString("n"));
            java.util.Collections.sort(dirs, byName);
            java.util.Collections.sort(files, byName);
            JSONArray arr = new JSONArray();
            for (JSONObject o : dirs) arr.put(o);
            for (JSONObject o : files) arr.put(o);
            return new JSONObject().put("ok", true).put("path", dir).put("items", arr).put("is_set", isSet);
        } catch (Exception e) {
            return err(String.valueOf(e.getMessage()));
        }
    }

    private static long parse(String s) {
        try {
            return Long.parseLong(s.trim());
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    /** Marks the files that start with the archive signature (reads the first few bytes of each). */
    private static void sniff(String dir, List<JSONObject> files, List<String> names, byte[] sig,
                              boolean builtin, int encMask) {
        try {
            int want = Math.max(5, sig.length + 1);
            StringBuilder b = new StringBuilder("cd -- ").append(q(dir)).append(" || exit 2\n");
            for (String n : names) {
                b.append("printf '%s\\t' ").append(q(n)).append("; head -c ").append(want).append(" -- ")
                        .append(q("./" + n)).append(" | od -An -tx1 | tr -d ' \\n'; echo\n");
            }
            Res r = run(b.toString());
            StringBuilder want0 = new StringBuilder();
            for (byte x : sig) want0.append(String.format("%02x", x & 0xff));
            String sigHex = want0.toString();
            Map<String, String> heads = new ConcurrentHashMap<>();
            for (String line : r.out.split("\n")) {
                int t = line.lastIndexOf('\t');
                if (t > 0) heads.put(line.substring(0, t), line.substring(t + 1).trim());
            }
            for (JSONObject o : files) {
                String h = heads.get(o.optString("n"));
                if (h == null || !h.startsWith(sigHex)) continue;
                if (builtin) {
                    if (h.length() >= 10) {
                        int flags = Integer.parseInt(h.substring(8, 10), 16);
                        o.put("k", "archive").put("e", (flags & encMask) != 0);
                    }
                } else {
                    o.put("k", "archive");
                }
            }
        } catch (Exception ignored) {
        }
    }

    /** Names (files and folders) in a folder. */
    static Set<String> names(String dir) throws IOException {
        Res r = run("ls -A -- " + q(dir));
        Set<String> s = new HashSet<>();
        for (String l : r.out.split("\n")) if (!l.isEmpty()) s.add(l);
        return s;
    }

    /** A File that knows whether it is a folder (File.isDirectory can't tell for protected paths). */
    static final class PFile extends File {
        private final boolean dir;

        PFile(String path, boolean dir) {
            super(path);
            this.dir = dir;
        }

        @Override
        public boolean isDirectory() { return dir; }

        @Override
        public boolean isFile() { return !dir; }
    }

    /** Entries of a protected folder for the folder chooser. */
    static File[] kids(File dir) {
        JSONObject r = scan(dir.getPath(), false, null, 0, true, 0, "_index.kln");
        JSONArray a = r.optJSONArray("items");
        if (a == null) return new File[0];
        List<File> out = new ArrayList<>();
        for (int i = 0; i < a.length(); i++) {
            JSONObject o = a.optJSONObject(i);
            if (o == null) continue;
            out.add(new PFile(new File(dir, o.optString("n")).getPath(), "dir".equals(o.optString("k"))));
        }
        return out.toArray(new File[0]);
    }

    /** Names below `base` containing every word of `query` (case-insensitive), as the Python search returns them. */
    static JSONObject search(String base, String query, boolean hidden) {
        try {
            StringBuilder f = new StringBuilder();
            int terms = 0;
            for (String t : query.trim().split("\\s+")) {
                if (t.isEmpty()) continue;
                terms++;
                f.append(" -iname ").append(q("*" + t.replaceAll("[\\\\\\[\\]*?]", "?") + "*"));
            }
            if (terms == 0) return new JSONObject().put("ok", true).put("path", base)
                    .put("items", new JSONArray()).put("truncated", false);
            String cmd = "find " + q(base) + " -mindepth 1" + f + " -exec stat -L -c '" + FMT + "' {} + 2>/dev/null | head -n 1001\n";
            Res r = run(cmd);
            String prefix = base.endsWith("/") ? base : base + "/";
            JSONArray arr = new JSONArray();
            boolean truncated = false;
            for (String line : r.out.split("\n")) {
                String[] p = line.split(TAB, 4);
                if (p.length < 4 || !p[3].startsWith(prefix)) continue;
                String rel = p[3].substring(prefix.length());
                if (!hidden) {
                    boolean skip = false;
                    for (String seg : rel.split("/")) if (seg.startsWith(".")) skip = true;
                    if (skip) continue;
                }
                if (arr.length() >= 1000) {
                    truncated = true;
                    break;
                }
                JSONObject o = new JSONObject();
                if (p[0].startsWith("directory")) o.put("n", rel).put("k", "dir").put("s", 0).put("m", parse(p[2])).put("c", -1);
                else if (p[0].startsWith("regular")) o.put("n", rel).put("k", "file").put("s", parse(p[1])).put("m", parse(p[2])).put("e", false);
                else continue;
                arr.put(o);
            }
            return new JSONObject().put("ok", true).put("path", base).put("items", arr).put("truncated", truncated);
        } catch (Exception e) {
            return err(String.valueOf(e.getMessage()));
        }
    }

    /** Total size in bytes of a folder or file (du); -1 if unknown. */
    static long du(String path) {
        try {
            Res r = run("du -sk -- " + q(path) + " 2>/dev/null");
            String[] t = r.out.trim().split("\\s+");
            return t.length > 0 ? parse(t[0]) * 1024 : -1;
        } catch (Exception e) {
            return -1;
        }
    }

    static boolean exists(String path) {
        try {
            return run("[ -e " + q(path) + " ] || [ -L " + q(path) + " ]").code == 0;
        } catch (Exception e) {
            return false;
        }
    }

    // ------------------------------------------------------------------ changing things

    private static boolean outsideStorage(String path) {
        return !(path.startsWith("/storage/") || path.startsWith("/sdcard/") || path.startsWith("/mnt/"));
    }

    /** After root creates something in another app's folder, give it that folder's owner and label. */
    private static String ownFix(String path) {
        if (mode() != ROOT || !outsideStorage(path)) return "";
        String parent = path.contains("/") && path.lastIndexOf('/') > 0 ? path.substring(0, path.lastIndexOf('/')) : "/";
        return "U=$(stat -c %u:%g -- " + q(parent) + " 2>/dev/null); [ -n \"$U\" ] && chown -R \"$U\" -- " + q(path)
                + " 2>/dev/null; restorecon -R " + q(path) + " 2>/dev/null\n";
    }

    static String mkdirs(String path) {
        try {
            if (exists(path)) return "That name is taken";
            Res r = run("mkdir -p -- " + q(path) + "\n" + ownFix(path));
            return exists(path) ? null : errOf(r);
        } catch (Exception e) {
            return String.valueOf(e.getMessage());
        }
    }

    static String createFile(String path) {
        try {
            if (exists(path)) return "That name is taken";
            Res r = run(": > " + q(path) + "\n" + ownFix(path));
            return exists(path) ? null : errOf(r);
        } catch (Exception e) {
            return String.valueOf(e.getMessage());
        }
    }

    static String rename(String from, String to) {
        try {
            if (exists(to)) return "That name is taken";
            Res r = run("mv -- " + q(from) + " " + q(to));
            return r.code == 0 ? null : errOf(r);
        } catch (Exception e) {
            return String.valueOf(e.getMessage());
        }
    }

    private static JSONObject result(boolean ok, String msg, int done, int skipped) {
        JSONObject o = new JSONObject();
        try {
            o.put("ok", ok).put("message", msg).put("done", done).put("skipped", skipped).put("dirs", 0);
        } catch (Exception ignored) {
        }
        return o;
    }

    private static String parentOf(String p) {
        int i = p.lastIndexOf('/');
        return i <= 0 ? "/" : p.substring(0, i);
    }

    private static String baseOf(String p) {
        int i = p.lastIndexOf('/');
        return i < 0 ? p : p.substring(i + 1);
    }

    private static String join(String dir, String name) {
        return dir.endsWith("/") ? dir + name : dir + "/" + name;
    }

    /** Checks a copy or move first, like the Python fs_plan. */
    static JSONObject plan(String srcsJson, String dest, boolean move) {
        try {
            JSONArray a = new JSONArray(srcsJson);
            Set<String> have = names(dest);
            if (!exists(dest)) return err("the destination folder doesn't exist");
            JSONArray conflicts = new JSONArray();
            for (int i = 0; i < a.length(); i++) {
                String s = a.getString(i);
                String base = baseOf(s);
                if (!exists(s)) return err(base + " no longer exists");
                String d = dest.endsWith("/") ? dest : dest + "/";
                if (d.startsWith(s + "/")) return err("can't put \"" + base + "\" inside itself");
                boolean same = parentOf(s).equals(dest.length() > 1 && dest.endsWith("/")
                        ? dest.substring(0, dest.length() - 1) : dest);
                if (same && move) return err("the items are already in this folder");
                if (same) continue;
                if (have.contains(base)) conflicts.put(base);
            }
            return new JSONObject().put("ok", true).put("conflicts", conflicts);
        } catch (Exception e) {
            return err(String.valueOf(e.getMessage()));
        }
    }

    private static String unique(String name, Set<String> taken) {
        int dot = name.lastIndexOf('.');
        String root = dot > 0 ? name.substring(0, dot) : name, ext = dot > 0 ? name.substring(dot) : "";
        for (int i = 1; ; i++) {
            String c = root + " (" + i + ")" + ext;
            if (!taken.contains(c)) return c;
        }
    }

    /** Copies or moves with shell cp / mv. policy: replace, skip or keep. */
    static JSONObject transfer(String srcsJson, String dest, boolean move, String policy) {
        try {
            JSONArray a = new JSONArray(srcsJson);
            Set<String> have = names(dest);
            String dd = dest.length() > 1 && dest.endsWith("/") ? dest.substring(0, dest.length() - 1) : dest;
            StringBuilder b = new StringBuilder();
            List<String> targets = new ArrayList<>();
            int skipped = 0, todo = 0;
            for (int i = 0; i < a.length(); i++) {
                String s = a.getString(i);
                String base = baseOf(s);
                boolean same = parentOf(s).equals(dd);
                String name = base;
                if (same) {
                    name = unique(base, have);
                } else if (have.contains(base)) {
                    if (policy.equals("skip")) {
                        skipped++;
                        continue;
                    } else if (policy.equals("keep")) {
                        name = unique(base, have);
                    } else {
                        b.append("rm -rf -- ").append(q(join(dd, base))).append('\n');
                    }
                }
                have.add(name);
                String t = join(dd, name);
                targets.add(t);
                todo++;
                b.append(move ? "mv -f -- " : "cp -a -- ").append(q(s)).append(' ').append(q(t))
                        .append(" || echo ").append(q("FAIL\t" + base)).append(" >&2\n");
            }
            if (todo == 0) return result(true, skipped > 0 ? "Skipped " + skipped + " item(s)" : "Nothing to do", 0, skipped);
            for (String t : targets) b.append(ownFix(t));
            Res r = run(b.toString());
            List<String> failed = new ArrayList<>();
            for (String l : r.err.split("\n")) {
                if (l.startsWith("FAIL\t")) failed.add(l.substring(5));
            }
            int done = todo - failed.size();
            String verb = move ? "Moved " : "Copied ";
            String msg = verb + done + " item(s)" + (skipped > 0 ? ", skipped " + skipped : "");
            if (!failed.isEmpty()) {
                msg += "; " + failed.size() + " failed:\n" + errOf(r);
                return result(false, msg, done, skipped);
            }
            return result(true, msg, done, skipped);
        } catch (Exception e) {
            return result(false, String.valueOf(e.getMessage()), 0, 0);
        }
    }

    static JSONObject delete(String pathsJson) {
        try {
            JSONArray a = new JSONArray(pathsJson);
            StringBuilder b = new StringBuilder("rm -rf --");
            for (int i = 0; i < a.length(); i++) b.append(' ').append(q(a.getString(i)));
            Res r = run(b.toString());
            int left = 0;
            for (int i = 0; i < a.length(); i++) if (exists(a.getString(i))) left++;
            if (left > 0) return result(false, left + " item(s) couldn't be deleted:\n" + errOf(r), a.length() - left, 0);
            return result(true, "Deleted " + a.length() + " item(s)", a.length(), 0);
        } catch (Exception e) {
            return result(false, String.valueOf(e.getMessage()), 0, 0);
        }
    }

    // ------------------------------------------------------------------ staging (real file <-> cache copy)

    private static File stageRoot() {
        File d = new File(app.getExternalCacheDir(), "priv/stage");
        d.mkdirs();
        return d;
    }

    private static String hash(String s) {
        return Integer.toHexString(s.hashCode()) + Integer.toHexString(s.length() * 31 + 7);
    }

    /** Copies a protected file into Kiln's cache and returns the copy (reused while the file is unchanged). */
    static File stage(String path) throws IOException {
        Res st = run("stat -L -c '%s" + TAB + "%Y' -- " + q(path));
        if (st.code != 0) throw new IOException(errOf(st));
        String sig = st.out.trim();
        File dir = new File(stageRoot(), hash(path));
        dir.mkdirs();
        File dst = new File(dir, baseOf(path));
        File meta = new File(dir, ".meta");
        String have = meta.isFile() ? slurp(meta).trim() : "";
        if (dst.isFile() && have.equals(sig)) {
            origins.put(dst.getPath(), path);
            return dst;
        }
        dst.delete();
        Res r = run("cp -f -- " + q(path) + " " + q(dst.getPath()) + " && chmod 666 " + q(dst.getPath()));
        if (r.code != 0 || !dst.isFile()) throw new IOException(errOf(r));
        try (java.io.FileOutputStream o = new java.io.FileOutputStream(meta)) {
            o.write(sig.getBytes("UTF-8"));
        }
        origins.put(dst.getPath(), path);
        return dst;
    }

    /** The real path a staged copy came from (null if it isn't a staged copy). */
    static String originOf(String stagedPath) { return stagedPath == null ? null : origins.get(stagedPath); }

    /** Writes a staged copy back over the original, keeping the original's owner and permissions. */
    static void push(File staged, String origin) throws IOException {
        Res r = run("if [ -e " + q(origin) + " ]; then cat " + q(staged.getPath()) + " > " + q(origin)
                + "; else cp -f -- " + q(staged.getPath()) + " " + q(origin) + "; " + ownFix(origin).replace("\n", "; ")
                + " true; fi");
        if (r.code != 0) throw new IOException(errOf(r));
        try {
            Res st = run("stat -L -c '%s" + TAB + "%Y' -- " + q(origin));
            File meta = new File(staged.getParentFile(), ".meta");
            try (java.io.FileOutputStream o = new java.io.FileOutputStream(meta)) {
                o.write(st.out.trim().getBytes("UTF-8"));
            }
        } catch (Exception ignored) {
        }
    }

    /** A fresh empty folder in the cache for something being built before it is copied in. */
    static File newTemp() {
        File d = new File(app.getExternalCacheDir(), "priv/tmp/" + System.nanoTime());
        d.mkdirs();
        return d;
    }

    /** Copies everything in a cache folder into a protected folder. Returns an error text or null. */
    static String pushInto(File tmp, String dest, boolean overwrite) {
        try {
            Res r = run("mkdir -p -- " + q(dest) + " && cp -a" + (overwrite ? "" : "n") + " " + q(tmp.getPath() + "/.")
                    + " " + q(dest + "/") + "\n" + ownFix(dest));
            return r.code == 0 ? null : errOf(r);
        } catch (Exception e) {
            return String.valueOf(e.getMessage());
        }
    }

    static void deleteTree(File f) {
        File[] k = f.listFiles();
        if (k != null) for (File c : k) deleteTree(c);
        f.delete();
    }

    /** Removes everything staged (called at start-up so old copies don't pile up). */
    static void clearStage() {
        try {
            if (app == null) return;
            deleteTree(new File(app.getExternalCacheDir(), "priv"));
        } catch (Throwable ignored) {
        }
    }

    // ---- files handed to another app for editing: written back when Kiln comes to the front

    static void watch(String stagedPath) {
        if (!origins.containsKey(stagedPath)) return;
        File f = new File(stagedPath);
        watched.put(stagedPath, f.length() + "|" + f.lastModified());
    }

    /** Writes back staged files another app changed. Returns the names written (blocks). */
    static List<String> syncWatched() {
        List<String> done = new ArrayList<>();
        for (Map.Entry<String, String> e : new ArrayList<>(watched.entrySet())) {
            File f = new File(e.getKey());
            String now = f.length() + "|" + f.lastModified();
            if (now.equals(e.getValue())) continue;
            String origin = origins.get(e.getKey());
            if (origin == null) continue;
            try {
                push(f, origin);
                watched.put(e.getKey(), now);
                done.add(f.getName());
            } catch (IOException ignored) {
            }
        }
        return done;
    }

    static boolean hasWatched() { return !watched.isEmpty(); }
}
