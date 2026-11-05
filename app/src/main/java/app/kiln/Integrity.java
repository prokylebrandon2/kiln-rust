package app.kiln;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * SHA-256 checksum files and comparisons. A file's checksum file is exactly what
 * {@code sha256sum name > name.sha256} writes. A folder gets one checksum file, {@code Folder.sha256},
 * beside it: a comment header, then one {@code hash  relative/path} line per file, sorted by path, so
 * the same folder always gives the same text. Renaming, moving, changing, adding or removing a file
 * changes it. Links aren't followed (they're listed as skipped) and empty folders aren't recorded.
 * Every method returns a JSON result and never throws.
 */
final class Integrity {
    private Integrity() {}

    static final String EXT = ".sha256";
    static final String FOLDER_HEADER = "# SHA256-MANIFEST-V1 folder";
    private static final Pattern LINE = Pattern.compile("^\\\\?([0-9a-fA-F]{64}) [ *](.+)$");

    /** Called now and then while working; return false to cancel. frac is -1 when unknown. */
    interface Tick { boolean go(double frac, String name); }

    private static final class Cancelled extends IOException {
        Cancelled() { super("Cancelled"); }
    }

    private static final class Meter {
        final Tick tick;
        long total, done, last = System.nanoTime() - 1_000_000_000L;

        Meter(Tick t) { tick = t; }

        void add(long n, String name) throws Cancelled {
            done += n;
            long now = System.nanoTime();
            if (now - last < 100_000_000L) return;
            last = now;
            if (tick != null && !tick.go(total > 0 ? Math.min(1.0, (double) done / total) : -1, name)) {
                throw new Cancelled();
            }
        }
    }

    // ------------------------------------------------------------------ helpers

    private static MessageDigest sha() {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (Exception e) {
            throw new IllegalStateException(e);
        }
    }

    private static String hex(byte[] b) {
        StringBuilder s = new StringBuilder(b.length * 2);
        for (byte x : b) s.append(String.format(Locale.ROOT, "%02x", x));
        return s.toString();
    }

    private static JSONObject fail(Exception e) {
        JSONObject o = new JSONObject();
        try {
            if (e instanceof Cancelled) o.put("ok", false).put("cancelled", true);
            else o.put("ok", false).put("error", String.valueOf(e.getMessage()));
        } catch (Exception ignored) {
        }
        return o;
    }

    private static JSONObject fail(String msg) {
        return fail(new IOException(msg));
    }

    private static JSONArray arr(Iterable<String> l, int cap) {
        JSONArray a = new JSONArray();
        for (String s : l) {
            if (a.length() >= cap) break;
            a.put(s);
        }
        return a;
    }

    static File manifestOf(File f) {
        return new File(f.getParentFile(), f.getName() + EXT);
    }

    /** Every file below dir by relative path (sorted), every folder, and what had to be left out. */
    private static void walk(File dir, String rel, TreeMap<String, File> files, TreeSet<String> dirs,
                             List<String> skipped) {
        File[] kids = dir.listFiles();
        if (kids == null) {
            skipped.add((rel.isEmpty() ? dir.getName() : rel) + " (can't read folder)");
            return;
        }
        for (File k : kids) {
            String r = rel.isEmpty() ? k.getName() : rel + "/" + k.getName();
            if (Files.isSymbolicLink(k.toPath())) {
                skipped.add(r + " (link)");
            } else if (k.isDirectory()) {
                dirs.add(r);
                walk(k, r, files, dirs, skipped);
            } else if (k.isFile()) {
                if (k.getName().indexOf('\n') >= 0 || k.getName().indexOf('\r') >= 0) skipped.add(r + " (odd name)");
                else files.put(r, k);
            }
        }
    }

    private static String hashFile(File f, String shown, Meter m) throws IOException {
        MessageDigest md = sha();
        byte[] b = new byte[256 * 1024];
        try (InputStream in = new FileInputStream(f)) {
            int n;
            while ((n = in.read(b)) > 0) {
                md.update(b, 0, n);
                m.add(n, shown);
            }
        }
        m.add(0, shown);
        return hex(md.digest());
    }

    // ------------------------------------------------------------------ generate

    /** The first free name for item's checksum file: name.sha256, then "name (1).sha256", and so on. */
    static File uniqueManifestOf(File f) {
        File t = manifestOf(f);
        for (int i = 1; t.exists(); i++) t = new File(f.getParentFile(), f.getName() + " (" + i + ")" + EXT);
        return t;
    }

    /** Writes name.sha256 beside each item. Nothing is written unless every hash finished. */
    static JSONObject generate(List<File> items, Tick tick) { return generate(items, false, tick); }

    /** keepBoth: a checksum file that already exists is left alone and the new one gets a free name. */
    static JSONObject generate(List<File> items, boolean keepBoth, Tick tick) {
        try {
            Meter m = new Meter(tick);
            final class Plan {
                File item;
                TreeMap<String, File> files = new TreeMap<>();
                List<String> skipped = new ArrayList<>();
                String text, hash;
                int n;
            }
            List<Plan> plans = new ArrayList<>();
            for (File f : items) {
                Plan p = new Plan();
                p.item = f;
                if (f.isDirectory()) {
                    walk(f, "", p.files, new TreeSet<String>(), p.skipped);
                    for (File x : p.files.values()) m.total += x.length();
                } else {
                    p.files.put(f.getName(), f);
                    m.total += f.length();
                }
                plans.add(p);
            }
            List<String> failed = new ArrayList<>();
            List<Plan> good = new ArrayList<>();
            for (Plan p : plans) {
                boolean dir = p.item.isDirectory();
                StringBuilder text = new StringBuilder();
                if (dir) text.append(FOLDER_HEADER).append("\n# Paths are relative to the folder.\n");
                int n = 0;
                for (java.util.Map.Entry<String, File> e : p.files.entrySet()) {
                    try {
                        String h = hashFile(e.getValue(), e.getKey(), m);
                        text.append(h).append("  ").append(e.getKey()).append('\n');
                        p.hash = h;
                        n++;
                    } catch (Cancelled c) {
                        throw c;
                    } catch (IOException ex) {
                        p.skipped.add(e.getKey() + " (can't read)");
                    }
                }
                if (!dir && n == 0) {
                    failed.add(p.item.getName());
                    continue;
                }
                p.n = n;
                p.text = text.toString();
                if (dir) p.hash = null;
                good.add(p);
            }
            if (good.isEmpty()) return fail("Couldn't read " + (failed.isEmpty() ? "anything" : failed.get(0)));
            JSONArray made = new JSONArray();
            List<String> skipped = new ArrayList<>();
            for (Plan p : good) {
                File target = keepBoth ? uniqueManifestOf(p.item) : manifestOf(p.item);
                File tmp = new File(target.getParentFile(), target.getName() + ".tmp");
                Files.write(tmp.toPath(), p.text.getBytes(StandardCharsets.UTF_8));
                Files.move(tmp.toPath(), target.toPath(), StandardCopyOption.REPLACE_EXISTING);
                JSONObject o = new JSONObject().put("name", target.getName()).put("folder", p.item.isDirectory())
                        .put("files", p.n);
                if (p.hash != null) o.put("hash", p.hash);
                made.put(o);
                skipped.addAll(p.skipped);
            }
            return new JSONObject().put("ok", true).put("made", made).put("skipped", arr(skipped, 50))
                    .put("skippedN", skipped.size()).put("failed", arr(failed, 20));
        } catch (Exception e) {
            return fail(e);
        }
    }

    // ------------------------------------------------------------------ verify

    /** Checks the files a checksum file lists. A folder checksum file also reports files it doesn't list. */
    static JSONObject verify(File manifest, Tick tick) {
        try {
            String name = manifest.getName();
            boolean folder = false;
            int malformed = 0;
            List<String[]> ents = new ArrayList<>();
            for (String l : Files.readAllLines(manifest.toPath(), StandardCharsets.UTF_8)) {
                if (l.endsWith("\r")) l = l.substring(0, l.length() - 1);
                if (l.startsWith(FOLDER_HEADER)) folder = true;
                if (l.isEmpty() || l.startsWith("#")) continue;
                Matcher mt = LINE.matcher(l);
                if (mt.matches()) ents.add(new String[]{mt.group(1).toLowerCase(Locale.ROOT), mt.group(2)});
                else malformed++;
            }
            File parent = manifest.getParentFile();
            File base = parent;
            if (folder) {
                String stem = name.endsWith(EXT) ? name.substring(0, name.length() - EXT.length()) : name;
                base = new File(parent, stem);
                if (!base.isDirectory()) return fail("The folder \"" + stem + "\" isn't next to this checksum file.");
            } else if (ents.isEmpty()) {
                return fail("No checksums found in this file.");
            }
            Meter m = new Meter(tick);
            for (String[] e : ents) {
                File f = new File(base, e[1]);
                if (f.isFile()) m.total += f.length();
            }
            int ok = 0;
            List<String> changed = new ArrayList<>(), missing = new ArrayList<>(), added = new ArrayList<>();
            Set<String> listed = new HashSet<>();
            for (String[] e : ents) {
                listed.add(e[1]);
                File f = new File(base, e[1]);
                if (!f.isFile()) {
                    missing.add(e[1]);
                    continue;
                }
                try {
                    if (hashFile(f, e[1], m).equals(e[0])) ok++;
                    else changed.add(e[1]);
                } catch (Cancelled c) {
                    throw c;
                } catch (IOException ex) {
                    missing.add(e[1] + " (can't read)");
                }
            }
            if (folder) {
                TreeMap<String, File> now = new TreeMap<>();
                walk(base, "", now, new TreeSet<String>(), new ArrayList<String>());
                for (String k : now.keySet()) if (!listed.contains(k)) added.add(k);
            }
            boolean match = changed.isEmpty() && missing.isEmpty() && added.isEmpty() && malformed == 0;
            return new JSONObject().put("ok", true).put("match", match).put("manifest", name)
                    .put("folder", folder).put("checked", ents.size()).put("good", ok)
                    .put("changed", arr(changed, 50)).put("changedN", changed.size())
                    .put("missing", arr(missing, 50)).put("missingN", missing.size())
                    .put("added", arr(added, 50)).put("addedN", added.size()).put("malformed", malformed);
        } catch (Exception e) {
            return fail(e);
        }
    }

    // ------------------------------------------------------------------ compare

    private static int fill(InputStream in, byte[] buf) throws IOException {
        int n = 0;
        while (n < buf.length) {
            int r = in.read(buf, n, buf.length - n);
            if (r < 0) break;
            n += r;
        }
        return n;
    }

    /** Position of the first byte that differs, or -1 when the files are the same. */
    private static long firstDiff(File a, File b, MessageDigest md, String shown, Meter m) throws IOException {
        byte[] x = new byte[128 * 1024], y = new byte[128 * 1024];
        long pos = 0;
        try (InputStream ia = new FileInputStream(a); InputStream ib = new FileInputStream(b)) {
            while (true) {
                int na = fill(ia, x), nb = fill(ib, y);
                if (md != null && na > 0) md.update(x, 0, na);
                if (na != nb) return pos + Math.min(na, nb);
                for (int i = 0; i < na; i++) if (x[i] != y[i]) return pos + i;
                if (na <= 0) return -1;
                pos += na;
                m.add(na, shown);
            }
        }
    }

    static JSONObject compareFiles(File a, File b, Tick tick) {
        try {
            long sa = a.length(), sb = b.length();
            JSONObject o = new JSONObject().put("ok", true).put("kind", "files").put("sizeA", sa).put("sizeB", sb);
            if (sa != sb) return o.put("same", false).put("reason", "size");
            Meter m = new Meter(tick);
            m.total = sa;
            MessageDigest md = sha();
            long d = firstDiff(a, b, md, a.getName(), m);
            if (d >= 0) return o.put("same", false).put("reason", "content").put("at", d);
            return o.put("same", true).put("hash", hex(md.digest()));
        } catch (Exception e) {
            return fail(e);
        }
    }

    static JSONObject compareDirs(File a, File b, Tick tick) {
        try {
            TreeMap<String, File> fa = new TreeMap<>(), fb = new TreeMap<>();
            TreeSet<String> da = new TreeSet<>(), db = new TreeSet<>();
            List<String> skipA = new ArrayList<>(), skipB = new ArrayList<>();
            walk(a, "", fa, da, skipA);
            walk(b, "", fb, db, skipB);
            List<String> onlyA = new ArrayList<>(), onlyB = new ArrayList<>(), changed = new ArrayList<>();
            List<String> dirsOnlyA = new ArrayList<>(), dirsOnlyB = new ArrayList<>(), unreadable = new ArrayList<>();
            List<String> both = new ArrayList<>();
            Meter m = new Meter(tick);
            for (String k : fa.keySet()) {
                if (fb.containsKey(k)) {
                    both.add(k);
                    if (fa.get(k).length() == fb.get(k).length()) m.total += fa.get(k).length();
                } else if (db.contains(k)) changed.add(k + " (file here, folder there)");
                else onlyA.add(k);
            }
            for (String k : fb.keySet()) {
                if (fa.containsKey(k)) continue;
                if (da.contains(k)) changed.add(k + " (folder here, file there)");
                else onlyB.add(k);
            }
            for (String k : da) if (!db.contains(k) && !fb.containsKey(k)) dirsOnlyA.add(k);
            for (String k : db) if (!da.contains(k) && !fa.containsKey(k)) dirsOnlyB.add(k);
            int same = 0;
            for (String k : both) {
                File x = fa.get(k), y = fb.get(k);
                if (x.length() != y.length()) {
                    changed.add(k);
                    continue;
                }
                try {
                    if (firstDiff(x, y, null, k, m) >= 0) changed.add(k);
                    else same++;
                } catch (Cancelled c) {
                    throw c;
                } catch (IOException ex) {
                    unreadable.add(k);
                }
            }
            boolean identical = onlyA.isEmpty() && onlyB.isEmpty() && changed.isEmpty() && dirsOnlyA.isEmpty()
                    && dirsOnlyB.isEmpty() && unreadable.isEmpty();
            return new JSONObject().put("ok", true).put("kind", "dirs").put("same", identical)
                    .put("compared", fa.size() + onlyB.size()).put("identical", same)
                    .put("changed", arr(changed, 50)).put("changedN", changed.size())
                    .put("onlyA", arr(onlyA, 50)).put("onlyAN", onlyA.size())
                    .put("onlyB", arr(onlyB, 50)).put("onlyBN", onlyB.size())
                    .put("dirsOnlyA", arr(dirsOnlyA, 50)).put("dirsOnlyAN", dirsOnlyA.size())
                    .put("dirsOnlyB", arr(dirsOnlyB, 50)).put("dirsOnlyBN", dirsOnlyB.size())
                    .put("unreadable", arr(unreadable, 50)).put("unreadableN", unreadable.size())
                    .put("skipped", skipA.size() + skipB.size());
        } catch (Exception e) {
            return fail(e);
        }
    }
}
