package app.kiln;

import android.content.Context;
import android.content.res.ColorStateList;
import android.content.res.Configuration;
import android.graphics.Typeface;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.List;

/**
 * Pieces for the result screens of Generate SHA-256, Verify SHA-256 and Compare: a status header,
 * a row of number tiles, a match bar and lists of names grouped under a coloured heading. They are
 * plain views that go into a Sheet, so every result looks like the rest of the app.
 */
final class Results {
    private Results() {}

    static final int GOOD = 0, BAD = 1, WARN = 2, NEUTRAL = 3;
    /** How many names a list shows before it says "and N more". */
    private static final int SHOWN = 12;

    private static boolean night(Context c) {
        return (c.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK)
                == Configuration.UI_MODE_NIGHT_YES;
    }

    static int tone(Context c, int kind) {
        switch (kind) {
            case GOOD: return night(c) ? 0xFF66BB6A : 0xFF2E7D32;
            case BAD: return c.getColor(R.color.k_danger);
            case WARN: return night(c) ? 0xFFFFB74D : 0xFFB26A00;
            default: return Ui.accent(c);
        }
    }

    private static LinearLayout card(Context c) {
        LinearLayout card = new LinearLayout(c);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(Ui.dp(c, 14), Ui.dp(c, 4), Ui.dp(c, 14), Ui.dp(c, 4));
        card.setBackground(Ui.shape(c, c.getColor(R.color.k_chip), 0, 14));
        return card;
    }

    /** A round status icon (tick or cross) beside a headline and a line under it. */
    static View header(Context c, int kind, String title, String sub) {
        LinearLayout h = new LinearLayout(c);
        h.setGravity(Gravity.CENTER_VERTICAL);
        int col = tone(c, kind);
        ImageView ic = new ImageView(c);
        ic.setImageResource(kind == BAD ? R.drawable.ic_close : R.drawable.ic_check);
        Ui.tint(ic, col);
        int p = Ui.dp(c, 14);
        ic.setPadding(p, p, p, p);
        ic.setBackground(Ui.shape(c, Ui.alpha(col, 0x2A), 0, 27));
        h.addView(ic, new LinearLayout.LayoutParams(Ui.dp(c, 54), Ui.dp(c, 54)));
        LinearLayout t = new LinearLayout(c);
        t.setOrientation(LinearLayout.VERTICAL);
        TextView a = Ui.label(c, title, 18, true, false);
        t.addView(a);
        if (sub != null && !sub.isEmpty()) {
            TextView b = Ui.label(c, sub, 13, false, true);
            b.setMaxLines(2);
            b.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            t.addView(b);
        }
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tp.leftMargin = Ui.dp(c, 14);
        h.addView(t, tp);
        return h;
    }

    /** A row of equal tiles. Each entry is {big, small}; tones[i] is a kind, or -1 for plain. */
    static View tiles(Context c, String[][] items, int[] tones) {
        LinearLayout row = new LinearLayout(c);
        for (int i = 0; i < items.length; i++) {
            View t = Ui.statTile(c, items[i][0], items[i][1]);
            if (tones != null && i < tones.length && tones[i] >= 0 && t instanceof LinearLayout) {
                View first = ((LinearLayout) t).getChildAt(0);
                if (first instanceof TextView) ((TextView) first).setTextColor(tone(c, tones[i]));
            }
            row.addView(t, Ui.tileLp(c, i > 0, i < items.length - 1));
        }
        return row;
    }

    /** A labelled bar: a caption on the left, a value on the right, the bar beneath. frac is 0..1. */
    static View bar(Context c, double frac, int kind, String left, String right) {
        LinearLayout box = new LinearLayout(c);
        box.setOrientation(LinearLayout.VERTICAL);
        LinearLayout top = new LinearLayout(c);
        top.addView(Ui.label(c, left, 13, false, true),
                new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        TextView r = Ui.label(c, right, 13, true, false);
        r.setTextColor(tone(c, kind));
        top.addView(r);
        box.addView(top);
        ProgressBar pb = Ui.meter(c, frac, true);
        pb.setProgressTintList(ColorStateList.valueOf(tone(c, kind)));
        LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(c, 8));
        bp.topMargin = Ui.dp(c, 8);
        box.addView(pb, bp);
        return box;
    }

    private static View badge(Context c, String letter) {
        TextView t = Ui.label(c, letter, 13, true, false);
        t.setGravity(Gravity.CENTER);
        t.setTextColor(Ui.accent(c));
        t.setBackground(Ui.shape(c, Ui.alpha(Ui.accent(c), 0x26), 0, 14));
        t.setLayoutParams(new LinearLayout.LayoutParams(Ui.dp(c, 28), Ui.dp(c, 28)));
        return t;
    }

    private static View partyRow(Context c, String letter, String name, String sub) {
        LinearLayout r = new LinearLayout(c);
        r.setGravity(Gravity.CENTER_VERTICAL);
        r.setPadding(0, Ui.dp(c, 10), 0, Ui.dp(c, 10));
        r.addView(badge(c, letter));
        LinearLayout t = new LinearLayout(c);
        t.setOrientation(LinearLayout.VERTICAL);
        TextView n = Ui.label(c, name, 14, true, false);
        n.setSingleLine(true);
        n.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        t.addView(n);
        if (sub != null && !sub.isEmpty()) t.addView(Ui.label(c, sub, 12, false, true));
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tp.leftMargin = Ui.dp(c, 12);
        r.addView(t, tp);
        return r;
    }

    /** The two things being compared, labelled A and B. */
    static View pair(Context c, String nameA, String subA, String nameB, String subB) {
        LinearLayout card = card(c);
        card.addView(partyRow(c, "A", nameA, subA));
        card.addView(Ui.rule(c));
        card.addView(partyRow(c, "B", nameB, subB));
        return card;
    }

    private static View nameRow(Context c, String path, boolean folder) {
        while (path.length() > 1 && path.endsWith("/")) path = path.substring(0, path.length() - 1);
        int cut = path.lastIndexOf('/');
        String nm = cut >= 0 ? path.substring(cut + 1) : path;
        String dir = cut > 0 ? path.substring(0, cut) : "";
        LinearLayout r = new LinearLayout(c);
        r.setGravity(Gravity.CENTER_VERTICAL);
        r.setPadding(0, Ui.dp(c, 9), 0, Ui.dp(c, 9));
        ImageView ic = new ImageView(c);
        ic.setImageResource(folder ? R.drawable.ic_folder : R.drawable.ic_file);
        Ui.tint(ic, Ui.text2(c));
        r.addView(ic, new LinearLayout.LayoutParams(Ui.dp(c, 20), Ui.dp(c, 20)));
        LinearLayout t = new LinearLayout(c);
        t.setOrientation(LinearLayout.VERTICAL);
        TextView a = Ui.label(c, nm, 14, true, false);
        a.setSingleLine(true);
        a.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        t.addView(a);
        if (!dir.isEmpty()) {
            TextView b = Ui.label(c, dir, 12, false, true);
            b.setSingleLine(true);
            b.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            t.addView(b);
        }
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tp.leftMargin = Ui.dp(c, 12);
        r.addView(t, tp);
        return r;
    }

    /** A card with a coloured dot, a heading, a count and the names under it. Null when empty. */
    static View section(Context c, int kind, String title, JSONArray names, int total, boolean folders) {
        if (total <= 0) return null;
        LinearLayout card = card(c);
        LinearLayout head = new LinearLayout(c);
        head.setGravity(Gravity.CENTER_VERTICAL);
        head.setPadding(0, Ui.dp(c, 10), 0, Ui.dp(c, 10));
        View dot = new View(c);
        dot.setBackground(Ui.shape(c, tone(c, kind), 0, 5));
        head.addView(dot, new LinearLayout.LayoutParams(Ui.dp(c, 10), Ui.dp(c, 10)));
        TextView t = Ui.label(c, title, 14, true, false);
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tp.leftMargin = Ui.dp(c, 10);
        head.addView(t, tp);
        TextView n = Ui.label(c, String.valueOf(total), 12, true, false);
        n.setTextColor(tone(c, kind));
        n.setGravity(Gravity.CENTER);
        n.setPadding(Ui.dp(c, 9), Ui.dp(c, 2), Ui.dp(c, 9), Ui.dp(c, 2));
        n.setBackground(Ui.shape(c, Ui.alpha(tone(c, kind), 0x2A), 0, 10));
        head.addView(n);
        card.addView(head);
        int shown = 0;
        for (int i = 0; names != null && i < names.length() && shown < SHOWN; i++) {
            String s = names.optString(i);
            if (s.isEmpty()) continue;
            card.addView(Ui.rule(c));
            card.addView(nameRow(c, s, folders));
            shown++;
        }
        if (total > shown) {
            card.addView(Ui.rule(c));
            TextView more = Ui.label(c, "and " + (total - shown) + " more", 13, false, true);
            more.setPadding(0, Ui.dp(c, 10), 0, Ui.dp(c, 10));
            card.addView(more);
        }
        return card;
    }

    /** The same card for plain names (a Java list), such as files about to be replaced. */
    static View section(Context c, int kind, String title, List<String> names, boolean folders) {
        JSONArray a = new JSONArray();
        for (String s : names) a.put(s);
        return section(c, kind, title, a, names.size(), folders);
    }

    /** One finished checksum: the file it wrote and either its hash or how many files it covers. */
    static View made(Context c, JSONObject o, String filesText) {
        LinearLayout card = card(c);
        card.addView(nameRow(c, o.optString("name"), false));
        if (o.has("hash")) {
            card.addView(Ui.rule(c));
            TextView k = Ui.label(c, "SHA-256", 12, false, true);
            k.setPadding(0, Ui.dp(c, 9), 0, 0);
            card.addView(k);
            TextView h = Ui.label(c, o.optString("hash"), 12, false, false);
            h.setTypeface(Typeface.MONOSPACE);
            h.setTextIsSelectable(true);
            h.setPadding(0, Ui.dp(c, 2), 0, Ui.dp(c, 10));
            card.addView(h);
        } else if (filesText != null) {
            card.addView(Ui.rule(c));
            TextView k = Ui.label(c, filesText, 13, false, true);
            k.setPadding(0, Ui.dp(c, 9), 0, Ui.dp(c, 10));
            card.addView(k);
        }
        return card;
    }

    /** A small grey line of explanation. */
    static View note(Context c, String text) {
        TextView t = Ui.label(c, text, 12, false, true);
        t.setLineSpacing(0, 1.1f);
        return t;
    }

    /** Adds a view to a sheet with a gap after it; null is skipped. */
    static void add(Sheet s, View v, int gapDp) {
        if (v == null) return;
        s.content(v).space(gapDp);
    }
}
