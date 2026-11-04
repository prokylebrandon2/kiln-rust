package app.kiln;

import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Random;

/**
 * The recycle bin. Each storage (Internal storage, an SD card or flash drive) keeps its own bin in a
 * hidden folder at its top, so putting something in is a rename on the same volume: instant, whatever
 * the size, and it needs no extra space. An item is stored as "<id>" (the file or folder itself,
 * renamed) plus "<id>.json" saying what it was called, where it came from and when it was deleted.
 */
final class Trash {
    private Trash() {}

    static final String DIR = ".KilnTrash";
    private static final Random RND = new Random();

    /** One thing in a bin. */
    static final class Entry {
        File data;          // the item itself, inside the bin
        File meta;          // its description (.json)
        String name;        // the name it had
        String orig;        // the full path it came from
        long when;          // deleted at, epoch seconds
        boolean dir;
        long size = -1;     // bytes (filled by size()); -1 = not counted yet
        File root;          // the storage this bin belongs to
    }

    // ---- where an item can go

    /** The storage (one of `roots`) that holds `f`, or null if none does or `f` is a bin or a storage itself. */
    static File rootOf(File f, List<File> roots) {
        String p = f.getAbsolutePath();
        File best = null;
        for (File r : roots) {
            String rp = r.getAbsolutePath();
            if (p.equals(rp)) return null;                           // a whole storage can't be binned
            if (p.startsWith(rp + "/") && (best == null || rp.length() > best.getAbsolutePath().length())) best = r;
        }
        if (best == null) return null;
        String rel = p.substring(best.getAbsolutePath().length() + 1);
        if (rel.equals(DIR) || rel.startsWith(DIR + "/")) return null;   // already inside the bin
        return best;
    }

    // ---- put in

    /** Moves `f` into the bin of `root`. Returns null when done, or what went wrong. */
    static String put(File f, File root) {
        File bin = new File(root, DIR);
        if (!bin.isDirectory() && !bin.mkdirs()) return "can't create the recycle bin";
        try {
            File nomedia = new File(bin, ".nomedia");          // keep gallery apps out of it
            if (!nomedia.exists()) new FileOutputStream(nomedia).close();
        } catch (Exception ignored) {
        }
        File data, meta;
        do {
            String id = Long.toString(System.currentTimeMillis(), 36) + Integer.toString(RND.nextInt(1 << 20), 36);
            data = new File(bin, id);
            meta = new File(bin, id + ".json");
        } while (data.exists() || meta.exists());
        try {
            JSONObject o = new JSONObject();
            o.put("name", f.getName());
            o.put("orig", f.getAbsolutePath());
            o.put("when", System.currentTimeMillis() / 1000);
            o.put("dir", f.isDirectory());
            try (FileOutputStream out = new FileOutputStream(meta)) {
                out.write(o.toString().getBytes(StandardCharsets.UTF_8));
            }
        } catch (Exception e) {
            meta.delete();
            return "can't write to the recycle bin: " + e.getMessage();
        }
        if (!f.renameTo(data)) {
            meta.delete();
            return "couldn't move \"" + f.getName() + "\" to the recycle bin";
        }
        return null;
    }

    // ---- look inside

    /** Everything in the bin of `root`, newest first. Sizes are not counted yet. */
    static List<Entry> list(File root) {
        List<Entry> out = new ArrayList<>();
        File bin = new File(root, DIR);
        File[] kids = bin.listFiles();
        if (kids == null) return out;
        for (File k : kids) {
            String n = k.getName();
            if (!n.endsWith(".json")) continue;
            File data = new File(bin, n.substring(0, n.length() - 5));
            if (!data.exists()) {                                // its item is gone: drop the stale note
                k.delete();
                continue;
            }
            try {
                byte[] b = new byte[(int) Math.min(k.length(), 1 << 16)];
                try (java.io.FileInputStream in = new java.io.FileInputStream(k)) {
                    int got = 0;
                    while (got < b.length) {
                        int r = in.read(b, got, b.length - got);
                        if (r < 0) break;
                        got += r;
                    }
                }
                JSONObject o = new JSONObject(new String(b, StandardCharsets.UTF_8));
                Entry e = new Entry();
                e.data = data;
                e.meta = k;
                e.root = root;
                e.name = o.optString("name", data.getName());
                e.orig = o.optString("orig", "");
                e.when = o.optLong("when", 0);
                e.dir = o.optBoolean("dir", data.isDirectory());
                out.add(e);
            } catch (Exception ignored) {
            }
        }
        Collections.sort(out, (a, b) -> Long.compare(b.when, a.when));
        return out;
    }

    /** Total bytes of a file or folder (links are not followed). */
    static long size(File f) {
        if (java.nio.file.Files.isSymbolicLink(f.toPath())) return 0;
        if (!f.isDirectory()) return f.length();
        long t = 0;
        File[] kids = f.listFiles();
        if (kids != null) for (File k : kids) t += size(k);
        return t;
    }

    // ---- take out

    /** Puts an item back where it came from (under a new name if that is taken). Returns the new path, or null on failure. */
    static File restore(Entry e) {
        File target = new File(e.orig);
        File parent = target.getParentFile();
        if (parent == null || (!parent.isDirectory() && !parent.mkdirs())) return null;
        if (target.exists() || java.nio.file.Files.isSymbolicLink(target.toPath())) {
            String n = e.name;
            int dot = e.dir ? -1 : n.lastIndexOf('.');
            String stem = dot > 0 ? n.substring(0, dot) : n, ext = dot > 0 ? n.substring(dot) : "";
            int i = 1;
            do {
                target = new File(parent, stem + (i == 1 ? " (restored)" : " (restored " + i + ")") + ext);
                i++;
            } while (target.exists());
        }
        if (!e.data.renameTo(target)) return null;
        e.meta.delete();
        return target;
    }

    /** Deletes an item for good. */
    static boolean purge(Entry e) {
        boolean ok = deleteTree(e.data);
        if (ok) e.meta.delete();
        return ok;
    }

    static boolean deleteTree(File f) {
        if (!java.nio.file.Files.isSymbolicLink(f.toPath()) && f.isDirectory()) {
            File[] kids = f.listFiles();
            if (kids != null) for (File k : kids) deleteTree(k);
        }
        return f.delete() || !f.exists();
    }
}
