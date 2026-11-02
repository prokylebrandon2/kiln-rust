package app.kiln;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.graphics.drawable.StateListDrawable;
import android.text.InputType;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.ImageButton;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;

/** Flat, quiet building blocks shared by every screen. Colors come from res/values(-night). */
final class Ui {
    private Ui() {}

    static final int FILLED = 0, OUTLINE = 1, TEXT = 2, DANGER = 3, TONAL = 4;

    static int dp(Context c, float v) {
        return Math.round(v * c.getResources().getDisplayMetrics().density);
    }

    static int bg(Context c) { return c.getColor(R.color.k_bg); }
    static int surface(Context c) { return c.getColor(R.color.k_surface); }
    static int text(Context c) { return c.getColor(R.color.k_text); }
    static int text2(Context c) { return c.getColor(R.color.k_text2); }
    static int line(Context c) { return c.getColor(R.color.k_line); }
    static int accent(Context c) { return c.getColor(R.color.k_accent); }

    static GradientDrawable shape(Context c, int fill, int stroke, float radiusDp) {
        GradientDrawable g = new GradientDrawable();
        g.setColor(fill);
        g.setCornerRadius(dp(c, radiusDp));
        if (stroke != 0) g.setStroke(dp(c, 1), stroke);
        return g;
    }

    static int alpha(int color, int a) { return (a << 24) | (color & 0xFFFFFF); }

    /** A background that lights up with a ripple where the finger touches it. */
    static Drawable pressable(Context c, int fill, int stroke, float radiusDp) {
        boolean solid = fill != 0 && (fill >>> 24) != 0;
        int rc = solid ? alpha(c.getColor(R.color.k_on_accent), 0x4D) : alpha(text(c), 0x26);
        return ripple(c, fill, stroke, radiusDp, rc);
    }

    static Drawable ripple(Context c, int fill, int stroke, float radiusDp, int rippleColor) {
        StateListDrawable content = new StateListDrawable();
        content.addState(new int[]{-android.R.attr.state_enabled},
                shape(c, fill == 0 ? 0 : fade(fill), stroke == 0 ? 0 : fade(stroke), radiusDp));
        content.addState(new int[]{}, shape(c, fill, stroke, radiusDp));
        GradientDrawable mask = new GradientDrawable();
        mask.setColor(0xFF000000);
        mask.setCornerRadius(dp(c, radiusDp));
        return new RippleDrawable(ColorStateList.valueOf(rippleColor), content, mask);
    }

    /** A little squash while the finger is down. Doesn't consume the touch. */
    static void pressScale(View v) {
        v.setOnTouchListener((view, e) -> {
            int a = e.getActionMasked();
            if (a == MotionEvent.ACTION_DOWN) {
                view.animate().scaleX(0.96f).scaleY(0.96f).setDuration(Defaults.anim(view.getContext(), 90)).start();
            } else if (a == MotionEvent.ACTION_UP || a == MotionEvent.ACTION_CANCEL) {
                view.animate().scaleX(1f).scaleY(1f).setDuration(Defaults.anim(view.getContext(), 160)).start();
            }
            return false;
        });
    }

    /** Rows sliding in one after another. Null when animations are off. */
    static android.view.animation.LayoutAnimationController rowAnim(Context c) {
        if (!Defaults.animOn(c)) return null;
        android.view.animation.AnimationSet set = new android.view.animation.AnimationSet(true);
        set.setInterpolator(new android.view.animation.DecelerateInterpolator(1.5f));
        android.view.animation.TranslateAnimation t = new android.view.animation.TranslateAnimation(
                android.view.animation.Animation.RELATIVE_TO_SELF, 0f,
                android.view.animation.Animation.RELATIVE_TO_SELF, 0f,
                android.view.animation.Animation.RELATIVE_TO_SELF, 0.14f,
                android.view.animation.Animation.RELATIVE_TO_SELF, 0f);
        t.setDuration(Defaults.anim(c, 240));
        android.view.animation.AlphaAnimation a = new android.view.animation.AlphaAnimation(0f, 1f);
        a.setDuration(Defaults.anim(c, 200));
        set.addAnimation(t);
        set.addAnimation(a);
        android.view.animation.LayoutAnimationController lc =
                new android.view.animation.LayoutAnimationController(set, 0.12f);
        lc.setOrder(android.view.animation.LayoutAnimationController.ORDER_NORMAL);
        return lc;
    }

    /** Opens another screen with Kiln's slide, or none when animations are off. */
    static void noSlideIfOff(android.app.Activity a) {
        if (!Defaults.animOn(a)) a.overridePendingTransition(0, 0);
    }

    private static int dim(int color) {
        int a = color >>> 24;
        int r = (color >> 16 & 255) * 85 / 100, g = (color >> 8 & 255) * 85 / 100, b = (color & 255) * 85 / 100;
        return a << 24 | r << 16 | g << 8 | b;
    }

    private static int fade(int color) {
        return ((color >>> 24) / 3) << 24 | (color & 0xFFFFFF);
    }

    static TextView label(Context c, String s, int sp, boolean bold, boolean secondary) {
        TextView t = new TextView(c);
        t.setText(s);
        t.setTextSize(sp);
        t.setTextColor(secondary ? text2(c) : text(c));
        if (bold) t.setTypeface(Typeface.DEFAULT_BOLD);
        return t;
    }

    static Button button(Context c, String label, int kind, View.OnClickListener l) {
        Button b = new Button(c);
        b.setText(label);
        b.setAllCaps(false);
        b.setTextSize(15);
        b.setTypeface(Typeface.DEFAULT_BOLD);
        b.setStateListAnimator(null);                  // no lift/elevation animation
        b.setMinHeight(0);
        b.setMinimumHeight(dp(c, 44));
        b.setMinWidth(0);
        b.setMinimumWidth(dp(c, 64));
        b.setPadding(dp(c, 16), dp(c, 10), dp(c, 16), dp(c, 10));   // room for a wrapped label
        int acc = accent(c);
        switch (kind) {
            case FILLED:
                b.setTextColor(c.getColor(R.color.k_on_accent));
                b.setBackground(pressable(c, acc, 0, 10));
                break;
            case OUTLINE:
                b.setTextColor(acc);
                b.setBackground(pressable(c, 0, line(c), 10));
                break;
            case DANGER:
                b.setTextColor(c.getColor(R.color.k_danger));
                b.setBackground(pressable(c, 0, c.getColor(R.color.k_danger), 10));
                break;
            case TONAL:
                b.setTextColor(acc);
                b.setBackground(pressable(c, c.getColor(R.color.k_chip), 0, 10));
                break;
            default:
                b.setTextColor(acc);
                b.setBackground(pressable(c, 0, 0, 10));
                break;
        }
        pressScale(b);
        b.setOnClickListener(l);
        return b;
    }

    static ImageButton iconButton(Context c, int res, View.OnClickListener l) {
        ImageButton b = new ImageButton(c);
        b.setImageResource(res);
        b.setImageTintList(ColorStateList.valueOf(text(c)));
        b.setBackground(pressable(c, 0, 0, 24));
        pressScale(b);
        b.setOnClickListener(l);
        return b;
    }

    static void tint(ImageView v, int color) { v.setImageTintList(ColorStateList.valueOf(color)); }

    static EditText field(Context c, String hint, boolean secret) {
        EditText e = new EditText(c);
        e.setHint(hint);
        e.setSingleLine(true);
        e.setTextSize(16);
        e.setTextColor(text(c));
        e.setHintTextColor(text2(c));
        e.setPadding(dp(c, 14), dp(c, 12), dp(c, 14), dp(c, 12));
        e.setBackground(shape(c, bg(c), line(c), 10));
        e.setInputType(secret
                ? (InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD)
                : InputType.TYPE_CLASS_TEXT);
        return e;
    }

    static CheckBox check(Context c, String label, boolean on) {
        CheckBox b = new CheckBox(c);
        b.setText(label);
        b.setTextSize(15);
        b.setTextColor(text(c));
        b.setChecked(on);
        return b;
    }

    static void setSecret(EditText e, boolean visible) {
        int sel = e.getSelectionEnd();
        e.setInputType(InputType.TYPE_CLASS_TEXT | (visible
                ? InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                : InputType.TYPE_TEXT_VARIATION_PASSWORD));
        e.setSelection(Math.max(sel, 0));
    }

    static ViewGroup.MarginLayoutParams margins(ViewGroup.MarginLayoutParams p, Context c,
                                                int l, int t, int r, int b) {
        p.setMargins(dp(c, l), dp(c, t), dp(c, r), dp(c, b));
        return p;
    }

    static int gravityEnd() { return Gravity.END; }

    /** A ScrollView with a height cap, so a modal never outgrows the room above the keyboard. */
    static final class MaxScroll extends ScrollView {
        int limit = -1;               // pixels; -1 = 60% of the screen

        MaxScroll(Context c) { super(c); }

        @Override
        protected void onMeasure(int w, int h) {
            int max = limit >= 0 ? limit : (int) (getResources().getDisplayMetrics().heightPixels * 0.6f);
            super.onMeasure(w, MeasureSpec.makeMeasureSpec(max, MeasureSpec.AT_MOST));
        }
    }

    // ---- small cards used by the info screens (About, Archive info, Benchmark, WAV info)

    /** A rounded card of label/value rows with thin lines between. A third item, "stack", puts the
     *  value under its label (for long things such as a path). */
    static LinearLayout detailCard(Context c, java.util.List<String[]> rows) {
        LinearLayout card = new LinearLayout(c);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(dp(c, 14), dp(c, 4), dp(c, 14), dp(c, 4));
        card.setBackground(shape(c, c.getColor(R.color.k_chip), 0, 14));
        for (int i = 0; i < rows.size(); i++) {
            String[] r = rows.get(i);
            boolean stack = r.length > 2 && "stack".equals(r[2]);
            if (i > 0) card.addView(rule(c));
            LinearLayout row = new LinearLayout(c);
            row.setPadding(0, dp(c, 11), 0, dp(c, 11));
            TextView k = label(c, r[0], 14, false, true);
            TextView v = label(c, r[1], 14, true, false);
            if (stack) {
                row.setOrientation(LinearLayout.VERTICAL);
                row.addView(k);
                v.setTextIsSelectable(true);
                LinearLayout.LayoutParams vp = new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
                vp.topMargin = dp(c, 3);
                row.addView(v, vp);
            } else {
                row.setGravity(Gravity.CENTER_VERTICAL);
                row.addView(k, new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT));
                v.setGravity(Gravity.END);
                LinearLayout.LayoutParams vp = new LinearLayout.LayoutParams(
                        0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
                vp.leftMargin = dp(c, 16);
                row.addView(v, vp);
            }
            card.addView(row);
        }
        return card;
    }

    /** A thin separator line. */
    static View rule(Context c) {
        View ln = new View(c);
        ln.setBackgroundColor(line(c));
        ln.setLayoutParams(new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 1));
        return ln;
    }

    /** A big number over a small label, in a rounded tile. */
    static View statTile(Context c, String big, String small) {
        LinearLayout t = new LinearLayout(c);
        t.setOrientation(LinearLayout.VERTICAL);
        t.setGravity(Gravity.CENTER);
        t.setPadding(dp(c, 6), dp(c, 12), dp(c, 6), dp(c, 12));
        t.setBackground(shape(c, c.getColor(R.color.k_chip), 0, 14));
        TextView b = label(c, big, 18, true, false);
        b.setGravity(Gravity.CENTER);
        b.setSingleLine(true);
        t.addView(b);
        TextView sm = label(c, small, 12, false, true);
        sm.setGravity(Gravity.CENTER);
        t.addView(sm);
        return t;
    }

    /** Layout for one tile in a row of equal tiles; `gap` leaves room beside it. */
    static LinearLayout.LayoutParams tileLp(Context c, boolean leftGap, boolean rightGap) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        lp.setMargins(leftGap ? dp(c, 4) : 0, 0, rightGap ? dp(c, 4) : 0, 0);
        return lp;
    }

    /** A thin horizontal bar, 0..1. */
    static ProgressBar meter(Context c, double frac, boolean accent) {
        ProgressBar bar = new ProgressBar(c, null, android.R.attr.progressBarStyleHorizontal);
        bar.setMax(1000);
        bar.setProgress((int) Math.max(Math.round(Math.min(frac, 1.0) * 1000), 8));   // never looks empty
        bar.setProgressTintList(ColorStateList.valueOf(accent ? accent(c) : text2(c)));
        bar.setProgressBackgroundTintList(ColorStateList.valueOf(line(c)));
        bar.setLayoutParams(new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(c, 8)));
        return bar;
    }
}
