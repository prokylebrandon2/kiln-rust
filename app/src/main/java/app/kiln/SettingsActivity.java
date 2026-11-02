package app.kiln;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.provider.OpenableColumns;
import android.database.Cursor;
import android.widget.Toast;

import com.chaquo.python.PyObject;
import com.chaquo.python.Python;

import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import android.os.Bundle;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.content.res.ColorStateList;
import android.text.TextUtils;
import android.widget.CompoundButton;
import android.widget.EditText;
import android.widget.ProgressBar;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Switch;
import android.widget.TextView;

import java.util.Map;

/** Settings: a few switches and the list of "always open with" choices. */
public class SettingsActivity extends Activity {
    private LinearLayout content;
    private JSONObject scripts = new JSONObject();
    private interface Picked { void got(File f, String name); }
    private Picked picked;
    private static final int PICK = 71;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);
        page.setBackgroundColor(Ui.bg(this));

        LinearLayout bar = new LinearLayout(this);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setPadding(Ui.dp(this, 4), Ui.dp(this, 6), Ui.dp(this, 8), Ui.dp(this, 6));
        bar.addView(Ui.iconButton(this, R.drawable.ic_up, v -> finish()),
                new LinearLayout.LayoutParams(Ui.dp(this, 48), Ui.dp(this, 48)));
        TextView t = Ui.label(this, "Settings", 19, true, false);
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tp.leftMargin = Ui.dp(this, 8);
        bar.addView(t, tp);
        page.addView(bar);
        View line = new View(this);
        line.setBackgroundColor(Ui.line(this));
        page.addView(line, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(this, 1)));

        ScrollView sv = new ScrollView(this);
        content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(Ui.dp(this, 16), Ui.dp(this, 12), Ui.dp(this, 16), Ui.dp(this, 32));
        sv.addView(content);
        page.addView(sv, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        setContentView(page);
        render();
    }

    private LinearLayout card() {
        LinearLayout c = new LinearLayout(this);
        c.setOrientation(LinearLayout.VERTICAL);
        c.setPadding(Ui.dp(this, 16), Ui.dp(this, 8), Ui.dp(this, 16), Ui.dp(this, 8));
        c.setBackground(Ui.shape(this, Ui.surface(this), Ui.line(this), 16));
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.bottomMargin = Ui.dp(this, 16);
        content.addView(c, lp);
        return c;
    }

    private void heading(String s) {
        TextView h = Ui.label(this, s, 13, true, true);
        h.setPadding(Ui.dp(this, 4), Ui.dp(this, 4), 0, Ui.dp(this, 8));
        content.addView(h);
    }

    private void toggle(LinearLayout parent, String title, String detail, boolean on,
                        CompoundButton.OnCheckedChangeListener l) {
        LinearLayout row = new LinearLayout(this);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(0, Ui.dp(this, 10), 0, Ui.dp(this, 10));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.addView(Ui.label(this, title, 16, false, false));
        if (detail != null) col.addView(Ui.label(this, detail, 12, false, true));
        row.addView(col, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        Switch sw = new Switch(this);
        sw.setChecked(on);
        sw.setOnCheckedChangeListener(l);
        row.addView(sw);
        parent.addView(row);
    }

    /** A row that shows the current choice and opens a modal with radio buttons when tapped. */
    private void choiceRow(LinearLayout parent, String title, String value, String detail, Runnable open) {
        LinearLayout row = new LinearLayout(this);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(0, Ui.dp(this, 10), 0, Ui.dp(this, 10));
        row.setBackground(Ui.pressable(this, 0, 0, 10));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.addView(Ui.label(this, title, 16, false, false));
        if (detail != null) col.addView(Ui.label(this, detail, 12, false, true));
        row.addView(col, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        TextView v = Ui.label(this, value, 14, true, false);
        v.setTextColor(Ui.accent(this));
        v.setPadding(Ui.dp(this, 10), 0, 0, 0);
        row.addView(v);
        row.setOnClickListener(x -> open.run());
        parent.addView(row);
    }

    private void render() {
        content.removeAllViews();
        heading("FILE LIST");
        LinearLayout a = card();
        toggle(a, "Show \"..\" row", "Tap it at the top of a list to go up a folder",
                Defaults.upRow(this), (b, on) -> Defaults.setUpRow(this, on));
        toggle(a, "Show previews", "Pictures, album art and video frames as icons",
                Defaults.previews(this), (b, on) -> Defaults.setPreviews(this, on));

        heading("ANIMATIONS");
        LinearLayout an = card();
        choiceRow(an, "Animation speed", Defaults.ANIM_NAMES[Defaults.animLevel(this)],
                "Off also turns off the slide between screens.", () -> {
                    Sheet sh = new Sheet(this, "Animation speed");
                    String[] det = {"No animations", null, "Default", null};
                    for (int i = 0; i < Defaults.ANIM_NAMES.length; i++) {
                        final int lv = i;
                        sh.radio(Defaults.ANIM_NAMES[i], det[i], i == Defaults.animLevel(this), () -> {
                            Defaults.setAnimLevel(this, lv);
                            render();
                        });
                    }
                    sh.button("Cancel", Ui.TEXT, null).show();
                });

        heading("SEARCH");
        LinearLayout sc = card();
        toggle(sc, "Search subfolders", "Search inside folders below the current one, not just the list",
                Defaults.deepSearch(this), (b, on) -> Defaults.setDeepSearch(this, on));
        choiceRow(sc, "Path shown in results", Defaults.PATH_NAMES[Defaults.searchPath(this)],
                "Relative is from the folder you searched in. Absolute starts at /", () -> {
                    Sheet sh = new Sheet(this, "Path shown in results");
                    String[] det = {"e.g. Docs/Notes", "e.g. /storage/emulated/0/Docs/Notes"};
                    for (int i = 0; i < Defaults.PATH_NAMES.length; i++) {
                        final int v = i;
                        sh.radio(Defaults.PATH_NAMES[i], det[i], i == Defaults.searchPath(this), () -> {
                            Defaults.setSearchPath(this, v);
                            render();
                        });
                    }
                    sh.button("Cancel", Ui.TEXT, null).show();
                });

        heading("SCRIPTS");
        LinearLayout sr = card();
        String mainName = scripts.optString("main", "");
        String mainMagic = scripts.optString("main_magic", "");
        choiceRow(sr, "Main script", mainName.isEmpty() ? "Built-in v14" : mainName,
                "Used for everything. Applies the next time Kiln starts."
                        + (mainMagic.isEmpty() ? "" : "\nRecognises files starting with " + mainMagic),
                () -> scriptSheet("main", mainName));

        heading("TOOLS");
        LinearLayout tl = card();
        choiceRow(tl, "Self-test", "Run", "Checks packing, extracting and encryption on this phone.",
                () -> runTool("selftest", null, "Self-test", ""));
        choiceRow(tl, "Benchmark a file", "Choose...", "Compares Kiln with zlib, bz2 and lzma.",
                () -> pickFile((f, n) -> runTool("bench", f.getPath(), "Benchmark", n)));
        choiceRow(tl, "WAV info", "Choose...", "Shows whether a .wav is float or integer audio.",
                () -> pickFile((f, n) -> runTool("wavcheck", f.getPath(), "WAV info", n)));

        heading("ADVANCED");
        LinearLayout ad = card();
        choiceRow(ad, "Parallel jobs", String.valueOf(Defaults.jobs(this)),
                "Worker processes when packing. 1 is safest; more is experimental and can crash the app.",
                () -> {
                    Sheet sh = new Sheet(this, "Parallel jobs");
                    int[] opts = {1, 2, 4};
                    for (final int o : opts) {
                        sh.radio(String.valueOf(o), o == 1 ? "Default" : "Experimental", o == Defaults.jobs(this), () -> {
                            Defaults.setJobs(this, o);
                            py("set_jobs", "kiln_app", o);
                            render();
                        });
                    }
                    sh.button("Cancel", Ui.TEXT, null).show();
                });

        heading("OPEN FILES WITH");
        LinearLayout d = card();
        Map<String, String> defs = Defaults.all(this);
        if (defs.isEmpty()) {
            TextView none = Ui.label(this, "No defaults yet. Choose \"Open with...\" on a file and "
                    + "tick \"Always open .ext files with this app\".", 14, false, true);
            none.setPadding(0, Ui.dp(this, 10), 0, Ui.dp(this, 10));
            d.addView(none);
        } else {
            for (final Map.Entry<String, String> en : defs.entrySet()) {
                LinearLayout row = new LinearLayout(this);
                row.setGravity(Gravity.CENTER_VERTICAL);
                row.setPadding(0, Ui.dp(this, 6), 0, Ui.dp(this, 6));
                LinearLayout col = new LinearLayout(this);
                col.setOrientation(LinearLayout.VERTICAL);
                col.addView(Ui.label(this, "." + en.getKey(), 16, true, false));
                col.addView(Ui.label(this, Defaults.labelOf(en.getValue()), 13, false, true));
                row.addView(col, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
                row.addView(Ui.button(this, "Reset", Ui.TEXT, v -> {
                    Defaults.clear(this, en.getKey());
                    render();
                }));
                d.addView(row);
            }
            LinearLayout.LayoutParams rp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            rp.topMargin = Ui.dp(this, 6);
            rp.bottomMargin = Ui.dp(this, 6);
            d.addView(Ui.button(this, "Reset all", Ui.OUTLINE, v -> {
                Defaults.clearAll(this);
                render();
            }), rp);
        }
    }

    // ---------------------------------------------------------------- scripts, tools, file picking

    private String py(String fn, String module, Object... args) {
        try {
            if (!Python.isStarted()) return "{\"ok\":false,\"message\":\"Kiln is still starting\"}";
            PyObject m = Python.getInstance().getModule(module);
            return m.callAttr(fn, args).toString();
        } catch (Throwable t) {
            return "{\"ok\":false,\"message\":" + JSONObject.quote(String.valueOf(t.getMessage())) + "}";
        }
    }

    private void loadScripts() {
        new Thread(() -> {
            try {
                final JSONObject o = new JSONObject(py("status", "kiln_scripts", getFilesDir().getPath()));
                runOnUiThread(() -> {
                    scripts = o;
                    render();
                });
            } catch (Exception ignored) {
            }
        }).start();
    }

    private void pickFile(Picked cb) {
        picked = cb;
        Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        i.addCategory(Intent.CATEGORY_OPENABLE);
        i.setType("*/*");
        startActivityForResult(i, PICK);
    }

    @Override
    protected void onActivityResult(int req, int res, Intent data) {
        super.onActivityResult(req, res, data);
        if (req != PICK || res != RESULT_OK || data == null || data.getData() == null || picked == null) return;
        final Uri uri = data.getData();
        final Picked cb = picked;
        picked = null;
        final Sheet busy = new Sheet(this, "Reading file...").cancelable(false);
        busy.show();
        new Thread(() -> {
            File out = null;
            String err = null;
            String name = "file";
            try (Cursor c = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME},
                    null, null, null)) {
                if (c != null && c.moveToFirst()) name = c.getString(0);
            } catch (Exception ignored) {
            }
            name = name.replace('/', '_');
            try {
                File dir = new File(getCacheDir(), "pick");
                dir.mkdirs();
                out = new File(dir, name);
                try (InputStream in = getContentResolver().openInputStream(uri);
                     OutputStream os = new FileOutputStream(out)) {
                    if (in == null) throw new java.io.IOException("can't read that file");
                    byte[] buf = new byte[1 << 16];
                    int k;
                    while ((k = in.read(buf)) > 0) os.write(buf, 0, k);
                }
            } catch (Exception e) {
                err = String.valueOf(e.getMessage());
            }
            final File f = out;
            final String n = name, e = err;
            runOnUiThread(() -> {
                busy.dismiss();
                if (e != null) new Sheet(this, "Couldn't read the file").text(e).button("OK", Ui.FILLED, null).show();
                else cb.got(f, n);
            });
        }).start();
    }

    private void scriptSheet(final String role, String current) {
        final boolean isMain = role.equals("main");
        Sheet sh = new Sheet(this, "Main script");
        sh.text("Kiln's built-in v14 can't be removed. A replacement must provide every function the "
                + "app uses; it's checked when you choose it, and the app returns to v14 if it fails to load.");
        sh.option(R.drawable.ic_open, "Choose a .py file...", false,
                () -> pickFile((f, n) -> installScript(role, f, n, "")));
        if (!current.isEmpty()) {
            final String sig = scripts.optString(role + "_magic", "");
            sh.option(R.drawable.ic_edit, "Signature: " + (sig.isEmpty() ? "not set" : sig) + " (change)", false,
                    () -> changeMagic(role, sig, null));
            sh.option(R.drawable.ic_close, "Switch back to built-in v14", false, () -> {
                py("remove", "kiln_scripts", getFilesDir().getPath(), role);
                loadScripts();
                if (isMain) restartNote();
            });
        }
        sh.button("Cancel", Ui.TEXT, null).show();
    }

    private void installScript(final String role, final File f, final String name, final String magic) {
        final Sheet busy = new Sheet(this, "Checking script...").cancelable(false);
        busy.show();
        new Thread(() -> {
            final String r = py("install", "kiln_scripts", getFilesDir().getPath(), role, f.getPath(), name, magic);
            runOnUiThread(() -> {
                busy.dismiss();
                try {
                    JSONObject o = new JSONObject(r);
                    if (o.optBoolean("ok")) {
                        f.delete();
                        Toast.makeText(this, "Using " + name + " (v" + o.optString("version") + ")",
                                Toast.LENGTH_SHORT).show();
                        loadScripts();
                        if (role.equals("main")) restartNote();
                    } else if (o.optBoolean("need_magic")) {
                        // the script doesn't say which files it handles (or the typed signature was wrong)
                        askMagic(name, o.optString("message"), magic,
                                m -> installScript(role, f, name, m), f::delete);
                    } else {
                        f.delete();
                        new Sheet(this, "Can't use this script").text(o.optString("message"))
                                .button("OK", Ui.FILLED, null).show();
                    }
                } catch (Exception e) {
                    f.delete();
                    new Sheet(this, "Can't use this script").text(r).button("OK", Ui.FILLED, null).show();
                }
            });
        }).start();
    }

    /** Ask for the first bytes of the files a script handles. */
    private void askMagic(String name, String message, String initial,
                          final java.util.function.Consumer<String> ok, final Runnable cancel) {
        final EditText ed = Ui.field(this, "KLNA   or   0x4B4C4E41", false);
        ed.setText(initial == null ? "" : initial);
        ed.setSelection(ed.length());
        TextView hint = Ui.label(this, "The first bytes of the files this script reads, as text "
                + "(KLNA) or hex (0x4B4C4E41). Up to 16 bytes. The file list uses this to tell which "
                + "files are archives.", 12, false, true);
        LinearLayout.LayoutParams hp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        hp.topMargin = Ui.dp(this, 10);
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.addView(ed);
        box.addView(hint, hp);
        new Sheet(this, "Signature for " + name).keyboard()
                .text(message == null || message.isEmpty() ? "Which files does this script handle?" : message)
                .content(box)
                .button("Cancel", Ui.TEXT, () -> {
                    if (cancel != null) cancel.run();
                    return true;
                })
                .button("Use it", Ui.FILLED, () -> {
                    String t = ed.getText().toString().trim();
                    if (t.isEmpty()) {
                        ed.setError("Enter the signature");
                        return false;
                    }
                    ok.accept(t);
                    return true;
                }).show();
        ed.requestFocus();
    }

    /** Change the signature of the script that's already installed. */
    private void changeMagic(final String role, String current, String message) {
        askMagic("main script", message, current, m -> {
            new Thread(() -> {
                final String r = py("set_magic", "kiln_scripts", getFilesDir().getPath(), role, m);
                runOnUiThread(() -> {
                    try {
                        JSONObject o = new JSONObject(r);
                        if (o.optBoolean("ok")) {
                            loadScripts();
                            if (role.equals("main")) restartNote();
                        } else {
                            changeMagic(role, m, o.optString("message"));
                        }
                    } catch (Exception e) {
                        new Sheet(this, "Couldn't change it").text(r).button("OK", Ui.FILLED, null).show();
                    }
                });
            }).start();
        }, null);
    }

    // ---- nicer results for Benchmark and WAV info

    private TextView smallTag(String t) {
        TextView tag = Ui.label(this, t, 11, true, false);
        tag.setTextColor(Ui.accent(this));
        tag.setPadding(Ui.dp(this, 7), Ui.dp(this, 2), Ui.dp(this, 7), Ui.dp(this, 2));
        tag.setBackground(Ui.shape(this, Ui.bg(this), 0, 8));
        return tag;
    }

    private TextView subjectLine(String subject, String second) {
        TextView t = Ui.label(this, (subject.isEmpty() ? "" : subject + "\n") + second, 13, false, true);
        t.setLineSpacing(0, 1.15f);
        return t;
    }

    private void showBench(String title, String subject, JSONObject v) {
        org.json.JSONArray rows = v.optJSONArray("rows");
        if (rows == null) return;
        long min = Long.MAX_VALUE, max = 1;
        double fast = Double.MAX_VALUE, slow = 0;
        for (int i = 0; i < rows.length(); i++) {
            JSONObject r = rows.optJSONObject(i);
            min = Math.min(min, r.optLong("size"));
            max = Math.max(max, r.optLong("size"));
            fast = Math.min(fast, r.optDouble("secs"));
            slow = Math.max(slow, r.optDouble("secs"));
        }
        boolean showSmall = rows.length() > 1 && min < max;
        boolean showFast = rows.length() > 1 && slow - fast >= 0.05;
        int files = v.optInt("files", 1);
        LinearLayout list = new LinearLayout(this);
        list.setOrientation(LinearLayout.VERTICAL);
        list.setPadding(Ui.dp(this, 14), Ui.dp(this, 4), Ui.dp(this, 14), Ui.dp(this, 4));
        list.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 14));
        for (int i = 0; i < rows.length(); i++) {
            JSONObject r = rows.optJSONObject(i);
            boolean best = showSmall && r.optLong("size") == min;
            boolean quick = showFast && r.optDouble("secs") == fast;
            if (i > 0) list.addView(Ui.rule(this));
            LinearLayout item = new LinearLayout(this);
            item.setOrientation(LinearLayout.VERTICAL);
            item.setPadding(0, Ui.dp(this, 12), 0, Ui.dp(this, 12));
            LinearLayout top = new LinearLayout(this);
            top.setGravity(Gravity.CENTER_VERTICAL);
            top.addView(Ui.label(this, r.optString("name"), 15, true, false));
            if (best) {
                TextView tag = smallTag("Smallest");
                LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
                tp.leftMargin = Ui.dp(this, 8);
                top.addView(tag, tp);
            }
            if (quick) {
                TextView tag = smallTag("Fastest");
                LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
                tp.leftMargin = Ui.dp(this, 6);
                top.addView(tag, tp);
            }
            View gap = new View(this);
            top.addView(gap, new LinearLayout.LayoutParams(0, 1, 1f));
            TextView pct = Ui.label(this, String.format(java.util.Locale.US, "%.1f%%", r.optDouble("ratio")),
                    15, true, false);
            if (best) pct.setTextColor(Ui.accent(this));
            top.addView(pct);
            item.addView(top);
            LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(this, 8));
            bp.topMargin = Ui.dp(this, 8);
            item.addView(Ui.meter(this, (double) r.optLong("size") / max, best), bp);
            TextView sub = Ui.label(this, Fmt.size(r.optLong("size")) + "  \u00b7  "
                    + String.format(java.util.Locale.US, "%.1f s", r.optDouble("secs")), 12, false, true);
            LinearLayout.LayoutParams sp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            sp.topMargin = Ui.dp(this, 6);
            item.addView(sub, sp);
            list.addView(item);
        }
        TextView note = Ui.label(this, "Percentages are the packed size as a share of the input. "
                + "Bars compare the sizes against each other.", 12, false, true);
        new Sheet(this, title)
                .content(subjectLine(subject, "Input: " + v.optString("input")
                        + (files > 1 ? "  \u00b7  " + files + " files" : "")))
                .space(12).content(list).space(12).content(note).space(4)
                .button("Close", Ui.FILLED, null).show();
    }

    private static String rateText(int hz) {
        double k = hz / 1000.0;
        return (k == Math.floor(k) ? String.valueOf((int) k) : String.format(java.util.Locale.US, "%.1f", k)) + " kHz";
    }

    private static String lengthText(double secs) {
        if (secs < 60) return String.format(java.util.Locale.US, "%.1f s", secs);
        int t = (int) Math.round(secs);
        return t / 60 + ":" + (t % 60 < 10 ? "0" : "") + t % 60;
    }

    private void showWav(String title, String subject, JSONObject v) {
        boolean flt = "float".equals(v.optString("kind"));
        int ch = v.optInt("ch");
        LinearLayout row1 = new LinearLayout(this);
        row1.addView(Ui.statTile(this, (flt ? "Float " : "PCM ") + v.optInt("bits") + "-bit", "format"),
                Ui.tileLp(this, false, true));
        row1.addView(Ui.statTile(this, rateText(v.optInt("rate")), "sample rate"), Ui.tileLp(this, true, false));
        LinearLayout row2 = new LinearLayout(this);
        row2.addView(Ui.statTile(this, ch == 1 ? "Mono" : ch == 2 ? "Stereo" : ch + " channels", "channels"),
                Ui.tileLp(this, false, true));
        row2.addView(Ui.statTile(this, lengthText(v.optDouble("secs")), "length"), Ui.tileLp(this, true, false));

        Sheet sh = new Sheet(this, title);
        if (!subject.isEmpty()) {
            TextView t = Ui.label(this, subject, 13, false, true);
            t.setSingleLine(true);
            t.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            sh.content(t).space(10);
        }
        sh.content(row1).space(8).content(row2);

        String verdict = v.optString("verdict");
        if (!verdict.isEmpty()) {
            LinearLayout box = new LinearLayout(this);
            box.setOrientation(LinearLayout.VERTICAL);
            box.setPadding(Ui.dp(this, 14), Ui.dp(this, 12), Ui.dp(this, 14), Ui.dp(this, 12));
            box.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 14));
            TextView h = Ui.label(this, "VERDICT", 11, true, true);
            box.addView(h);
            TextView t = Ui.label(this, verdict, 14, true, false);
            if (v.optBoolean("good")) t.setTextColor(Ui.accent(this));
            LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            tp.topMargin = Ui.dp(this, 4);
            box.addView(t, tp);
            String note = v.optString("note");
            if (!note.isEmpty()) {
                TextView n = Ui.label(this, note, 12, false, true);
                LinearLayout.LayoutParams np = new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
                np.topMargin = Ui.dp(this, 6);
                box.addView(n, np);
            }
            sh.space(14).content(box);
        }

        org.json.JSONArray det = v.optJSONArray("details");
        if (det != null && det.length() > 0) {
            LinearLayout box = new LinearLayout(this);
            box.setOrientation(LinearLayout.VERTICAL);
            box.setPadding(Ui.dp(this, 14), Ui.dp(this, 6), Ui.dp(this, 14), Ui.dp(this, 6));
            box.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 14));
            for (int i = 0; i < det.length(); i++) {
                if (i > 0) box.addView(Ui.rule(this));
                TextView t = Ui.label(this, det.optString(i), 13, false, false);
                t.setPadding(0, Ui.dp(this, 9), 0, Ui.dp(this, 9));
                box.addView(t);
            }
            sh.space(14).content(Ui.label(this, "ANALYSIS", 11, true, true)).space(6).content(box);
        }

        String filter = v.optString("filter");
        if (!filter.isEmpty()) {
            java.util.List<String[]> rows = new java.util.ArrayList<>();
            rows.add(new String[]{"Filter kiln will use", filter, "stack"});
            sh.space(14).content(Ui.detailCard(this, rows));
        }
        sh.space(4).button("Close", Ui.FILLED, null).show();
    }

    private void restartNote() {
        new Sheet(this, "Restart Kiln").text("The main script changes the next time Kiln starts. "
                + "Close Kiln from the recent apps list and open it again.")
                .button("OK", Ui.FILLED, null).show();
    }

    /** Python reports tool progress here and polls for cancellation. Public so Python can call it. */
    public static class ToolProgress {
        private final SettingsActivity owner;
        private volatile boolean cancel;

        ToolProgress(SettingsActivity owner) { this.owner = owner; }

        public void event(String json) { owner.onToolProgress(json); }

        public boolean cancelled() { return cancel; }

        void cancel() { cancel = true; }
    }

    private ToolProgress toolP;
    private ProgressBar toolBar;
    private TextView toolLine;

    private void onToolProgress(String json) {
        try {
            JSONObject o = new JSONObject(json);
            final double frac = o.optDouble("frac", -1);
            final String file = o.optString("file", "");
            runOnUiThread(() -> {
                if (toolBar == null) return;
                if (!file.isEmpty() && toolLine != null) toolLine.setText(file);
                if (frac < 0) {
                    toolBar.setIndeterminate(true);
                } else {
                    toolBar.setIndeterminate(false);
                    toolBar.setProgress((int) Math.round(frac * 1000));
                }
            });
        } catch (Exception ignored) {
        }
    }

    private void runTool(final String tool, final String arg, final String title, final String subject) {
        final ToolProgress tp = new ToolProgress(this);
        toolP = tp;
        toolLine = Ui.label(this, tool.equals("selftest") ? "Starting..." : "Working...", 13, false, true);
        toolLine.setSingleLine(true);
        toolLine.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        toolBar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        toolBar.setIndeterminate(true);
        toolBar.setMax(1000);
        ColorStateList acc = ColorStateList.valueOf(Ui.accent(this));
        toolBar.setProgressTintList(acc);
        toolBar.setIndeterminateTintList(acc);
        TextView hint = Ui.label(this, tool.equals("selftest")
                ? "This can take a minute or two. Cancel stops it after the current step."
                : "Cancel stops it after the current step.", 12, false, true);
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.addView(toolLine);
        LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        bp.topMargin = Ui.dp(this, 12);
        box.addView(toolBar, bp);
        LinearLayout.LayoutParams hp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        hp.topMargin = Ui.dp(this, 12);
        box.addView(hint, hp);
        final Sheet busy = new Sheet(this, title).content(box).cancelable(false);
        busy.button("Cancel", Ui.OUTLINE, () -> {
            tp.cancel();
            toolLine.setText("Cancelling...");
            busy.lastButton().setEnabled(false);
            return false;
        });
        busy.show();
        new Thread(() -> {
            String r = py("tool", "kiln_app", tool, arg == null ? "" : arg, tp);
            boolean cancelled = false;
            String text;
            JSONObject viewObj = null;
            try {
                JSONObject o = new JSONObject(r);
                cancelled = o.optBoolean("cancelled");
                text = o.optString("text", o.optString("message", r));
                viewObj = o.optJSONObject("view");
            } catch (Exception e) {
                text = r;
            }
            final String shown = text;
            final boolean was = cancelled;
            final JSONObject view = viewObj;
            runOnUiThread(() -> {
                toolBar = null;
                toolLine = null;
                toolP = null;
                if (isFinishing() || isDestroyed()) return;
                busy.dismiss();
                if (was) {
                    Toast.makeText(this, title + " cancelled", Toast.LENGTH_SHORT).show();
                    return;
                }
                if (view != null && "bench".equals(view.optString("type"))) {
                    showBench(title, subject, view);
                    return;
                }
                if (view != null && "wav".equals(view.optString("type"))) {
                    showWav(title, subject, view);
                    return;
                }
                if (tool.equals("wavcheck")) {                   // not a WAV it can describe: say so plainly
                    new Sheet(this, title).text((subject.isEmpty() ? "" : subject + "\n\n") + shown)
                            .button("Close", Ui.FILLED, null).show();
                    return;
                }
                TextView tv = Ui.label(this, shown, 12, false, false);
                tv.setTypeface(android.graphics.Typeface.MONOSPACE);
                tv.setTextIsSelectable(true);
                new Sheet(this, title).content(tv).button("Close", Ui.FILLED, null).show();
            });
        }).start();
    }

    @Override
    protected void onDestroy() {
        if (toolP != null) toolP.cancel();      // leaving Settings stops a running tool
        super.onDestroy();
    }

    @Override
    protected void onResume() {
        super.onResume();
        loadScripts();
    }

    @Override
    public void finish() {
        super.finish();
        Ui.noSlideIfOff(this);
    }
}
