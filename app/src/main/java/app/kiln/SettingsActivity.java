package app.kiln;

import android.app.Activity;
import android.os.Bundle;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.CompoundButton;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Switch;
import android.widget.TextView;

import java.util.Map;

/** Settings: a few switches and the list of "always open with" choices. */
public class SettingsActivity extends Activity {
    private LinearLayout content;

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

    private void render() {
        content.removeAllViews();
        heading("FILE LIST");
        LinearLayout a = card();
        toggle(a, "Show \"..\" row", "Tap it at the top of a list to go up a folder",
                Defaults.upRow(this), (b, on) -> Defaults.setUpRow(this, on));
        toggle(a, "Show previews", "Pictures, album art and video frames as icons",
                Defaults.previews(this), (b, on) -> Defaults.setPreviews(this, on));

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
}
