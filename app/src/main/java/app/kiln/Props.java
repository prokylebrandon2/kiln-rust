package app.kiln;

import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.system.Os;
import android.system.StructStat;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.MimeTypeMap;
import android.widget.CheckBox;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.InputStream;
import java.nio.file.Files;
import java.security.MessageDigest;
import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Date;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.atomic.AtomicBoolean;

/** The "Properties" box: general facts about a file or folder, plus archive facts and an MD5.
 *  Slow parts (folder totals, MD5, reading an archive's table) run in the background, show
 *  "Calculating..." and stop when the box is closed. */
final class Props {
    private Props() {}

    /** Reads a Kiln archive's table (the engine's open_archive). */
    interface Probe { JSONObject open(File f) throws Exception; }

    private final static class Box {
        final Activity a;
        final LinearLayout col;
        final List<String[]> lines = new ArrayList<>();       // label, value: what Copy puts on the clipboard
        final AtomicBoolean stop = new AtomicBoolean(false);

        Box(Activity a) {
            this.a = a;
            col = new LinearLayout(a);
            col.setOrientation(LinearLayout.VERTICAL);
        }

        void heading(String s) {
            TextView t = Ui.label(a, s, 15, true, false);
            t.setTextColor(Ui.accent(a));
            t.setPadding(0, Ui.dp(a, 10), 0, Ui.dp(a, 2));
            col.addView(t);
            lines.add(new String[]{s, null});
        }

        /** One "Label  value" row. Returns the value view so it can be filled in later. */
        TextView row(String label, String value) {
            LinearLayout r = new LinearLayout(a);
            r.setPadding(0, Ui.dp(a, 3), 0, Ui.dp(a, 3));
            TextView l = Ui.label(a, label, 14, true, true);
            r.addView(l, new LinearLayout.LayoutParams(Ui.dp(a, 128), ViewGroup.LayoutParams.WRAP_CONTENT));
            TextView v = Ui.label(a, value, 14, false, false);
            v.setTextIsSelectable(true);
            r.addView(v, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
            col.addView(r);
            lines.add(new String[]{label, value});
            return v;
        }

        /** Changes a row's value (from any thread) and what Copy will use. */
        void set(final TextView v, final String label, final String value) {
            a.runOnUiThread(() -> {
                if (stop.get()) return;
                v.setText(value);
                for (String[] ln : lines) if (label.equals(ln[0])) ln[1] = value;
            });
        }

        String text() {
            StringBuilder sb = new StringBuilder();
            for (String[] ln : lines) {
                if (ln[1] == null) sb.append('\n').append(ln[0]).append('\n');
                else sb.append(ln[0]).append(": ").append(ln[1]).append('\n');
            }
            return sb.toString().trim();
        }
    }

    /** kinds maps a file to "archive" / "set" / "old" / "file" / "dir" (as the file list knows it). */
    static void show(final Activity a, final List<File> files, final Map<String, String> kinds,
                     final Probe probe, final Runnable changed) {
        if (files.isEmpty()) return;
        final Box b = new Box(a);
        if (files.size() == 1) single(b, files.get(0), kinds.get(files.get(0).getName()), probe, changed);
        else several(b, files);
        new Sheet(a, "Properties").content(b.col)
                .onClose(() -> b.stop.set(true))
                .button("Copy", Ui.TEXT, () -> {
                    ClipboardManager cm = (ClipboardManager) a.getSystemService(Context.CLIPBOARD_SERVICE);
                    if (cm != null) cm.setPrimaryClip(ClipData.newPlainText("Properties", b.text()));
                    Toast.makeText(a, "Copied", Toast.LENGTH_SHORT).show();
                    return false;
                })
                .button("OK", Ui.FILLED, null).show();
    }

    // ------------------------------------------------------------------ one item

    private static void single(final Box b, final File f, String kind, final Probe probe,
                               final Runnable changed) {
        final Activity a = b.a;
        final boolean dir = f.isDirectory();
        TextView name = Ui.label(a, f.getName(), 18, true, false);
        name.setPadding(0, 0, 0, Ui.dp(a, 2));
        b.col.addView(name);
        b.heading("General information");
        b.row("Type", dir ? "Folder" : typeOf(f, kind));
        final TextView count = dir ? b.row("File count", "Calculating...") : null;
        final TextView size = b.row("Size", dir ? "Calculating..." : sizeText(f.length()));
        b.row("Last modification", new SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.getDefault())
                .format(new Date(f.lastModified())));
        String perm = permissions(f);
        if (perm != null) b.row("Permissions", perm);
        String[] mnt = mountOf(f);
        if (mnt != null) {
            b.row("FS path", mnt[0]);
            b.row("FS type", mnt[1]);
        }
        if (dir) {
            final File nomedia = new File(f, ".nomedia");
            CheckBox cb = Ui.check(a, "Disable media scanning", nomedia.exists());
            cb.setOnCheckedChangeListener((v, on) -> {
                try {
                    if (on) {
                        if (!nomedia.exists() && !nomedia.createNewFile()) throw new java.io.IOException();
                    } else if (nomedia.exists() && !nomedia.delete()) {
                        throw new java.io.IOException();
                    }
                    if (changed != null) changed.run();
                } catch (Exception e) {
                    Toast.makeText(a, "Couldn't change that", Toast.LENGTH_SHORT).show();
                    v.setOnCheckedChangeListener(null);
                    v.setChecked(nomedia.exists());
                }
            });
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            lp.topMargin = Ui.dp(a, 6);
            lp.gravity = Gravity.START;
            b.col.addView(cb, lp);
            totals(b, new File[]{f}, count, size, null);
            return;
        }
        final TextView md5 = b.row("MD5", "Calculating...");
        new Thread(() -> {
            String r;
            try {
                r = md5Of(f, b.stop);
            } catch (Exception e) {
                r = "unavailable";
            }
            if (r != null) b.set(md5, "MD5", r);
        }).start();

        if ("archive".equals(kind) || "set".equals(kind)) {
            b.heading("Archive properties");
            final boolean set = "set".equals(kind);
            b.row("Type", set ? "Kiln set (random names)" : "Kiln archive");
            final TextView enc = b.row("Encrypted", set ? "Yes" : "...");
            final TextView n = b.row("File count", "Calculating...");
            final TextView raw = b.row("Content size", "Calculating...");
            final TextView ratio = b.row("Compression ratio", "Calculating...");
            new Thread(() -> {
                try {
                    JSONObject r = probe.open(f);
                    if (b.stop.get()) return;
                    if (r.optBoolean("need_password")) {
                        b.set(enc, "Encrypted", "Yes (open it to see more)");
                        b.set(n, "File count", "needs the password");
                        b.set(raw, "Content size", "needs the password");
                        b.set(ratio, "Compression ratio", "needs the password");
                        return;
                    }
                    if (!r.optBoolean("ok")) {
                        String why = r.optString("error", "couldn't read it");
                        b.set(enc, "Encrypted", "?");
                        b.set(n, "File count", why);
                        b.set(raw, "Content size", "?");
                        b.set(ratio, "Compression ratio", "?");
                        return;
                    }
                    JSONArray ents = r.optJSONArray("entries");
                    long total = 0;
                    int cnt = ents == null ? 0 : ents.length();
                    for (int i = 0; i < cnt; i++) total += Math.max(ents.getJSONObject(i).optLong("s"), 0);
                    long packed = r.optLong("packed", f.length());
                    b.set(enc, "Encrypted", r.optBoolean("encrypted") ? "Yes" : "No");
                    b.set(n, "File count", String.valueOf(cnt));
                    b.set(raw, "Content size", sizeText(total));
                    b.set(ratio, "Compression ratio", total > 0 ? Math.round(100.0 * packed / total) + "%" : "-");
                } catch (Exception e) {
                    b.set(n, "File count", "couldn't read it");
                }
            }).start();
        }
    }

    // ------------------------------------------------------------------ several items

    private static void several(final Box b, List<File> files) {
        final Activity a = b.a;
        b.heading("General information");
        b.row("Items", files.size() + " selected");
        File parent = files.get(0).getParentFile();
        if (parent != null) b.row("Location", parent.getPath());
        final TextView count = b.row("File count", "Calculating...");
        final TextView size = b.row("Size", "Calculating...");
        final TextView folders = b.row("Folders", "Calculating...");
        totals(b, files.toArray(new File[0]), count, size, folders);
    }

    /** Walks folders in the background, updating the totals as it goes. */
    private static void totals(final Box b, final File[] roots, final TextView count, final TextView size,
                               final TextView folders) {
        new Thread(() -> {
            final long[] t = new long[3];                         // files, bytes, folders
            long[] last = {0};
            for (File r : roots) {
                if (!walk(r, t, b.stop, last, b, count, size, folders, true)) return;
            }
            publish(b, t, count, size, folders, false);
        }).start();
    }

    private static boolean walk(File f, long[] t, AtomicBoolean stop, long[] last, Box b, TextView count,
                                TextView size, TextView folders, boolean top) {
        if (stop.get()) return false;
        if (!Files.isSymbolicLink(f.toPath()) && f.isDirectory()) {
            if (!top || folders != null) t[2]++;
            File[] kids = f.listFiles();
            if (kids != null) {
                for (File k : kids) if (!walk(k, t, stop, last, b, count, size, folders, false)) return false;
            }
        } else {
            t[0]++;
            t[1] += f.length();
        }
        long now = System.currentTimeMillis();
        if (now - last[0] > 150) {
            last[0] = now;
            publish(b, t, count, size, folders, true);
        }
        return true;
    }

    private static void publish(Box b, long[] t, TextView count, TextView size, TextView folders, boolean busy) {
        String dots = busy ? "..." : "";
        if (count != null) b.set(count, "File count", t[0] + dots);
        if (size != null) b.set(size, "Size", sizeText(t[1]) + dots);
        if (folders != null) b.set(folders, "Folders", t[2] + dots);
    }

    // ------------------------------------------------------------------ facts

    static String sizeText(long n) {
        return n < 1024 ? n + " B" : Fmt.size(n) + " (" + n + " B)";
    }

    private static String typeOf(File f, String kind) {
        if ("archive".equals(kind) || "set".equals(kind)) return "application/x-kiln";
        String m = MimeTypeMap.getSingleton().getMimeTypeFromExtension(Fmt.ext(f.getName()));
        if (m != null) return m;
        if (f.isFile()) {                                    // no extension: go by the content
            String s = Sniff.mime(f);
            if (!"*/*".equals(s)) return s;
            return "application/octet-stream";
        }
        return "Unknown";
    }

    /** "rw- rw- --- (660)" */
    private static String permissions(File f) {
        try {
            StructStat st = Os.stat(f.getPath());
            int m = st.st_mode & 0777;
            StringBuilder sb = new StringBuilder();
            for (int shift = 6; shift >= 0; shift -= 3) {
                int bits = (m >> shift) & 7;
                sb.append((bits & 4) != 0 ? 'r' : '-').append((bits & 2) != 0 ? 'w' : '-')
                        .append((bits & 1) != 0 ? 'x' : '-');
                if (shift > 0) sb.append(' ');
            }
            return sb + " (" + Integer.toOctalString(m) + ")";
        } catch (Exception e) {
            return null;
        }
    }

    /** The mount point holding f and its file system type, from /proc/mounts. */
    private static String[] mountOf(File f) {
        try {
            String path = f.getCanonicalPath();
            String best = null, type = null;
            try (java.io.BufferedReader r = new java.io.BufferedReader(
                    new java.io.FileReader("/proc/mounts"))) {
                String ln;
                while ((ln = r.readLine()) != null) {
                    String[] p = ln.split(" ");
                    if (p.length < 3) continue;
                    String mp = p[1].replace("\\040", " ");
                    boolean inside = mp.equals("/") || path.equals(mp) || path.startsWith(mp + "/");
                    if (inside && (best == null || mp.length() >= best.length())) {
                        best = mp;
                        type = p[2];
                    }
                }
            }
            return best == null ? null : new String[]{best, type};
        } catch (Exception e) {
            return null;
        }
    }

    /** MD5 of a file; null if the box was closed meanwhile. */
    private static String md5Of(File f, AtomicBoolean stop) throws Exception {
        MessageDigest md = MessageDigest.getInstance("MD5");
        try (InputStream in = new FileInputStream(f)) {
            byte[] buf = new byte[1 << 18];
            int k;
            while ((k = in.read(buf)) > 0) {
                if (stop.get()) return null;
                md.update(buf, 0, k);
            }
        }
        StringBuilder sb = new StringBuilder();
        for (byte x : md.digest()) sb.append(String.format(Locale.US, "%02x", x));
        return sb.toString();
    }

    /** Properties of the selected entries inside an archive (what the table says; no MD5). */
    static void showInArchive(final Activity a, String archiveName, List<String[]> picked) {
        // picked rows: {path, size, modified (seconds), "dir" or "file", number of files}
        final Box b = new Box(a);
        if (picked.size() == 1) {
            String[] p = picked.get(0);
            String nm = p[0].endsWith("/") ? p[0].substring(0, p[0].length() - 1) : p[0];
            TextView name = Ui.label(a, nm.substring(nm.lastIndexOf('/') + 1), 18, true, false);
            name.setPadding(0, 0, 0, Ui.dp(a, 2));
            b.col.addView(name);
        }
        b.heading("General information");
        long size = 0, files = 0;
        for (String[] p : picked) {
            size += Long.parseLong(p[1]);
            files += Long.parseLong(p[4]);
        }
        if (picked.size() == 1) {
            String[] p = picked.get(0);
            boolean dir = "dir".equals(p[3]);
            String path = p[0].endsWith("/") ? p[0].substring(0, p[0].length() - 1) : p[0];
            b.row("Type", dir ? "Folder" : typeOf(new File(path), "file"));
            b.row("Path", path);
            if (dir) b.row("File count", String.valueOf(files));
            b.row("Size", sizeText(size));
            long m = Long.parseLong(p[2]);
            if (m > 0) b.row("Last modification", new SimpleDateFormat("yyyy-MM-dd HH:mm:ss",
                    Locale.getDefault()).format(new Date(m * 1000L)));
        } else {
            b.row("Items", picked.size() + " selected");
            b.row("File count", String.valueOf(files));
            b.row("Size", sizeText(size));
        }
        b.row("In archive", archiveName);
        new Sheet(a, "Properties").content(b.col)
                .button("Copy", Ui.TEXT, () -> {
                    ClipboardManager cm = (ClipboardManager) a.getSystemService(Context.CLIPBOARD_SERVICE);
                    if (cm != null) cm.setPrimaryClip(ClipData.newPlainText("Properties", b.text()));
                    Toast.makeText(a, "Copied", Toast.LENGTH_SHORT).show();
                    return false;
                })
                .button("OK", Ui.FILLED, null).show();
    }
}
