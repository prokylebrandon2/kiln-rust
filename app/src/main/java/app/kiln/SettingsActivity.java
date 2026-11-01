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
        String fbName = scripts.optString("fallback", "");
        choiceRow(sr, "Main script", mainName.isEmpty() ? "Built-in v14" : mainName,
                "Used for everything. Applies the next time Kiln starts.", () -> scriptSheet("main", mainName));
        choiceRow(sr, "Fallback script", fbName.isEmpty() ? "None" : fbName,
                "Opens archives the main script can't read (browse, extract and check only).",
                () -> scriptSheet("fallback", fbName));

        heading("TOOLS");
        LinearLayout tl = card();
        choiceRow(tl, "Self-test", "Run", "Checks packing, extracting and encryption on this phone.",
                () -> runTool("selftest", null, "Self-test"));
        choiceRow(tl, "Benchmark a file", "Choose...", "Compares Kiln with zlib, bz2 and lzma.",
                () -> pickFile((f, n) -> runTool("bench", f.getPath(), "Benchmark")));
        choiceRow(tl, "WAV info", "Choose...", "Shows whether a .wav is float or integer audio.",
                () -> pickFile((f, n) -> runTool("wavcheck", f.getPath(), "WAV info")));

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
        Sheet sh = new Sheet(this, isMain ? "Main script" : "Fallback script");
        sh.text(isMain
                ? "Kiln's built-in v14 can't be removed. A replacement must provide every function the "
                + "app uses; it's checked when you choose it, and the app returns to v14 if it fails to load."
                : "Used only for archives the main script can't open. It can browse, extract and check "
                + "them, but not edit them.");
        sh.option(R.drawable.ic_open, "Choose a .py file...", false, () -> pickFile((f, n) -> installScript(role, f, n)));
        if (!current.isEmpty()) {
            sh.option(R.drawable.ic_close, isMain ? "Switch back to built-in v14" : "Remove the fallback", false, () -> {
                py("remove", "kiln_scripts", getFilesDir().getPath(), role);
                loadScripts();
                if (isMain) restartNote();
            });
        }
        sh.button("Cancel", Ui.TEXT, null).show();
    }

    private void installScript(final String role, final File f, final String name) {
        final Sheet busy = new Sheet(this, "Checking script...").cancelable(false);
        busy.show();
        new Thread(() -> {
            final String r = py("install", "kiln_scripts", getFilesDir().getPath(), role, f.getPath(), name);
            f.delete();
            runOnUiThread(() -> {
                busy.dismiss();
                try {
                    JSONObject o = new JSONObject(r);
                    if (o.optBoolean("ok")) {
                        Toast.makeText(this, "Using " + name + " (v" + o.optString("version") + ")",
                                Toast.LENGTH_SHORT).show();
                        loadScripts();
                        if (role.equals("main")) restartNote();
                    } else {
                        new Sheet(this, "Can't use this script").text(o.optString("message"))
                                .button("OK", Ui.FILLED, null).show();
                    }
                } catch (Exception e) {
                    new Sheet(this, "Can't use this script").text(r).button("OK", Ui.FILLED, null).show();
                }
            });
        }).start();
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

    private void runTool(final String tool, final String arg, final String title) {
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
            try {
                JSONObject o = new JSONObject(r);
                cancelled = o.optBoolean("cancelled");
                text = o.optString("text", o.optString("message", r));
            } catch (Exception e) {
                text = r;
            }
            final String shown = text;
            final boolean was = cancelled;
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
