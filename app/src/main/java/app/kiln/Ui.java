package app.kiln;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.StateListDrawable;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.ImageButton;
import android.widget.ImageView;
import android.widget.ScrollView;
import android.widget.TextView;

/** Flat, quiet building blocks shared by every screen. Colors come from res/values(-night). */
final class Ui {
    private Ui() {}

    static final int FILLED = 0, OUTLINE = 1, TEXT = 2, DANGER = 3;

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

    /** A plain pressed-state change instead of an animated ripple. */
    static StateListDrawable pressable(Context c, int fill, int stroke, float radiusDp) {
        StateListDrawable s = new StateListDrawable();
        int pressed = (fill == 0 || (fill >>> 24) == 0) ? c.getColor(R.color.k_sel) : dim(fill);
        s.addState(new int[]{android.R.attr.state_pressed}, shape(c, pressed, stroke, radiusDp));
        s.addState(new int[]{-android.R.attr.state_enabled}, shape(c, fill == 0 ? 0 : fade(fill), stroke == 0 ? 0 : fade(stroke), radiusDp));
        s.addState(new int[]{}, shape(c, fill, stroke, radiusDp));
        return s;
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
        b.setPadding(dp(c, 16), 0, dp(c, 16), 0);
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
            default:
                b.setTextColor(acc);
                b.setBackground(pressable(c, 0, 0, 10));
                break;
        }
        b.setOnClickListener(l);
        return b;
    }

    static ImageButton iconButton(Context c, int res, View.OnClickListener l) {
        ImageButton b = new ImageButton(c);
        b.setImageResource(res);
        b.setImageTintList(ColorStateList.valueOf(text(c)));
        b.setBackground(pressable(c, 0, 0, 24));
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

    /** A ScrollView that never grows past 50% of the screen, so sheets stay reachable. */
    static final class MaxScroll extends ScrollView {
        MaxScroll(Context c) { super(c); }

        @Override
        protected void onMeasure(int w, int h) {
            int max = (int) (getResources().getDisplayMetrics().heightPixels * 0.5f);
            super.onMeasure(w, MeasureSpec.makeMeasureSpec(max, MeasureSpec.AT_MOST));
        }
    }
}
